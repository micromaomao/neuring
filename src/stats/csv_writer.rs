use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::errors::AppError;
use crate::stats::Stats;

/// A buffered CSV writer for stats.
///
/// This implementation flushes the buffer every second so that the user can see
/// the stats immediately.
struct CsvStatsFile {
  f: BufWriter<File>,
  last_flush: Instant,
}

impl CsvStatsFile {
  pub fn new(path: impl AsRef<Path>) -> Result<Self, AppError> {
    let mut f = File::create(path).map_err(|e| AppError::StatsFileError(e))?;
    write!(f, "time,tx_packets\n").map_err(|e| AppError::StatsFileError(e))?;
    Ok(Self {
      f: BufWriter::new(f),
      last_flush: Instant::now(),
    })
  }

  pub fn write(&mut self, time: u64, stat: &Stats) -> Result<(), AppError> {
    write!(
      self.f,
      "{},{}\n",
      time,
      stat.tx_packets.load(Ordering::Acquire)
    )
    .map_err(|e| AppError::StatsFileError(e))?;
    let now = Instant::now();
    if now - self.last_flush > Duration::from_secs(1) {
      self.f.flush().map_err(|e| AppError::StatsFileError(e))?;
      self.last_flush = now;
    }
    Ok(())
  }
}

pub fn get_writer(path: impl AsRef<Path>) -> Result<Box<dyn Fn(u64, &Stats)>, AppError> {
  let f = CsvStatsFile::new(path)?;
  let f = Mutex::new(f);
  Ok(Box::new(move |time, stat| {
    f.lock()
      .unwrap()
      .write(time, stat)
      .expect("failed to write stats")
  }))
}
