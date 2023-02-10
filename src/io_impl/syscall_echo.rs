//! Implementation of a simple multi-threaded packet echo server using normal
//! `recv` and `send` syscalls.
//!
//! Multi-threading is implemented by using multiple sockets (binding to the
//! same address with SO_REUSEPORT). This works better than sharing the same
//! socket across threads.

use crate::io_impl::common::{get_sockaddr, setup_recv_socket};
use crate::io_impl::sys::{recvfrom, sendto};
use crate::stats;
use crate::{errors::AppError, stats::StatsAggregator};

use std::sync::atomic::Ordering;
use std::thread;
use std::time::Instant;

pub fn syscall_echo(
  listen_addr: &str,
  mtu: usize,
  nb_sockets: usize,
  start_time: Instant,
  stats: &StatsAggregator,
) -> Result<(), AppError> {
  let resolved_addr = get_sockaddr(listen_addr)?;
  thread::scope(|scope| {
    for tid in 0..nb_sockets {
      let sock_fd = setup_recv_socket(&resolved_addr)?;

      scope.spawn(move || {
        let mut recv_buf = vec![0u8; mtu];
        loop {
          let recv_res = unsafe { recvfrom(sock_fd, &mut recv_buf) };
          if recv_res.is_err() {
            continue;
          }
          let recv_res = recv_res.unwrap();
          let recv_time = stats::get_time_value_now(start_time);
          let send_res = unsafe {
            sendto(
              sock_fd,
              &recv_buf[..recv_res.recv_size],
              &recv_res.src_addr,
              recv_res.src_addr_len,
            )
          };
          stats.access_step(recv_time, |stats| {
            stats.rx_packets.fetch_add(1, Ordering::Relaxed);
            if send_res.is_ok() {
              stats.tx_packets.fetch_add(1, Ordering::Relaxed);
            }
          });
        }
      });
    }
    Ok(())
  })
}
