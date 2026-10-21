//! Breenix Userspace System Call Library
//!
//! This library provides safe(r) wrappers around Breenix kernel syscalls,
//! allowing userspace programs to interact with the kernel without writing
//! raw inline assembly.
//!
//! # Stages of Development
//!
//! This library is being developed in stages toward full POSIX libc compatibility:
//!
//! - **Stage 1 (Current)**: Raw syscall wrappers for Rust programs
//! - **Stage 2**: Higher-level abstractions (File, Process types)
//! - **Stage 3**: Memory allocator (malloc/free equivalent)
//! - **Stage 4**: C-compatible ABI for libc
//! - **Stage 5**: Full POSIX C library (libbreenix-libc, Breenix's own clean-room libc)
//!
//! # Usage
//!
//! This example uses the Breenix syscall ABI and is compile-checked only; run it inside Breenix.
//!
//! ```rust,no_run
//! use libbreenix::io::stdout;
//! use libbreenix::process::exit;
//!
//! fn main() {
//!     stdout().write(b"Hello from Breenix!\n");
//!     exit(0);
//! }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

// Re-export all public APIs
pub use errno::Errno;
pub use syscall::raw;
pub use types::*;

pub mod accounts;
pub mod argv;
pub mod audio;
pub mod crypto;
pub mod dns;
pub mod errno;
pub mod error;
pub mod fs;
pub mod gpu;
pub mod graphics;
pub mod http;
pub mod io;
#[cfg(feature = "std")]
pub mod layout;
#[cfg(feature = "std")]
pub mod etc_reconcile;
pub mod memory;
pub mod process;
pub mod pty;
#[cfg(feature = "runtime")]
pub mod runtime;
#[cfg(feature = "std")]
pub mod services;
pub mod signal;
pub mod socket;
#[cfg(feature = "suite")]
pub mod suite;
pub mod synth;
pub mod syscall;
pub mod termios;
#[cfg(feature = "std")]
pub mod ssh;
#[cfg(feature = "std")]
pub mod tls;
pub mod time;
pub mod types;
#[cfg(feature = "std")]
pub mod x509;

// Re-export commonly used signal functions
pub use signal::{
    alarm, getitimer, kill, setitimer, sigaction, sigaltstack, sigpending, sigprocmask,
    sigsuspend, Itimerval, Sigaction, StackT, Timeval,
};
