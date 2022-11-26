use clap::{Parser, Subcommand, ValueEnum};
use errors::AppError;
use std::{path::PathBuf, process};
use syscall_mode::syscall_mode;

mod common;
mod errors;
mod packetgen;
mod syscall_mode;

#[derive(Parser)]
#[command(version)]
/// A tool to benchmark network performance.
///
/// Don't forget to build in release mode.
pub(crate) struct Cli {
  #[command(subcommand)]
  command: Commands,

  #[arg(short = 'm', long, value_enum, default_value_t = IOMode::Syscall, global(true))]
  /// Which implementation to use.
  io_mode: IOMode,

  #[arg(global(true), long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range((packetgen::PACKET_HEAD_SIZE as i64)..))]
  packet_size: u32,

  #[arg(global(true), short = 's', long, required = false)]
  /// Output packet stats to CSV.
  stats_file: Option<PathBuf>,

  #[arg(global(true), short = 'l', long, required = false, default_value_t = packetgen::SEED)]
  seed: u64,
}

#[derive(Subcommand)]
enum Commands {
  /// Send packets
  Send {
    #[arg(required = true)]
    /// Address in the form host:port
    address: String,
  },
  /// Receive and count packets
  Recv {
    #[arg(required = true)]
    /// Address in the form host:port
    address: String,
  },
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum IOMode {
  Syscall,
  #[value(id = "io_uring", alias("iouring"), alias("io-uring"))]
  IOUring,
}

fn run() -> Result<(), AppError> {
  let cli = Cli::parse();
  match cli.io_mode {
    IOMode::Syscall => syscall_mode(&cli),
    IOMode::IOUring => Err(AppError::NotImplemented("io_uring mode")),
  }
}

fn main() {
  if let Err(e) = run() {
    eprintln!("Error: {}", e);
    process::exit(1);
  }
}
