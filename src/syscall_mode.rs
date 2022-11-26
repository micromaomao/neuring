use std::io;

use crate::common::setup_soekct;
use crate::packetgen::PacketGenerator;
use crate::{errors::AppError, Cli};

pub(crate) fn syscall_mode(cli: &Cli) -> Result<(), AppError> {
  let packet_size = cli.packet_size as usize;
  let mut buf = vec![0u8; packet_size];
  let mut pkgen = PacketGenerator::init_from_cli(cli);
  match &cli.command {
    crate::Commands::Send { address } => {
      let sock_fd = setup_soekct(address, true)?;
      loop {
        pkgen.get_next_packet(&mut buf);
        unsafe { send(sock_fd, buf.as_slice()) }?;
        let nb_packets_sent = pkgen.get_next_index();
        if nb_packets_sent % 1000 == 0 {
          eprint!("\r\x1b[3KSent {nb_packets_sent} packets\r")
        }
      }
    }
    crate::Commands::Recv { address } => {
      let sock_fd = setup_soekct(address, false)?;
      let mut nb_received = 0usize;
      loop {
        let len = unsafe { recv(sock_fd, &mut buf) }?;
        if len != packet_size {
          continue;
        }
        if !pkgen.verify_recv_packet(&buf) {
          continue;
        }
        nb_received += 1;
        if nb_received % 1000 == 0 {
          eprint!("\r\x1b[3KReceived {nb_received} packets\r")
        }
      }
    }
  }
}

pub unsafe fn send(sock_fd: libc::c_int, packet_data: &[u8]) -> Result<(), AppError> {
  unsafe {
    let ret = libc::send(
      sock_fd,
      packet_data.as_ptr() as *const _,
      packet_data.len(),
      libc::MSG_CONFIRM | libc::MSG_NOSIGNAL,
    );
    if ret == -1 {
      let errno = *libc::__errno_location();
      if errno == libc::EMSGSIZE {
        return Err(AppError::PacketSizeTooLarge);
      }
      if errno == libc::ECONNREFUSED {
        return Ok(());
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
