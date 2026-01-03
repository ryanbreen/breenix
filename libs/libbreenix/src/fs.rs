//! Filesystem syscall wrappers
//!
//! Provides safe wrappers around file-related system calls:
//! - open: Open a file and get a file descriptor
//! - read: Read data from a file descriptor (file-specific variant)
//! - fstat: Get file metadata
//! - lseek: Reposition file offset
//! - close: Close a file descriptor (re-exported from io)

use crate::errno::Errno;
use crate::syscall::{nr, raw};
use crate::types::Fd;

// Re-export close from io module for convenience
pub use crate::io::close;

/// Open flags (POSIX compatible)
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
/// O_DIRECTORY - must be a directory
pub const O_DIRECTORY: u32 = 0x10000;

/// Seek whence values
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

/// File type mode constants (for st_mode interpretation)
pub const S_IFMT: u32 = 0o170000;   // File type mask
pub const S_IFSOCK: u32 = 0o140000; // Socket
pub const S_IFLNK: u32 = 0o120000;  // Symbolic link
pub const S_IFREG: u32 = 0o100000;  // Regular file
pub const S_IFBLK: u32 = 0o060000;  // Block device
pub const S_IFDIR: u32 = 0o040000;  // Directory
pub const S_IFCHR: u32 = 0o020000;  // Character device
pub const S_IFIFO: u32 = 0o010000;  // FIFO (pipe)

/// stat structure (Linux x86_64 compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    _pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    _reserved: [i64; 3],
}

impl Stat {
    /// Create a zeroed Stat structure
    pub const fn new() -> Self {
        Self {
            st_dev: 0,
            st_ino: 0,
            st_nlink: 0,
            st_mode: 0,
            st_uid: 0,
            st_gid: 0,
            _pad0: 0,
            st_rdev: 0,
            st_size: 0,
            st_blksize: 0,
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
            _reserved: [0; 3],
        }
    }

    /// Check if this is a regular file
    pub fn is_file(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFREG
    }

    /// Check if this is a directory
    pub fn is_dir(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFDIR
    }

    /// Check if this is a symbolic link
    pub fn is_symlink(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFLNK
    }
}

/// Open a file and return a file descriptor.
///
/// # Arguments
/// * `path` - Path to the file (null-terminated)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
///
/// # Returns
/// File descriptor on success, Errno on failure.
///
/// # Example
/// ```ignore
/// let fd = open("/hello.txt\0", O_RDONLY)?;
/// ```
#[inline]
pub fn open(path: &str, flags: u32) -> Result<Fd, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::OPEN,
            path.as_ptr() as u64,
            flags as u64,
            0, // mode (not used for O_RDONLY)
        ) as i64
    };
    Errno::from_syscall(ret)
}

/// Open a file with mode (for O_CREAT).
///
/// # Arguments
/// * `path` - Path to the file (null-terminated)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, etc.)
/// * `mode` - File permissions if creating (e.g., 0o644)
///
/// # Returns
/// File descriptor on success, Errno on failure.
#[inline]
pub fn open_with_mode(path: &str, flags: u32, mode: u32) -> Result<Fd, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::OPEN,
            path.as_ptr() as u64,
            flags as u64,
            mode as u64,
        ) as i64
    };
    Errno::from_syscall(ret)
}

/// Read from a file descriptor into a buffer.
///
/// This uses the SYS_READ syscall (syscall 0) which is already implemented
/// in the io module, but we provide this here for consistency with the fs module API.
///
/// # Arguments
/// * `fd` - File descriptor to read from
/// * `buf` - Buffer to read data into
///
/// # Returns
/// Number of bytes read on success, Errno on failure.
#[inline]
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::READ,
            fd,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as i64
    };
    Errno::from_syscall(ret).map(|n| n as usize)
}

/// Get file status (fstat).
///
/// # Arguments
/// * `fd` - File descriptor
///
/// # Returns
/// Stat structure on success, Errno on failure.
#[inline]
pub fn fstat(fd: Fd) -> Result<Stat, Errno> {
    let mut stat = Stat::new();
    let ret = unsafe {
        raw::syscall2(
            nr::FSTAT,
            fd,
            &mut stat as *mut Stat as u64,
        ) as i64
    };
    Errno::from_syscall(ret)?;
    Ok(stat)
}

