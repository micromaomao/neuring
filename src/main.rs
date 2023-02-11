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

  #[arg(global(true), short = 'i', long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
  /// Interval in milliseconds between stat steps.
  stats_interval_ms: u64,

  #[arg(global(true), short = 't', long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
  /// Number of seconds between stats dump.
  stats_evict_interval_secs: u64,

  #[arg(global(true), short = 'T', long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
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
  /// Send packets with normal syscalls
  #[clap(name = "syscall-send")]
  SyscallSendrecv {
    #[arg(required = true)]
    /// Address to send to, in the form host:port
    server_addr: String,

    #[arg(long, value_parser = positive_usize_parser, default_value_t = 1)]
    /// Amount of packets to send at one time. Setting this to a high value may
    /// cause inaccurate latency stats.  If this value is 1, plain `send` will
    /// be used, otherwise `sendmmsg` will be used.
    batch_size: usize,

    #[arg(long, short = 'j', value_parser = positive_usize_parser, default_value_t = 1)]
    /// Number of sockets to use.  Each socket will be handled by 2 new threads
    /// - one for sending and one for receiving.
    nb_sockets: usize,
  },

  /// An echo server with normal syscalls
  #[clap(name = "syscall-echo")]
  SyscallEcho {
    #[arg(required = true)]
    /// Address to listen on, in the form host:port
    server_addr: String,

    #[arg(long, short = 'j', value_parser = positive_usize_parser, default_value_t = 1)]
    /// Number of sockets to use.  Each socket will be handled by 2 new threads
    /// - one for sending and one for receiving.
    nb_sockets: usize,

    #[arg(long, value_parser = positive_usize_parser, default_value_t = 2000)]
    /// The maximum size of a packet we will process
    mtu: usize,
  },

  /// io_uring-based echo server
  #[clap(name = "io-uring-echo")]
  IoUringEcho {
    #[arg(required = true)]
    /// Address to listen on, in the form host:port
    server_addr: String,

    #[arg(long, short = 'j', value_parser = positive_usize_parser, default_value_t = 1)]
    /// Number of sockets to use.  Each socket will be handled by a separate
    /// ring, but there will only be one user thread in any case.
    nb_sockets: usize,

    #[arg(long, value_parser = positive_usize_parser, default_value_t = 2000)]
    /// The maximum size of a packet we will process
    mtu: usize,

    #[arg(long, short = 'r', value_parser = clap::value_parser!(u32).range(2..), default_value_t = 32768)]
    /// The size of the io_uring ring, must be a power of 2.  Depending on your
    /// system there might be further limits.
    ring_size: u32,

    #[arg(long, value_parser = clap::value_parser!(u32).range(0..), default_value_t = 1000)]
    /// The number of milliseconds to wait for a packet to arrive before the
    /// kernel stops polling.  Kernel polling will not be used if this is zero,
    /// effectively making this a single-threaded async IO benchmark.
    kernel_poll_timeout: u32,

    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), default_value_t = 32)]
    /// Number of recv requests to send to the kernel.
    nb_recv: u32,
  },
}

fn run() -> Result<(), AppError> {
  let cli = Cli::parse();
  let stats = make_stats_aggregator_from_arg(&cli)?;
  match cli.command {
    Commands::SyscallSendrecv {
      ref server_addr,
      batch_size,
      nb_sockets,
    } => io_impl::syscall_sendrecv::syscall_sendrecv(
      server_addr,
      cli.packet_size as usize,
      batch_size,
      cli.seed,
      nb_sockets,
      &stats,
      Instant::now(),
    ),
    Commands::SyscallEcho {
      ref server_addr,
      nb_sockets,
      mtu,
    } => io_impl::syscall_echo::syscall_echo(server_addr, mtu, nb_sockets, Instant::now(), &stats),
    Commands::IoUringEcho {
      ref server_addr,
      nb_sockets,
      mtu,
      ring_size,
      kernel_poll_timeout,
      nb_recv,
    } => io_impl::iouring_echo::iouring_echo(
      server_addr,
      mtu,
      nb_sockets,
      Instant::now(),
      &stats,
      ring_size,
      nb_recv,
      kernel_poll_timeout,
    ),
  }
}

fn main() {
  if let Err(e) = run() {
    eprintln!("Error: {}", e);
    process::exit(1);
  }
}
