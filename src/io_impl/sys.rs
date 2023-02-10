use crate::errors::AppError;

use std::{io, mem};

pub const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT;

pub unsafe fn send(sock_fd: libc::c_int, packet_data: &[u8]) -> Result<(), AppError> {
  unsafe {
    let ret = libc::send(
      sock_fd,
      packet_data.as_ptr() as *const _,
      packet_data.len(),
      SEND_FLAGS,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EMSGSIZE {
        return Err(AppError::PacketSizeTooLarge);
      }
      return Err(AppError::IOError("send", io::Error::last_os_error()));
    }
    #[cfg(debug_assertions)]
    if ret != packet_data.len() as _ {
      unreachable!("Did not send the full packet...?");
      // There is no "partial write" for UDP - if the message is larger than
      // the max length allowable it will return EMSGSIZE.
    }
    Ok(())
  }
}

pub unsafe fn sendmmsg(sock_fd: libc::c_int, pkts: &mut [libc::mmsghdr]) -> Result<(), AppError> {
  let mut rest = &mut pkts[..];
  while !rest.is_empty() {
    unsafe {
      let ret = libc::sendmmsg(
        sock_fd,
        rest.as_mut_ptr(),
        rest.len().try_into().unwrap(),
        SEND_FLAGS,
      );
      if ret == -1 {
        let errno = *libc::__errno_location();
        if errno == libc::EMSGSIZE {
          return Err(AppError::PacketSizeTooLarge);
        }
        return Err(AppError::IOError("sendmmsg", io::Error::last_os_error()));
      }
      rest = &mut rest[usize::try_from(ret).unwrap()..];
    }
  }
  Ok(())
}

pub unsafe fn recv(sock_fd: libc::c_int, recv_buf: &mut [u8]) -> Result<usize, AppError> {
  unsafe {
    let ret = libc::recv(
      sock_fd,
      recv_buf.as_mut_ptr() as *mut _,
      recv_buf.len(),
      libc::MSG_TRUNC,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
        return Ok(0);
      }
      return Err(AppError::IOError("recv", io::Error::last_os_error()));
    }
    debug_assert!(ret >= 0);
    Ok(ret as usize)
  }
}

pub struct RecvfromRes {
  pub recv_size: usize,
  pub src_addr: libc::sockaddr_storage,
  pub src_addr_len: libc::socklen_t,
}

pub unsafe fn recvfrom(sock_fd: libc::c_int, recv_buf: &mut [u8]) -> Result<RecvfromRes, AppError> {
  unsafe {
    let mut addr: libc::sockaddr_storage = std::mem::zeroed();
    let mut addr_len = mem::size_of_val(&addr) as libc::socklen_t;
    let ret = libc::recvfrom(
      sock_fd,
      recv_buf.as_mut_ptr() as *mut _,
      recv_buf.len(),
      0,
      &mut addr as *mut _ as *mut _,
      &mut addr_len,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      return Err(AppError::IOError("recvfrom", io::Error::last_os_error()));
    }
    Ok(RecvfromRes {
      recv_size: ret as usize,
      src_addr: addr,
      src_addr_len: addr_len,
    })
  }
}

pub unsafe fn sendto(
  sock_fd: libc::c_int,
  buf: &[u8],
  dst_addr: &libc::sockaddr_storage,
  dst_addr_len: libc::socklen_t,
) -> Result<(), AppError> {
  unsafe {
    let ret = libc::sendto(
      sock_fd,
      buf.as_ptr() as *const _,
      buf.len(),
      SEND_FLAGS,
      dst_addr as *const _ as *const _,
      dst_addr_len,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EMSGSIZE {
        return Err(AppError::PacketSizeTooLarge);
      }
      return Err(AppError::IOError("sendto", io::Error::last_os_error()));
    }
    #[cfg(debug_assertions)]
    if ret != buf.len() as _ {
      unreachable!("Did not send the full packet...?");
      // There is no "partial write" for UDP - if the message is larger than
      // the max length allowable it will return EMSGSIZE.
    }
    Ok(())
  }
}
