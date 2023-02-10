//! This module contains various actual implementations of packet
//! sending/receiving.

mod common;
mod sys;
pub mod syscall_sendrecv;
pub mod syscall_echo;
pub mod iouring_sendrecv;
pub mod iouring_echo;
