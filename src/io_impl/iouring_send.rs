//! Implementation of a packet sender using io_uring.

use crate::errors::AppError;
use crate::io_impl::common::setup_socket;
use crate::pkt::write_packet;
use crate::stats::StatsAggregator;

pub fn iouring_send(
  dest_addr: &str,
  packet_size: usize,
  ring_entries: usize,
  seed: u64,
  stats_agg: StatsAggregator,
) -> Result<(), AppError> {
  let sock_fd = setup_socket(dest_addr, true)?;
  eprintln!("Ready to send to {dest_addr}.");
  unimplemented!()
}
