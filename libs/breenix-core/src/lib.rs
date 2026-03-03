//! Breenix Core — portable kernel primitives shared across all targets.
//!
//! This crate extracts the hardware-independent portions of the Breenix kernel
//! so they can be reused across x86_64, aarch64, and wasm32 targets.
//!
//! Modules:
//! - `graphics` — framebuffer, font rendering, terminal emulation
//! - `tty` — POSIX termios, line discipline
//! - `block` — block device trait
//! - `fs` — VFS types, ext2, devfs, ramfs
//! - `ipc` — pipes, file descriptor table
//! - `signal` — POSIX signal constants and types
//! - `process` — portable process data model

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod block;
pub mod fs;
pub mod graphics;
pub mod ipc;
pub mod process;
pub mod signal;
pub mod tty;
