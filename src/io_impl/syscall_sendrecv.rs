//! Implementation of a multi-threaded packet send and receiver using either the
//! `send` or `sendmmsg` syscall for sending, and a `recv` loop for receiving.
//!
//! Note that on each thread we create a new socket, rather than sharing the
//! same socket across all threads, which means that we use multiple ports
//! simoultaneously for sending. This is deliberate in order to more accurately
//! mimic real-world use cases, and also to prevent weird resource distributions
//! in the Linux kernel due to hashing by flow.
//!
//! See https://lwn.net/Articles/542629/

use std::io;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use crate::errors::AppError;
use crate::io_impl::common::{get_sockaddr, get_socket_local_port, setup_send_socket};
use crate::pkt::{parse_packet, write_packet};
use crate::stats::{self, StatsAggregator};

pub fn syscall_sendrecv(
  dest_addr: &str,
  packet_size: usize,
  batch_size: usize,
  seed: u64,
  nb_threads: usize,
  stats_agg: &StatsAggregator,
  start_time: Instant,
) -> Result<(), AppError> {
  let index = AtomicU64::new(0);
  let resolved_addr = get_sockaddr(dest_addr)?;
  thread::scope(|scope| -> Result<(), AppError> {
    for tid in 0..nb_threads {
      let sock_fd = setup_send_socket(&resolved_addr)?;
      let local_port = unsafe { get_socket_local_port(sock_fd) }?;

      eprintln!("Thread {tid}-send will send from local port {local_port} to {dest_addr}.");
      let tx_next_index = &index;
      scope.spawn(move || {
        if batch_size == 1 {
          // Just use `send` for single-packet batches.
          let mut buf = vec![0u8; packet_size];
          loop {
            let next_ind = tx_next_index.fetch_add(1, Ordering::Relaxed);
            let time = stats::get_time_value_now(start_time);
            write_packet(seed, next_ind, time, &mut buf);
            let _ = unsafe { send(sock_fd, &buf) };
            stats_agg.access_step(time, |stats| {
              stats.tx_packets.fetch_add(1, Ordering::Relaxed);
            });
          }
        } else {
          // Pre-allocate a bunch of buffers that we will re-use for each batch.
          let mut iovec_buf: Box<[MaybeUninit<libc::iovec>]> = Box::new_uninit_slice(batch_size);
          let mut mmsghdr_buf: Box<[MaybeUninit<libc::mmsghdr>]> =
            Box::new_uninit_slice(batch_size);
          let mut pkt_buf: Vec<u8> = vec![0u8; packet_size * batch_size];

          loop {
            let time = stats::get_time_value_now(start_time);

            // To not have to do atomics for each packet, we reserve a chunk
            // of indices up-front.
            let reserved_ind_chunk_start =
              tx_next_index.fetch_add(batch_size as u64, Ordering::Relaxed);

            unsafe {
              for i in 0..batch_size {
                let pkt_index = reserved_ind_chunk_start + i as u64;
                let pkt_slice = &mut pkt_buf[i * packet_size..(i + 1) * packet_size];
                write_packet(seed, pkt_index, time, pkt_slice);

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
              }

              let _ = sendmmsg(
                sock_fd,
                MaybeUninit::slice_assume_init_mut(&mut mmsghdr_buf[..]),
              );
              stats_agg.access_step(time, |stats| {
                stats
                  .tx_packets
                  .fetch_add(batch_size as u64, Ordering::Relaxed);
              });
            }
          }
        }
      });

      // recv loop
      scope.spawn(move || {
        // Use a slightly larger buffer to detect wrong packet sizes.
        let mut recv_buf = vec![0u8; packet_size + 4];
        loop {
          let recv_res = unsafe { recv(sock_fd, &mut recv_buf) };
          if recv_res.is_err() {
            continue;
          }
          let recv_size = recv_res.unwrap();
          let recv_time = stats::get_time_value_now(start_time);
          if recv_size != packet_size {
            // Ignore
            continue;
          }
          match parse_packet(seed, &recv_buf[0..recv_size]) {
            Ok(pkt_header) => {
              let send_time = pkt_header.send_time;
              if send_time > recv_time {
                // Ignore
                continue;
              }
              stats_agg.access_step(recv_time, |stats| {
                stats.rx_packets.fetch_add(1, Ordering::Relaxed);
              });
              stats_agg.access_step(send_time, |stats| {
                stats.rx_packets_sent_here.fetch_add(1, Ordering::Relaxed);
                stats
                  .total_latency_sent_here
                  .fetch_add(recv_time - send_time, Ordering::Relaxed);
              });
            }
            Err(_) => {
              // Ignore
              continue;
            }
          };
        }
      });
    }
    Ok(())
  })
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

unsafe fn recv(sock_fd: libc::c_int, recv_buf: &mut [u8]) -> Result<usize, AppError> {
  unsafe {
    let ret = libc::recv(
      sock_fd,
      recv_buf.as_mut_ptr() as *mut _,
      recv_buf.len(),
      libc::MSG_TRUNC,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
        return Ok(0);
      }
      return Err(AppError::IOError("recv", io::Error::last_os_error()));
    }
    debug_assert!(ret >= 0);
    Ok(ret as usize)
  }
}
