//! This module contains various actual implementations of packet
//! sending/receiving.

mod common;
pub mod syscall_send;
pub mod iouring_send;
