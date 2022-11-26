use std::io;

use thiserror::Error;

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
}
