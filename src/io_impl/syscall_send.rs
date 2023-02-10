//! Implementation of a packet sender using either the `send` or `sendmmsg`
//! syscall.

use std::io;
use std::mem::MaybeUninit;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::errors::AppError;
use crate::io_impl::common::setup_socket;
use crate::pkt::write_packet;
use crate::stats::StatsAggregator;

pub fn syscall_send(
  dest_addr: &str,
  packet_size: usize,
  batch_size: usize,
  seed: u64,
  stats_agg: StatsAggregator,
) -> Result<(), AppError> {
  let sock_fd = setup_socket(dest_addr, true)?;
  eprintln!("Ready to send to {dest_addr}.");
  if batch_size == 1 {
    let mut buf = vec![0u8; packet_size];
    let mut index = 0;
    let start_time = Instant::now();
    loop {
      let time = start_time.elapsed().as_millis() as u64;
      write_packet(seed, index, time, &mut buf);
      unsafe { send(sock_fd, &buf) }?;
      stats_agg.access_step(time, |stats| {
        stats.tx_packets.fetch_add(1, Ordering::Relaxed);
      });
      index += 1;
    }
  } else {
    let mut iovec_buf: Box<[MaybeUninit<libc::iovec>]> = Box::new_uninit_slice(batch_size);
    let mut mmsghdr_buf: Box<[MaybeUninit<libc::mmsghdr>]> = Box::new_uninit_slice(batch_size);
    let mut pkt_buf: Vec<u8> = vec![0u8; packet_size * batch_size];
    let mut index = 0;
    let start_time = Instant::now();
    loop {
      let time = start_time.elapsed().as_millis() as u64;
      unsafe {
        for i in 0..batch_size {
          let pkt_slice = &mut pkt_buf[i * packet_size..(i + 1) * packet_size];
          write_packet(seed, index, time, pkt_slice);
          iovec_buf[i] = MaybeUninit::new(libc::iovec {
            iov_base: pkt_slice.as_ptr() as *const libc::c_void as *mut _,
            iov_len: pkt_slice.len(),
          });
          mmsghdr_buf[i] = MaybeUninit::new(libc::mmsghdr {
            msg_hdr: libc::msghdr {
              msg_name: std::ptr::null_mut(),
              msg_namelen: 0,
              msg_iov: iovec_buf[i].assume_init_ref() as *const libc::iovec as *mut _,
              msg_iovlen: 1,
              msg_control: std::ptr::null_mut(),
              msg_controllen: 0,
              msg_flags: 0,
            },
            msg_len: 0,
          });
          index += 1;
        }
        sendmmsg(
          sock_fd,
          MaybeUninit::slice_assume_init_mut(&mut mmsghdr_buf[..]),
        )?;
        stats_agg.access_step(time, |stats| {
          stats
            .tx_packets
            .fetch_add(batch_size as u64, Ordering::Relaxed);
        });
      }
    }
  }
}

const SEND_FLAGS: libc::c_int = libc::MSG_CONFIRM | libc::MSG_NOSIGNAL;

unsafe fn send(sock_fd: libc::c_int, packet_data: &[u8]) -> Result<(), AppError> {
  unsafe {
    let ret = libc::send(
      sock_fd,
      packet_data.as_ptr() as *const _,
      packet_data.len(),
      SEND_FLAGS,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EMSGSIZE {
        return Err(AppError::PacketSizeTooLarge);
      }
      return Err(AppError::IOError("send", io::Error::last_os_error()));
    }
    #[cfg(debug_assertions)]
    if ret != packet_data.len() as _ {
      unreachable!("Did not send the full packet...?");
      // There is no "partial write" for UDP - if the message is larger than
      // the max length allowable it will return EMSGSIZE.
    }
    Ok(())
  }
}

unsafe fn sendmmsg(sock_fd: libc::c_int, pkts: &mut [libc::mmsghdr]) -> Result<(), AppError> {
  let mut rest = &mut pkts[..];
  while !rest.is_empty() {
    unsafe {
      let ret = libc::sendmmsg(
        sock_fd,
        rest.as_mut_ptr(),
        rest.len().try_into().unwrap(),
        SEND_FLAGS,
      );
      if ret == -1 {
        let errno = *libc::__errno_location();
        if errno == libc::EMSGSIZE {
          return Err(AppError::PacketSizeTooLarge);
        }
        return Err(AppError::IOError("sendmmsg", io::Error::last_os_error()));
      }
      rest = &mut rest[usize::try_from(ret).unwrap()..];
    }
  }
  Ok(())
}