/// Reposition read/write file offset.
///
/// # Arguments
/// * `fd` - File descriptor
/// * `offset` - Offset value
/// * `whence` - SEEK_SET (0), SEEK_CUR (1), or SEEK_END (2)
///
/// # Returns
/// New file position on success, Errno on failure.
#[inline]
pub fn lseek(fd: Fd, offset: i64, whence: i32) -> Result<u64, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::LSEEK,
            fd,
            offset as u64,
            whence as u64,
        ) as i64
    };
    Errno::from_syscall(ret)
}

// Directory entry file type constants (d_type values)
/// Unknown file type
pub const DT_UNKNOWN: u8 = 0;
/// FIFO (named pipe)
pub const DT_FIFO: u8 = 1;
/// Character device
pub const DT_CHR: u8 = 2;
/// Directory
pub const DT_DIR: u8 = 4;
/// Block device
pub const DT_BLK: u8 = 6;
/// Regular file
pub const DT_REG: u8 = 8;
/// Symbolic link
pub const DT_LNK: u8 = 10;
/// Socket
pub const DT_SOCK: u8 = 12;

/// Linux dirent64 structure for getdents64 syscall.
///
/// This is a variable-length structure. The d_name field is variable
/// length and null-terminated. d_reclen is the total size including
/// padding for 8-byte alignment.
///
/// Memory layout:
/// - offset 0: d_ino (u64) - inode number
/// - offset 8: d_off (i64) - offset to next entry
/// - offset 16: d_reclen (u16) - length of this entry
/// - offset 18: d_type (u8) - file type
/// - offset 19: d_name[...] - null-terminated filename
#[repr(C)]
pub struct Dirent64 {
    /// Inode number
    pub d_ino: u64,
    /// Offset to next dirent (position cookie)
    pub d_off: i64,
    /// Length of this dirent
    pub d_reclen: u16,
    /// File type (DT_*)
    pub d_type: u8,
    // d_name follows (variable length, null-terminated)
}

impl Dirent64 {
    /// Get the name of this directory entry.
    ///
    /// # Safety
    /// The caller must ensure that the Dirent64 is part of a valid
    /// buffer returned by getdents64, and that the d_reclen field
    /// is valid.
    pub unsafe fn name(&self) -> &[u8] {
        let name_ptr = (self as *const Self as *const u8).add(19);
        // Find null terminator
        let mut len = 0;
        while *name_ptr.add(len) != 0 {
            len += 1;
        }
        core::slice::from_raw_parts(name_ptr, len)
    }

    /// Get the name as a str if valid UTF-8.
    ///
    /// # Safety
    /// Same requirements as `name()`.
    pub unsafe fn name_str(&self) -> Option<&str> {
        core::str::from_utf8(self.name()).ok()
    }
}

/// Get directory entries (getdents64).
///
/// Reads directory entries from an open directory file descriptor
/// into the provided buffer.
///
/// # Arguments
/// * `fd` - File descriptor for an open directory
/// * `buf` - Buffer to receive directory entries
///
/// # Returns
/// * On success: Number of bytes read (0 means end of directory)
/// * On error: Errno
///
/// # Example
/// ```ignore
/// let fd = open("/\0", O_RDONLY | O_DIRECTORY)?;
/// let mut buf = [0u8; 1024];
/// loop {
///     let n = getdents64(fd, &mut buf)?;
///     if n == 0 { break; }
///     // Process entries in buf[..n]
/// }
/// close(fd);
/// ```
#[inline]
pub fn getdents64(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::GETDENTS64,
            fd,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as i64
    };
    Errno::from_syscall(ret).map(|n| n as usize)
}

/// Iterator over directory entries in a getdents64 buffer.
pub struct DirentIter<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> DirentIter<'a> {
    /// Create a new iterator over directory entries.
    ///
    /// # Arguments
    /// * `buf` - Buffer containing getdents64 output
    /// * `len` - Number of valid bytes in the buffer
    pub fn new(buf: &'a [u8], len: usize) -> Self {
        Self {
            buf: &buf[..len],
            offset: 0,
        }
    }
}

impl<'a> Iterator for DirentIter<'a> {
    type Item = &'a Dirent64;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }

        // Check if we have enough bytes for the header
        if self.offset + 19 > self.buf.len() {
            return None;
        }

        let entry_ptr = self.buf[self.offset..].as_ptr() as *const Dirent64;
        let entry = unsafe { &*entry_ptr };

        // Validate d_reclen
        let reclen = entry.d_reclen as usize;
        if reclen == 0 || self.offset + reclen > self.buf.len() {
            return None;
        }

        self.offset += reclen;
        Some(entry)
    }
}
