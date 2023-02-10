#![feature(new_uninit)]
#![feature(maybe_uninit_slice)]

use clap::{Parser, Subcommand};
use errors::AppError;
use stats::{get_time_value_from_duration, StatsAggregator};
use std::{
  path::PathBuf,
  process,
  time::{Duration, Instant},
};

mod errors;
mod io_impl;
mod pkt;
mod stats;

#[derive(Parser)]
#[command(version)]
/// A tool to benchmark network performance.
///
/// Don't forget to build in release mode.
pub(crate) struct Cli {
  #[command(subcommand)]
  command: Commands,

  #[arg(global(true), long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range((pkt::PACKET_HEAD_SIZE as i64)..))]
  packet_size: u32,

  #[arg(
    global(true),
    short = 'l',
    long,
    required = false,
    default_value_t = 0x39016c0e906374f9
  )]
  seed: u64,

  #[arg(global(true), short = 's', long, required = false)]
  /// Output packet stats to CSV.
  stats_file: Option<PathBuf>,

  #[arg(global(true), short = 't', long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
  /// Interval in milliseconds between stat steps.
  stats_interval_ms: u64,

  #[arg(global(true), short = 'b', long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
  /// Number of seconds between stats dump.
  stats_evict_interval_secs: u64,

  #[arg(global(true), long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
  /// On each stats dump, stats older than this many seconds will be dumped.
  stats_evict_threshold_secs: u64,
}

fn positive_usize_parser(s: &str) -> Result<usize, &'static str> {
  let val: usize = s.parse().map_err(|_| "Invalid usize")?;
  if val <= 0 {
    return Err("Invalid value");
  }
  Ok(val)
}

fn make_stats_aggregator_from_arg(cli: &Cli) -> Result<stats::StatsAggregator, AppError> {
  let stats_file = &cli.stats_file;
  let writer;
  if let Some(stats_file) = stats_file {
    writer = Some(stats::get_csv_writer(stats_file)?);
  } else {
    writer = None;
  }
  let stats = StatsAggregator::new(
    get_time_value_from_duration(Duration::from_millis(cli.stats_interval_ms)),
    get_time_value_from_duration(Duration::from_secs(cli.stats_evict_interval_secs)),
    get_time_value_from_duration(Duration::from_secs(cli.stats_evict_threshold_secs)),
    writer,
  );
  Ok(stats)
}

#[derive(Subcommand)]
enum Commands {
  /// Send packets
  Syscall {
    #[arg(required = true)]
    /// Address to send to, in the form host:port
    server_addr: String,

    #[arg(long, value_parser = positive_usize_parser, default_value_t = 1)]
    /// Amount of packets to send at one time. Setting this to a high value may
    /// cause inaccurate latency stats.  If this value is 1, plain `send` will
    /// be used, otherwise `sendmmsg` will be used.
    batch_size: usize,

    #[arg(long, value_parser = positive_usize_parser, default_value_t = 1)]
    /// Number of sockets to use.  Each socket will be handled by 2 new threads
    /// - one for sending and one for receiving.
    nb_sockets: usize,
  },
}

fn run() -> Result<(), AppError> {
  let cli = Cli::parse();
  match cli.command {
    Commands::Syscall {
      ref server_addr,
      batch_size,
      nb_sockets,
    } => io_impl::syscall_sendrecv::syscall_sendrecv(
      server_addr,
      cli.packet_size as usize,
      batch_size,
      cli.seed,
      nb_sockets,
      &make_stats_aggregator_from_arg(&cli)?,
      Instant::now(),
    ),
  }
}

fn main() {
  if let Err(e) = run() {
    eprintln!("Error: {}", e);
    process::exit(1);
  }
}
