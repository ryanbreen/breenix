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
//! - **Stage 5**: Full POSIX libc port (musl or custom)
//!
//! # Usage
//!
//! ```rust,ignore
//! #![no_std]
//! #![no_main]
//!
//! use libbreenix::io::stdout;
//! use libbreenix::process::exit;
//!
//! #[no_mangle]
//! pub extern "C" fn _start() -> ! {
//!     stdout().write(b"Hello from Breenix!\n");
//!     exit(0);
//! }
//! ```

#![no_std]

// Re-export all public APIs
pub use errno::Errno;
pub use syscall::raw;
pub use types::*;

pub mod errno;
pub mod io;
pub mod memory;
pub mod process;
pub mod syscall;
pub mod time;
pub mod types;
