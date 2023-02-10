//! Some utility functions shared between implementations, like setting up
//! socket.

use std::{io, net::ToSocketAddrs, time::Duration};

use crate::errors::AppError;
use std::mem;

pub type GetSockaddrRes = (i32, libc::sockaddr, libc::socklen_t);

/// Use the libc API for address resolution to get the sockaddr struct, to be
/// used to connect/bind sockets. Returns (af, sockaddr, sockaddr_len).
pub fn get_sockaddr(addr: &str) -> Result<GetSockaddrRes, AppError> {
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
    Ok((af, sock_addr, addr_len))
  }
}

/// Connect a UDP socket to the given address, and return the socket fd.
pub fn setup_send_socket(dest_addr: &GetSockaddrRes) -> Result<libc::c_int, AppError> {
  let (af, ref sock_addr, addr_len) = *dest_addr;
  let sock_fd = unsafe { libc::socket(af, libc::SOCK_DGRAM, 0) };
  if sock_fd == -1 {
    return Err(AppError::IOError("socket", io::Error::last_os_error()));
  }
  unsafe {
    while libc::connect(sock_fd, sock_addr, addr_len) == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EAGAIN {
        std::thread::sleep(Duration::from_millis(100));
        continue;
      }
      return Err(AppError::IOError("connect", io::Error::last_os_error()));
    }
  }
  Ok(sock_fd)
}

/// Bind a UDP socket to the given address, and return the socket fd.
pub fn setup_recv_socket(listen_addr: &GetSockaddrRes) -> Result<libc::c_int, AppError> {
  let (af, ref sock_addr, addr_len) = *listen_addr;
  let sock_fd = unsafe { libc::socket(af, libc::SOCK_DGRAM, 0) };
  if sock_fd == -1 {
    return Err(AppError::IOError("socket", io::Error::last_os_error()));
  }
  let val: libc::c_int = 1;
  unsafe {
    if libc::setsockopt(
      sock_fd,
      libc::SOL_SOCKET,
      libc::SO_REUSEPORT,
      &val as *const _ as *const libc::c_void,
      mem::size_of_val(&val) as libc::socklen_t,
    ) == -1
    {
      return Err(AppError::IOError("setsockopt", io::Error::last_os_error()));
    }
    if libc::bind(sock_fd, sock_addr, addr_len) == -1 {
      return Err(AppError::IOError("bind", io::Error::last_os_error()));
    }
  }
  Ok(sock_fd)
}

/// Get the local port used by the socket.
pub unsafe fn get_socket_local_port(fd: libc::c_int) -> Result<libc::in_port_t, AppError> {
  // We can use sockaddr_in here since the only possibility is in or in6, and
  // in6 has the port number in the same place.
  let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
  let mut len = mem::size_of_val(&addr) as libc::socklen_t;
  let res = unsafe { libc::getsockname(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut len) };
  if res == -1 {
    return Err(AppError::IOError("getsockname", io::Error::last_os_error()));
  }
  Ok(addr.sin_port)
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
