//! Implementation of a packet sender using io_uring.

use std::time::Instant;

use crate::errors::AppError;
use crate::io_impl::common::{get_sockaddr};
use crate::pkt::write_packet;
use crate::stats::StatsAggregator;

pub fn iouring_send(
  dest_addr: &str,
  packet_size: usize,
  ring_entries: usize,
  seed: u64,
  nb_sockets: usize,
  stats_agg: &StatsAggregator,
  start_time: Instant,
) -> Result<(), AppError> {
  let resolved_addr = get_sockaddr(dest_addr)?;
  unimplemented!()
}
