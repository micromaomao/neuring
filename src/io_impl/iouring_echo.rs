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

use std::{io, time::Instant};

use io_uring::IoUring;

use crate::{
  errors::AppError,
  io_impl::common::{get_sockaddr, setup_recv_socket},
  stats::StatsAggregator,
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
  sqpoll_idle: u32,
) -> Result<(), AppError> {
  assert!(ring_size > 0 && ring_size.is_power_of_two());
  let resolved_addr = get_sockaddr(listen_addr)?;

  let mut socks = Vec::with_capacity(nb_sockets);
  for _ in 0..nb_sockets {
    let sock_fd = setup_recv_socket(&resolved_addr)?;
    let ring = build_ring(ring_size, sqpoll_idle, sock_fd).map_err(AppError::IoUringError)?;
    let ring_size = ring_size as usize;
    let sock_struct = unsafe {
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
      }
    };
    socks.push(sock_struct);
    let sock_struct = socks.last_mut().unwrap();

    // Fill ring with recv requests
    for idx in 0..2 {
      sock_struct.push_recv(idx)?;
    }
    sock_struct
      .ring
      .submitter()
      .submit()
      .map_err(AppError::IoUringError)?;
  }

  loop {
    for i in 0..nb_sockets {
      let sock = &mut socks[i];
      if let Err(e) = sock.check_cq() {
        eprintln!("Error encountered in socket {i}: {e}");
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
  fn push_recv(&mut self, index: usize) -> Result<(), AppError> {
    // dbg!(("recv", index));
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
    let entry = io_uring::opcode::RecvMsg::new(fd, &mut self.msghdr_buf[index] as *mut _)
      .build()
      .user_data(index as u64);

    unsafe {
      let mut subm = self.ring.submission();

      subm
        .push(&entry)
        .map_err(|_| AppError::IoUringFull("recvmsg", index, subm.capacity()))?;
    }

    self.state_buf[index] = PacketSlotState::RecvInProgress;
    Ok(())
  }

  /// Consume and handle all new entries in the completion queue.
  fn check_cq(&mut self) -> Result<(), AppError> {
    // To work around lifetime issues, we can't keep the ring or its queues
    // borrowed, but re-borrowing it is free anyway.

    loop {
      let entry = self.ring.completion().next();
      if entry.is_none() {
        break;
      }
      let entry = entry.unwrap();
      let index = entry.user_data() as usize;
      // dbg!((index, self.state_buf[index]));
      match self.state_buf[index] {
        PacketSlotState::RecvInProgress => {
          // dbg!(("recv res", index, entry.result()));
          if entry.result() <= 0 {
            // Recv failed (or no packets), ignore and retry.
            self.push_recv(index)?;
          } else {
            // Recv completed and we have the packet now, so send it straight
            // back.  But we need to update the iovec with the actual message
            // length.  No need to change any other fields.
            self.iovec_buf[index].iov_len = usize::try_from(entry.result()).unwrap();
            let fd = io_uring::types::Fixed(0);
            let send_entry =
              io_uring::opcode::SendMsgZc::new(fd, &self.msghdr_buf[index] as *const _)
                .build()
                .user_data(index as u64);
            let mut subm = self.ring.submission();
            unsafe {
              subm
                .push(&send_entry)
                .map_err(|_| AppError::IoUringFull("sendmsgzc", index, subm.capacity()))?;
            }
            // dbg!(("send", index));
            self.state_buf[index] = PacketSlotState::SendInProgress;
          }
        }
        PacketSlotState::SendInProgress => {
          // dbg!(("send res", index, entry.result()));
          // Send completed (or failed), so we can go back to recv now for the next packet.
          self.push_recv(index)?;
        }
      }
    }

    self.ring.submit().map_err(AppError::IoUringError)?;

    Ok(())
  }
}
