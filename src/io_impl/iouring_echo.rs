//! A multi-queue echo server implemented with io_uring.
//!
//! This implementation uses a separate ring for each "queue" (i.e. socket), as
//! this is the easiest way to implement, and is likely also the fastest, since
//! the kernel will use separate poll threads.
//!
//! Essentially, we send a bunch of recv requests to the kernel, and whenever we
//! get a result from any of those, we send a send request for that packet to
//! echo it back.
//!
//! For each ring, we allocate some fixed-length buffers to hold stuff like
//! iovec, sockaddr, msghdr and packet data. Each entry in our submission queue
//! will have a index added to their user data, representing the buffer index of
//! the packet in question.  When we get a completion for a recv, we can turn
//! around and send a send request with the same index, thus automatically
//! re-using the buffer data - since we're echoing it back anyway.  Once we
//! received a completion for our send, we can use that index for another
//! packet, so we send a recv request with the same index, thus completing the
//! loop.

use std::{
  collections::HashMap,
  io,
  sync::atomic::Ordering,
  time::{Duration, Instant},
};

use io_uring::IoUring;

use crate::{
  errors::AppError,
  io_impl::common::{get_sockaddr, setup_recv_socket},
  stats::{get_time_value_now, StatsAggregator},
};

/// The main entry point for the iouring echo server.
///
/// If `sqpoll_idle` is a positive number, the rings will use kernel polling, in
/// which case this number controls the idle timer.
///
/// Note that to simplify implementation, we will only use 1 user-mode thread,
/// even when kernel polling is not used. When kernel polling is used, this is
/// likely the most CPU-efficient approach.
pub fn iouring_echo(
  listen_addr: &str,
  mtu: usize,
  nb_sockets: usize,
  start_time: Instant,
  stats: &StatsAggregator,
  ring_size: u32,
  nb_recv: u32,
  sqpoll_idle: u32,
) -> Result<(), AppError> {
  assert!(ring_size > 0 && ring_size.is_power_of_two());
  assert!(nb_recv <= ring_size);
  let resolved_addr = get_sockaddr(listen_addr)?;

  let mut socks = Vec::with_capacity(nb_sockets);
  for _ in 0..nb_sockets {
    let sock_fd = setup_recv_socket(&resolved_addr)?;
    let ring = build_ring(ring_size, sqpoll_idle, sock_fd).map_err(AppError::IoUringError)?;
    let ring_size = ring_size as usize;
    let sock_struct = Socket::new(ring, ring_size, sock_fd, mtu);
    socks.push(sock_struct);
    let sock_struct = socks.last_mut().unwrap();

    // Fill ring with recv requests
    for idx in 0..usize::try_from(nb_recv).unwrap() {
      sock_struct.push_recv(idx)?;
    }

    if sqpoll_idle == 0 {
      sock_struct
        .ring
        .submitter()
        .submit()
        .map_err(AppError::IoUringError)?;
    }
  }

  let mut last_recv_report = Instant::now();

  loop {
    for i in 0..nb_sockets {
      let sock = &mut socks[i];
      if let Err(e) = sock.check_cq(stats, start_time) {
        eprintln!("Error encountered in socket {i}: {e}");
      }
      if sqpoll_idle == 0 {
        sock
          .ring
          .submitter()
          .submit()
          .map_err(AppError::IoUringError)?;
      }
      let now = Instant::now();
      if sock.nb_active_recv + 2 < nb_recv as usize {
        if now - last_recv_report > std::time::Duration::from_secs(5) {
          last_recv_report += Duration::from_secs(1); // Report again in 1 second.
          eprintln!(
            "Socket {i} has only {active_recv} recv requests in flight, but should have {nb_recv}. This has persisted for at least 5 second.",
            active_recv = sock.nb_active_recv,
          );
        }
      } else {
        last_recv_report = now;
      }
    }
  }
}

struct Socket {
  sock_fd: libc::c_int,
  ring: IoUring,
  mtu: usize,

  // We use box here to prevent accidentally moving the buffers.
  msghdr_buf: Box<[libc::msghdr]>,
  iovec_buf: Box<[libc::iovec]>,
  sockaddr_buf: Box<[libc::sockaddr_storage]>,

  /// A buffer containing mtu * ring_size bytes to store all the packet data.
  pkt_data_buf: Box<[u8]>,

  state_buf: Box<[PacketSlotState]>,
  nb_active_recv: usize,

  // For debugging
  debug: bool,
  request_tags: HashMap<u64, (usize, &'static str)>,
  next_request_tag: u64,
}

/// Use explicit values to make zero state correct.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
enum PacketSlotState {
  RecvInProgress = 0,
  SendInProgress = 1,
}

fn build_ring(
  ring_size: u32,
  sqpoll_idle: u32,
  register_sock_fd: libc::c_int,
) -> Result<IoUring, io::Error> {
  let mut builder = IoUring::builder();
  if sqpoll_idle != 0 {
    builder.setup_sqpoll(sqpoll_idle);
  }
  let ring = builder.build(ring_size)?;
  ring.submitter().register_files(&[register_sock_fd])?;
  Ok(ring)
}

impl Socket {
  fn new(ring: IoUring, ring_size: usize, sock_fd: libc::c_int, mtu: usize) -> Self {
    let mut sock = unsafe {
      Socket {
        ring,
        sock_fd,
        mtu,
        msghdr_buf: Box::new_zeroed_slice(ring_size).assume_init(),
        iovec_buf: Box::new_zeroed_slice(ring_size).assume_init(),
        sockaddr_buf: Box::new_zeroed_slice(ring_size).assume_init(),
        pkt_data_buf: Box::new_zeroed_slice(ring_size * mtu).assume_init(),
        // assume_init is safe since the enum is repr(C) and 0 is what we want.
        state_buf: Box::new_zeroed_slice(ring_size).assume_init(),
        nb_active_recv: 0,
        debug: false,
        request_tags: HashMap::new(),
        next_request_tag: 0,
      }
    };
    #[cfg(debug_assertions)]
    {
      sock.debug = true;
    }
    sock
  }

