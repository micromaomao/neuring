use std::time::{SystemTime, UNIX_EPOCH};

use rand::rngs::SmallRng;
use rand::{RngCore, SeedableRng};
use std::mem;

use crate::Cli;

pub(crate) struct PacketGenerator {
  packet_size: usize,
  seed: u64,
  data_buf: Vec<u8>,
  i: usize,
}

#[repr(C)]
pub(crate) struct PacketHeader {
  pub index: u64,
  pub timestamp_millis: u64,
}

pub const PACKET_HEAD_SIZE: usize = mem::size_of::<PacketHeader>();
pub const PACKET_CYCLE: usize = 102400;

impl PacketGenerator {
  pub fn init(packet_size: usize, seed: u64) -> Self {
    assert!(packet_size >= PACKET_HEAD_SIZE);
    let mut data_buf = vec![0u8; PACKET_CYCLE * packet_size];
    if packet_size > PACKET_HEAD_SIZE {
      let mut rng = SmallRng::seed_from_u64(seed);
      rng.fill_bytes(&mut data_buf);
    }
    PacketGenerator {
      packet_size,
      seed,
      data_buf,
      i: 0,
    }
  }

  pub fn init_from_cli(cli: &Cli) -> Self {
    let packet_size = cli.packet_size as usize;
    Self::init(packet_size, 0x39016c0e906374f9)
  }

  pub fn get_next_packet(&mut self, dest: &mut [u8]) {
    debug_assert_eq!(dest.len(), self.packet_size);
    let ph = PacketHeader {
      index: self.i as u64,
      timestamp_millis: SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64,
    };
    let ptr = (self.i % PACKET_CYCLE) * self.packet_size;
    dest.copy_from_slice(&self.data_buf[ptr..ptr + self.packet_size]);
    dest[0..PACKET_HEAD_SIZE].copy_from_slice(unsafe {
      std::slice::from_raw_parts(&ph as *const PacketHeader as *const u8, PACKET_HEAD_SIZE)
    });
    self.i += 1;
  }

  pub fn get_next_index(&self) -> u64 {
    self.i as u64
  }

  pub fn verify_recv_packet(&self, pkt: &[u8]) -> bool {
    if pkt.len() != self.packet_size {
      return false;
    }
    let mut head_buf = [0u8; PACKET_HEAD_SIZE];
    head_buf.copy_from_slice(&pkt[0..PACKET_HEAD_SIZE]);
    let head: PacketHeader = unsafe { std::mem::transmute(head_buf) };
    let ptr: usize = (head.index as usize % PACKET_CYCLE) * self.packet_size;
    let rand_buf: &[u8] = &self.data_buf[ptr..ptr + self.packet_size];
    rand_buf[PACKET_HEAD_SIZE..] == pkt[PACKET_HEAD_SIZE..]
  }
}
