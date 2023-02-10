//! This module contains various actual implementations of packet
//! sending/receiving.

mod common;
pub mod syscall_sendrecv;
pub mod syscall_forward;
pub mod iouring_sendrecv;
pub mod iouring_forward;
