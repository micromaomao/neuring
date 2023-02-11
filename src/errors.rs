use std::io;

use thiserror::Error;

/// A shared errors struct
#[derive(Debug, Error)]
pub enum AppError {
  #[error("Unfortunately, {0} is not implemented yet.")]
  NotImplemented(&'static str),
  #[error("{0}: {1}")]
  IOError(&'static str, #[source] io::Error),
  #[error("Unable to resolve {0}: {1}")]
  UnableToResolveNetAddr(String, String),
  #[error("Packet size too large.")]
  PacketSizeTooLarge,
  #[error("Unable to open stats file for writing: {0}")]
  StatsFileError(#[source] io::Error),
  #[error("io_uring: {0}")]
  IoUringError(#[source] io::Error),
  #[error("io_uring: queue full while pushing {0}({1}) (capacity {2})")]
  IoUringFull(&'static str, usize, usize),
}
