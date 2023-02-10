//! This module implements a multi-threaded, simple statistics aggregator, which
//! allows us to track statistics like latency and drop rate across time.
//!
//! It works by dividing the timeline into small, fixed-size steps, and keeping
//! aggregated information for each step. There is also automatic eviction of
//! old steps to keep memory usage bounded. Whenever a step access is attempted
//! which goes over the range of steps we currently have, we evict all steps
//! older than a certain threshold.
//!
//! It allows inserting new values into any steps that are still in memory, and
//! supports exporting the aggregated information as a CSV file.
//!
//! The unit of the time values provided to this module can be arbitrary.

use std::{
  collections::VecDeque,
  sync::{atomic::AtomicU64, RwLock},
};

pub struct StatsAggregator {
  /// Duration of each step.
  step_size: u64,

  /// Number of steps to keep in memory.
  max_steps: usize,

  /// Number of steps to keep in memory after eviction. This is computed from
  /// the requested time threshold.
  eviction_steps_to_keep: usize,

  /// The buffer
  locked_part: RwLock<LockedPart>,

  stats_writer: Option<Box<dyn Fn(u64, &Stats)>>,
}

#[derive(Debug, Default)]
struct LockedPart {
  /// The index of the first step stored in the steps buffer.
  first_step_idx: usize,

  /// The steps buffer.
  steps_buf: VecDeque<Stats>,
}

/// Aggregated statistics for a single step.
#[derive(Debug, Default)]
pub struct Stats {
  /// Number of packets sent in this step.
  pub tx_packets: AtomicU64,

  /// Number of packets received in this step.
  pub rx_packets: AtomicU64,

  /// Number of packets received that was sent in this step.  This is used to
  /// calculate the drop rate.
  pub rx_packets_sent_here: AtomicU64,

  /// Total latency of all packets that were *sent* in this step.
  pub total_latency_sent_here: AtomicU64,
}

impl StatsAggregator {
  pub fn new(
    step_size: u64,
    max_steps: usize,
    evict_threshold: u64,
    stats_writer: Option<Box<dyn Fn(u64, &Stats)>>,
  ) -> Self {
    Self {
      step_size,
      max_steps,
      eviction_steps_to_keep: (evict_threshold / step_size + 1) as usize,
      locked_part: RwLock::default(),
      stats_writer,
    }
  }

  pub fn access_step(&self, time: u64, f: impl FnOnce(&Stats)) -> bool {
    let step: usize = (time / self.step_size).try_into().unwrap();
    let read_lock = self.locked_part.read().unwrap();
    if step < read_lock.first_step_idx {
      return false;
    }
    let step_buf_idx = step - read_lock.first_step_idx;
    if step_buf_idx >= read_lock.steps_buf.len() {
      drop(read_lock);
      let mut write_lock = self.locked_part.write().unwrap();
      unimplemented!("evict based on threshold");
    } else {
      f(&read_lock.steps_buf[step_buf_idx]);
      true
    }
  }

  /// Single-threaded version of get_step. Avoids locking.
  pub fn get_step_mut(&mut self, time: u64) -> Option<&mut Stats> {
    let step: usize = (time / self.step_size).try_into().unwrap();
    let lock = self.locked_part.get_mut().unwrap();
    if step < lock.first_step_idx {
      return None;
    }
    let step_buf_idx = step - lock.first_step_idx;
    if step_buf_idx >= lock.steps_buf.len() {
      unimplemented!("evict based on threshold")
    } else {
      Some(&mut lock.steps_buf[step_buf_idx])
    }
  }
}

impl LockedPart {
  fn evict_first(&mut self, idx: usize) {
    debug_assert!(idx <= self.steps_buf.len());
    self.first_step_idx += idx;
    self.steps_buf.drain(..idx);
  }
}