  #[inline]
  fn make_user_data(&mut self, index: usize, request_type: &'static str) -> u64 {
    if self.debug {
      let tag = self.next_request_tag;
      self.next_request_tag += 1;
      if self
        .request_tags
        .insert(tag, (index, request_type))
        .is_some()
      {
        panic!("Duplicate tag");
      } else {
        // eprintln!("Sending CQE #{tag} for {request_type}");
      }
      tag
    } else {
      index as u64
    }
  }

  #[inline]
  fn parse_user_data(&mut self, user_data: u64) -> usize {
    if self.debug {
      match self.request_tags.remove(&user_data) {
        Some((index, request_type)) => {
          // eprintln!("Received CQE #{user_data} for {request_type}");
          index
        }
        None => panic!("Received non-existent CEQ #{user_data}"),
      }
    } else {
      user_data as usize
    }
  }

  unsafe fn push_entry(
    &mut self,
    mut entry: io_uring::squeue::Entry,
    index: usize,
    request_type: &'static str,
  ) -> Result<(), AppError> {
    let ud = self.make_user_data(index, request_type);
    entry = entry.user_data(ud);
    let mut sq = self.ring.submission();
    if sq.push(&entry).is_err() {
      drop(sq);
      if self.debug {
        self.request_tags.remove(&ud).unwrap();
        self.debug_report_queue_full(index, request_type);
      }
      Err(AppError::IoUringFull(request_type, index))
    } else {
      Ok(())
    }
  }

  fn debug_report_queue_full(&mut self, index: usize, request_type: &'static str) {
    let sq = self.ring.submission();
    eprintln!(
      "SQ full while pushing {request_type}({index}) - current len is {slen}, cap is {cap}",
      slen = sq.len(),
      cap = sq.capacity()
    );
    eprintln!(
      "dbg: SQ should have {hmlen} entries",
      hmlen = self.request_tags.len()
    );
    if self.request_tags.len() <= 16 {
      dbg!(&self.request_tags);
    }
  }

  fn push_recv(&mut self, index: usize) -> Result<(), AppError> {
    self.iovec_buf[index] = libc::iovec {
      iov_base: &mut self.pkt_data_buf[index * self.mtu] as *mut _ as *mut _,
      iov_len: self.mtu,
    };
    self.msghdr_buf[index] = libc::msghdr {
      msg_name: &mut self.sockaddr_buf[index] as *mut _ as *mut _,
      msg_namelen: std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
      msg_iov: &mut self.iovec_buf[index] as *mut _ as *mut _,
      msg_iovlen: 1,
      msg_control: std::ptr::null_mut(),
      msg_controllen: 0,
      msg_flags: 0,
    };

    let fd = io_uring::types::Fixed(0);
    let entry = io_uring::opcode::RecvMsg::new(fd, &mut self.msghdr_buf[index] as *mut _).build();

    unsafe {
      self.push_entry(entry, index, "recv")?;
    }

    self.state_buf[index] = PacketSlotState::RecvInProgress;
    self.nb_active_recv += 1;
    Ok(())
  }

  fn push_send(&mut self, index: usize) -> Result<(), AppError> {
    self.msghdr_buf[index].msg_control = std::ptr::null_mut();
    self.msghdr_buf[index].msg_controllen = 0;
    self.msghdr_buf[index].msg_flags = 0;

    let fd = io_uring::types::Fixed(0);
    let send_entry =
      io_uring::opcode::SendMsg::new(fd, &self.msghdr_buf[index] as *const _).build();
    unsafe {
      self.push_entry(send_entry, index, "sendmsg")?;
    }
    // dbg!(("send", index));
    self.state_buf[index] = PacketSlotState::SendInProgress;
    Ok(())
  }

  /// Consume and handle all new entries in the completion queue.
  fn check_cq(&mut self, stats: &StatsAggregator, start_time: Instant) -> Result<(), AppError> {
    // To work around lifetime issues, we can't keep the ring or its queues
    // borrowed, but re-borrowing it is free anyway.

    loop {
      if self.ring.submission().need_wakeup() {
        self.ring.submit().map_err(AppError::IoUringError)?;
      }
      if self.ring.submission().is_full() {
        self
          .ring
          .submitter()
          .squeue_wait()
          .map_err(AppError::IoUringError)?;
      }
      let entry = self.ring.completion().next();
      if entry.is_none() {
        break;
      }
      let entry = entry.unwrap();
      let index = self.parse_user_data(entry.user_data());
      match self.state_buf[index] {
        PacketSlotState::RecvInProgress => {
          self.nb_active_recv -= 1;
          if entry.result() <= 0 {
            // Recv failed (or no packets), ignore and retry.
            self.push_recv(index)?;
          } else {
            // Recv completed and we have the packet now, so send it straight
            // back.  But we need to update the iovec with the actual message
            // length.
            self.iovec_buf[index].iov_len = usize::try_from(entry.result()).unwrap();
            self.push_send(index)?;
            stats.access_step(get_time_value_now(start_time), |stats| {
              stats.rx_packets.fetch_add(1, Ordering::Relaxed);
            });
          }
        }
        PacketSlotState::SendInProgress => {
          // Send completed (or failed), so we can go back to recv now for the next packet.
          stats.access_step(get_time_value_now(start_time), |stats| {
            stats.tx_packets.fetch_add(1, Ordering::Relaxed);
          });
          self.push_recv(index)?;
        }
      }
    }

    Ok(())
  }
}
