use std::rc::Rc;
use std::time::Instant;

use rand::rngs::SmallRng;
use rand::{RngCore, SeedableRng};
use std::mem;

use crate::common::StatsFile;
use crate::errors::AppError;
use crate::Cli;

pub(crate) struct PacketGenerator {
  zero_instant: Instant,
  packet_size: usize,
  seed: u64,
  data_buf: Vec<u8>,
  i: u64,
  last_timestep_millis: u64,
  i_last_timestep: u64,
  recv_stats: RecvStats,
  stats_file: Option<StatsFile>,
}

#[repr(C)]
pub(crate) struct PacketHeader {
  pub index: u64,
  pub millis: u64,
}

pub const SEED: u64 = 0x39016c0e906374f9;
pub const PACKET_HEAD_SIZE: usize = mem::size_of::<PacketHeader>();
pub const PACKET_CYCLE: u64 = 1024000;
pub const TIME_STEP_MILLIS: u64 = 10;
pub const RECV_MAX_BACKLOG: usize = 100;

pub(crate) fn millis_since_start(zero_instant: Instant) -> u64 {
  Instant::now().duration_since(zero_instant).as_millis() as u64
}

impl PacketGenerator {
  pub fn init(packet_size: usize, seed: u64, is_send: bool, stats_file: StatsFile) -> Self {
    assert!(packet_size >= PACKET_HEAD_SIZE);
    eprintln!(
      "Will allocate {:.2} GiB of buffer space.",
      (PACKET_CYCLE * packet_size as u64) as f64 / 1024f64.powi(3)
    );
    let mut data_buf = vec![0u8; usize::try_from(PACKET_CYCLE).unwrap() * packet_size];
    if packet_size > PACKET_HEAD_SIZE {
      let mut rng = SmallRng::seed_from_u64(seed);
      rng.fill_bytes(&mut data_buf);
    }
    let zero_instant = Instant::now();
    let self_stats_file;
    let mut recv_stats = RecvStats::new(RECV_MAX_BACKLOG);
    if is_send {
      self_stats_file = Some(stats_file);
    } else {
      self_stats_file = None;
      recv_stats.set_stats_file(stats_file);
    }
    PacketGenerator {
      zero_instant,
      packet_size,
      seed,
      data_buf,
      i: 0,
      last_timestep_millis: millis_since_start(zero_instant),
      i_last_timestep: 0,
      recv_stats,
      stats_file: self_stats_file,
    }
  }

  pub fn init_from_cli(is_send: bool, cli: &Cli) -> Result<Self, AppError> {
    let packet_size = cli.packet_size as usize;
    Ok(Self::init(
      packet_size,
      cli.seed,
      is_send,
      StatsFile::from_cli(cli)?,
    ))
  }

  pub fn get_next_packet(&mut self, dest: &mut [u8]) {
    debug_assert_eq!(dest.len(), self.packet_size);
    let curr_millis = millis_since_start(self.zero_instant);
    let ph = PacketHeader {
      index: self.i,
      millis: curr_millis,
    };
    let ptr = (self.i % PACKET_CYCLE) as usize * self.packet_size;
    dest.copy_from_slice(&self.data_buf[ptr..ptr + self.packet_size]);
    dest[0..PACKET_HEAD_SIZE].copy_from_slice(unsafe {
      std::slice::from_raw_parts(&ph as *const PacketHeader as *const u8, PACKET_HEAD_SIZE)
    });
    self.i += 1;

    if curr_millis >= self.last_timestep_millis + TIME_STEP_MILLIS {
      let lt = self.last_timestep_millis;
      let count = self.i - self.i_last_timestep;
      self.last_timestep_millis = curr_millis;
      self.i_last_timestep = self.i;
      if let Some(ref mut sf) = self.stats_file {
        sf.write(lt, lt + TIME_STEP_MILLIS, count)
          .expect("wrtie to stats file failed.");
      }
    }
  }

  pub fn verify_recv_packet(&mut self, pkt: &[u8]) -> bool {
    if pkt.len() != self.packet_size {
      return false;
    }
    let mut head_buf = [0u8; PACKET_HEAD_SIZE];
    head_buf.copy_from_slice(&pkt[0..PACKET_HEAD_SIZE]);
    let head: PacketHeader = unsafe { std::mem::transmute(head_buf) };
    let ptr: usize = (head.index % PACKET_CYCLE) as usize * self.packet_size;
    let rand_buf: &[u8] = &self.data_buf[ptr..ptr + self.packet_size];
    if rand_buf[PACKET_HEAD_SIZE..] == pkt[PACKET_HEAD_SIZE..] {
      self.recv_stats.recv(head.millis);
      true
    } else {
      false
    }
  }
}

pub(crate) struct RecvStats {
  backlog: Vec<u64>,
  backlog_start_millis: u64,
  stats_file: Option<StatsFile>,
}

impl RecvStats {
  pub fn new(max_backlog: usize) -> Self {
    RecvStats {
      backlog: vec![0u64; max_backlog],
      backlog_start_millis: 0u64,
      stats_file: None,
    }
  }

  pub fn set_stats_file(&mut self, stats_file: StatsFile) {
    self.stats_file = Some(stats_file);
  }

  pub fn recv(&mut self, pkt_millis: u64) {
    if pkt_millis < self.backlog_start_millis {
      return;
    }
    let i = (pkt_millis - self.backlog_start_millis) / TIME_STEP_MILLIS;
    let bklen = self.backlog.len() as u64;
    if i >= bklen {
      let need_to_evict = i - bklen + 1;
      for e in 0..need_to_evict {
        if e >= bklen {
          break;
        }
        self.handle_evict(
          self.backlog_start_millis + e * TIME_STEP_MILLIS,
          self.backlog[e as usize],
        )
      }
      self.backlog.drain(0..need_to_evict.min(bklen) as usize);
      self.backlog.resize(bklen as usize, 0u64);
      self.backlog_start_millis += need_to_evict * TIME_STEP_MILLIS;
    }
    let i = (pkt_millis - self.backlog_start_millis) / TIME_STEP_MILLIS;
    self.backlog[i as usize] += 1;
  }

  /// One time step
  pub fn handle_evict(&mut self, start_millis: u64, count: u64) {
    if let Some(ref mut stats_file) = self.stats_file {
      stats_file
        .write(start_millis, start_millis + TIME_STEP_MILLIS, count)
        .expect("Unable to write stats.");
    }
  }
}
