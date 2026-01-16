//! PTY (Pseudo-Terminal) syscall wrappers
//!
//! Provides userspace API for creating and using pseudo-terminals.

use crate::syscall::raw;

/// Open flags
pub const O_RDWR: i32 = 0x02;
pub const O_NOCTTY: i32 = 0x100;
pub const O_CLOEXEC: i32 = 0x80000;

// PTY syscall numbers (will be assigned in kernel)
// For now, use high numbers that won't conflict
pub const SYS_POSIX_OPENPT: u64 = 400;
pub const SYS_GRANTPT: u64 = 401;
pub const SYS_UNLOCKPT: u64 = 402;
pub const SYS_PTSNAME: u64 = 403;

/// Open a new PTY master device
///
/// # Arguments
/// * `flags` - O_RDWR | O_NOCTTY | O_CLOEXEC
///
/// # Returns
/// * Ok(fd) - File descriptor for PTY master
/// * Err(errno) - Error code
pub fn posix_openpt(flags: i32) -> Result<i32, i32> {
    let result = unsafe { raw::syscall1(SYS_POSIX_OPENPT, flags as u64) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(result as i32)
    }
}

/// Grant access to slave PTY
///
/// This changes ownership and permissions of the slave device.
/// In Breenix, this is currently a no-op that validates the fd.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
///
/// # Returns
/// * Ok(()) - Success
/// * Err(errno) - Error (ENOTTY if not a PTY master)
pub fn grantpt(fd: i32) -> Result<(), i32> {
    let result = unsafe { raw::syscall1(SYS_GRANTPT, fd as u64) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(())
    }
}

/// Unlock the slave PTY for opening
///
/// After this call, the slave device can be opened.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
///
/// # Returns
/// * Ok(()) - Success
/// * Err(errno) - Error (ENOTTY if not a PTY master)
pub fn unlockpt(fd: i32) -> Result<(), i32> {
    let result = unsafe { raw::syscall1(SYS_UNLOCKPT, fd as u64) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(())
    }
}

/// Get the path to the slave PTY device
///
/// # Arguments
/// * `fd` - PTY master file descriptor
/// * `buf` - Buffer to store the path
///
/// # Returns
/// * Ok(len) - Length of path (not including null terminator)
/// * Err(errno) - Error (ENOTTY if not a PTY master, ERANGE if buffer too small)
pub fn ptsname(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let result = unsafe {
        raw::syscall3(SYS_PTSNAME, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        // Find the actual length (up to null terminator)
        let mut len = 0;
        for &byte in buf.iter() {
            if byte == 0 {
                break;
            }
            len += 1;
        }
        Ok(len)
    }
}

/// Convenience function: create a PTY pair
///
/// Opens a PTY master, grants access, unlocks it, and returns
/// the master fd and slave path.
///
/// # Returns
/// * Ok((master_fd, slave_path)) - Master fd and path to slave device
/// * Err(errno) - Error code
pub fn openpty() -> Result<(i32, [u8; 32]), i32> {
    // Open PTY master
    let master_fd = posix_openpt(O_RDWR | O_NOCTTY)?;

    // Grant access to slave
    if let Err(e) = grantpt(master_fd) {
        // Close master on error
        crate::io::close(master_fd as u64);
        return Err(e);
    }

    // Unlock slave
    if let Err(e) = unlockpt(master_fd) {
        crate::io::close(master_fd as u64);
        return Err(e);
    }

    // Get slave path
    let mut path = [0u8; 32];
    if let Err(e) = ptsname(master_fd, &mut path) {
        crate::io::close(master_fd as u64);
        return Err(e);
    }

    Ok((master_fd, path))
}

/// Get slave path as a byte slice (convenience for opening)
pub fn slave_path_bytes(path: &[u8; 32]) -> &[u8] {
    let mut len = 0;
    for &byte in path.iter() {
        if byte == 0 {
            break;
        }
        len += 1;
    }
    &path[..len]
}
