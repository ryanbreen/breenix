//! TTY Subsystem for Breenix
//!
//! Provides POSIX-compliant terminal semantics including:
//! - Line discipline (canonical vs raw mode)
//! - Signal generation (SIGINT, SIGTSTP, etc.)
//! - Terminal attributes (termios)
//! - Echo and line editing

pub mod ctty;
pub mod driver;
pub mod ioctl;
pub mod jobctl;
pub mod line_discipline;
pub mod pty;
pub mod slave_read;
pub mod termios;

// Re-export for external use
// Allow unused - these are public API re-exports for Phase 4+ syscalls and ioctls
#[allow(unused_imports)]
pub use line_discipline::LineDiscipline;
#[allow(unused_imports)]
pub use termios::Termios;

// Re-export driver functions for external use
// Allow unused - these are public API re-exports for keyboard interrupt integration
#[allow(unused_imports)]
pub use driver::{console, push_char, push_char_nonblock, write_output, TtyDevice};

/// Linux's read(2) moves terminal input to the caller this many bytes at a
/// time (tty_io.c, iterate_tty_read).
const READ_CHUNK: usize = 64;

/// Copy up to `n` bytes, which `take` removes from a terminal queue, to the
/// user address `ptr`, as Linux's read(2) does. An address outside user
/// space fails first, with nothing taken (vfs_read's access_ok). Then each
/// chunk leaves the queue before it is copied (n_tty advances read_tail
/// before tty_read copies out), so a chunk whose copy faults is consumed and
/// the read ends with the bytes already copied, or EFAULT when there are none.
pub(crate) fn read_to_user(ptr: u64, n: usize, mut take: impl FnMut(&mut [u8]) -> usize) -> Result<usize, i32> {
    use crate::syscall::errno::EFAULT;
    if n > 0 && (ptr == 0 || !crate::memory::layout::is_valid_user_range(ptr, n)) {
        return Err(EFAULT);
    }
    let mut chunk = [0u8; READ_CHUNK];
    let mut done = 0;
    while done < n {
        let got = take(&mut chunk[..(n - done).min(READ_CHUNK)]);
        if got == 0 {
            break;
        }
        if crate::syscall::userptr::write_user_bytes(ptr + done as u64, chunk.as_ptr(), got).is_err() {
            return if done > 0 { Ok(done) } else { Err(EFAULT) };
        }
        done += got;
    }
    Ok(done)
}

/// Initialize the TTY subsystem
///
/// This creates the console TTY device (TTY 0) and sets up
/// the infrastructure for terminal I/O.
pub fn init() {
    driver::init_console();
    pty::init();
    log::info!("TTY subsystem initialized");
}
