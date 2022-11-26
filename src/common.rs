use std::{
  fs::File,
  io::{self, BufWriter},
  net::ToSocketAddrs,
  time::{Duration, Instant},
};

use crate::{errors::AppError, Cli};
use std::io::Write;
use std::mem;

pub(crate) fn setup_soekct(addr: &str, is_send: bool) -> Result<libc::c_int, AppError> {
  let mut parsed_addrs = addr
    .to_socket_addrs()
    .map_err(|e| AppError::UnableToResolveNetAddr(addr.to_owned(), format!("{}", e)))?;
  let parsed_addr = parsed_addrs.next().ok_or_else(|| {
    AppError::UnableToResolveNetAddr(addr.to_owned(), "Host not found".to_owned())
  })?;
  if parsed_addrs.next().is_some() {
    eprintln!("Warn: {addr} resolved to multiple network addresses.");
  }
  let af = if parsed_addr.is_ipv4() {
    libc::AF_INET
  } else {
    libc::AF_INET6
  };
  unsafe {
    let sock_fd = libc::socket(af, libc::SOCK_DGRAM, 0);
    if sock_fd == -1 {
      return Err(AppError::IOError("socket", io::Error::last_os_error()));
    }
    let sock_addr: libc::sockaddr = match parsed_addr.ip() {
      std::net::IpAddr::V4(v4) => {
        assert_eq!(
          mem::size_of::<libc::sockaddr>(),
          mem::size_of::<libc::sockaddr_in>()
        );
        mem::transmute(libc::sockaddr_in {
          sin_family: af as _,
          sin_port: parsed_addr.port().to_be(),
          sin_addr: libc::in_addr {
            // octets is already in be. from_ne_bytes will preserve this in all platforms.
            s_addr: u32::from_ne_bytes(v4.octets()),
          },
          sin_zero: Default::default(),
        })
      }
      std::net::IpAddr::V6(v6) => {
        return Err(AppError::NotImplemented("ipv6"));
      }
    };
    let addr_len = mem::size_of_val(&sock_addr) as libc::socklen_t;
    if is_send {
      while libc::connect(sock_fd, &sock_addr, addr_len) == -1 {
        let errno = *libc::__errno_location();
        if errno == libc::EAGAIN {
          std::thread::sleep(Duration::from_millis(100));
          continue;
        }
        return Err(AppError::IOError("connect", io::Error::last_os_error()));
      }
      eprintln!("Connected to {parsed_addr}");
    } else {
      if libc::bind(sock_fd, &sock_addr, addr_len) == -1 {
        return Err(AppError::IOError("bind", io::Error::last_os_error()));
      }
    }
    Ok(sock_fd)
  }
}

pub(crate) fn getrandom(size: usize) -> Vec<u8> {
  let mut buf = vec![0u8; size];
  let mut written = 0usize;
  while written < size {
    unsafe {
      let ret = libc::getrandom(buf.as_mut_ptr().add(written) as *mut _, size - written, 0);
      if ret == -1 {
        panic!("getrandom failed: {}", io::Error::last_os_error());
      }
      assert!(ret > 0);
      written += ret as usize;
    }
  }
  buf
}

pub(crate) struct StatsFile {
  f: Option<BufWriter<File>>,
  last_flush: Instant,
}

impl StatsFile {
  pub fn from_cli(cli: &Cli) -> Result<Self, AppError> {
    let mut f = match cli.stats_file.as_ref().map(File::create) {
      Some(Ok(f)) => Some(BufWriter::new(f)),
      Some(Err(e)) => return Err(AppError::StatsFileError(e)),
      None => None,
    };
    if let Some(ref mut f) = f {
      write!(f, "start_ms,end_ms,count\n").map_err(|e| AppError::StatsFileError(e))?;
    }
    Ok(Self {
      f,
      last_flush: Instant::now(),
    })
  }

  pub fn write(&mut self, start_ms: u64, end_ms: u64, count: u64) -> Result<(), AppError> {
    if let Some(ref mut f) = self.f {
      write!(f, "{},{},{}\n", start_ms, end_ms, count).map_err(|e| AppError::StatsFileError(e))?;
      let now = Instant::now();
      if now - self.last_flush > Duration::from_secs(1) {
        f.flush().map_err(|e| AppError::StatsFileError(e))?;
        self.last_flush = now;
      }
    }
    Ok(())
  }
}
