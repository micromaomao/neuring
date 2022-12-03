use std::io;
use std::mem::MaybeUninit;

use crate::common::setup_soekct;
use crate::packetgen::PacketGenerator;
use crate::{errors::AppError, Cli};

pub(crate) fn syscall_mode(cli: &Cli) -> Result<(), AppError> {
  let packet_size = cli.packet_size as usize;
  match &cli.command {
    &crate::Commands::Send {
      ref address,
      batch_size,
    } => {
      let sock_fd = setup_soekct(address, true)?;
      let mut pkgen = PacketGenerator::init_from_cli(true, cli)?;
      eprintln!("Ready to send to {address}.");
      if batch_size == 1 {
        loop {
          let buf = pkgen.make_next_packet();
          unsafe { send(sock_fd, buf) }?;
        }
      } else {
        let mut iovec_buf: Box<[MaybeUninit<libc::iovec>]> = Box::new_uninit_slice(batch_size);
        let mut mmsghdr_buf: Box<[MaybeUninit<libc::mmsghdr>]> = Box::new_uninit_slice(batch_size);
        loop {
          unsafe {
            for i in 0..batch_size {
              let pkt = pkgen.make_next_packet();
              iovec_buf[i] = MaybeUninit::new(libc::iovec {
                iov_base: pkt.as_ptr() as *const libc::c_void as *mut _,
                iov_len: pkt.len(),
              });
              mmsghdr_buf[i] = MaybeUninit::new(libc::mmsghdr {
                msg_hdr: libc::msghdr {
                  msg_name: std::ptr::null_mut(),
                  msg_namelen: 0,
                  msg_iov: iovec_buf[i].assume_init_ref() as *const libc::iovec as *mut _,
                  msg_iovlen: 1,
                  msg_control: std::ptr::null_mut(),
                  msg_controllen: 0,
                  msg_flags: 0,
                },
                msg_len: 0,
              });
            }
            sendmmsg(
              sock_fd,
              MaybeUninit::slice_assume_init_mut(&mut mmsghdr_buf[..]),
            )?;
          }
        }
      }
    }
    crate::Commands::Recv { address } => {
      let sock_fd = setup_soekct(address, false)?;
      let mut pkgen = PacketGenerator::init_from_cli(false, cli)?;
      let mut buf = vec![0u8; packet_size];
      eprintln!("Ready to receive on {address}.");
      loop {
        let len = unsafe { recv(sock_fd, &mut buf) }?;
        if len != packet_size {
          continue;
        }
        if !pkgen.verify_recv_packet(&buf) {
          continue;
        }
      }
    }
  }
}

const SEND_FLAGS: libc::c_int = libc::MSG_CONFIRM | libc::MSG_NOSIGNAL;

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

pub(crate) unsafe fn sendmmsg(
  sock_fd: libc::c_int,
  pkts: &mut [libc::mmsghdr],
) -> Result<(), AppError> {
  let mut rest = &mut pkts[..];
  while !rest.is_empty() {
    unsafe {
      let ret = libc::sendmmsg(sock_fd, rest.as_mut_ptr(), rest.len().try_into().unwrap(), SEND_FLAGS);
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

pub(crate) unsafe fn recv(sock_fd: libc::c_int, recv_buf: &mut [u8]) -> Result<usize, AppError> {
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
