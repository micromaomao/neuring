// TODO

unsafe fn recv(sock_fd: libc::c_int, recv_buf: &mut [u8]) -> Result<usize, AppError> {
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
