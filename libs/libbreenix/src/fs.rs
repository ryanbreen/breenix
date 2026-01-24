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
/// O_NONBLOCK - non-blocking I/O (important for FIFOs)
pub const O_NONBLOCK: u32 = 0x800;
/// O_DIRECTORY - must be a directory
pub const O_DIRECTORY: u32 = 0x10000;

/// Access mode constants for access() syscall
pub const F_OK: u32 = 0;  // Test for existence
pub const R_OK: u32 = 4;  // Test for read permission
pub const W_OK: u32 = 2;  // Test for write permission
pub const X_OK: u32 = 1;  // Test for execute permission

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

/// Check user's permissions for a file.
///
/// # Arguments
/// * `path` - Path to the file (null-terminated string)
/// * `mode` - Access mode to check (F_OK, R_OK, W_OK, X_OK or combination)
///
/// # Returns
/// * `Ok(())` - Access is allowed
/// * `Err(errno)` - Access denied or file doesn't exist
///
/// # Example
/// ```ignore
/// // Check if file exists
/// access("/hello.txt\0", F_OK)?;
///
/// // Check if file is readable and writable
/// access("/hello.txt\0", R_OK | W_OK)?;
/// ```
#[inline]
pub fn access(path: &str, mode: u32) -> Result<(), Errno> {
    let ret = unsafe { raw::syscall2(nr::ACCESS, path.as_ptr() as u64, mode as u64) as i64 };
    Errno::from_syscall(ret).map(|_| ())
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

/// Write to a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to write to
/// * `buf` - Buffer containing data to write
///
/// # Returns
/// Number of bytes written on success, Errno on failure.
///
/// # Example
/// ```ignore
/// let fd = open_with_mode("/newfile.txt\0", O_WRONLY | O_CREAT, 0o644)?;
/// let n = write(fd, b"Hello, world!")?;
/// close(fd);
/// ```
#[inline]
pub fn write(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::WRITE,
            fd,
            buf.as_ptr() as u64,
            buf.len() as u64,
        ) as i64
    };
    Errno::from_syscall(ret).map(|n| n as usize)
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

/// Unlink (delete) a file.
///
/// Removes the directory entry for the specified pathname. If this was
/// the last link to the file and no processes have it open, the file
/// is deleted.
///
/// # Arguments
/// * `path` - Path to the file (null-terminated string)
///
/// # Returns
/// * `Ok(())` - File was successfully unlinked
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - File does not exist
/// * `EISDIR` - Path refers to a directory (use rmdir instead)
/// * `EACCES` - Permission denied
/// * `EIO` - I/O error
///
/// # Example
/// ```ignore
/// unlink("/tmp/myfile.txt\0")?;
/// ```
#[inline]
pub fn unlink(path: &str) -> Result<(), Errno> {
    let ret = unsafe { raw::syscall1(nr::UNLINK, path.as_ptr() as u64) as i64 };
    Errno::from_syscall(ret).map(|_| ())
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

/// Create a new directory.
///
/// Creates a new directory with the specified permissions.
///
/// # Arguments
/// * `path` - Path to the directory (null-terminated string)
/// * `mode` - Directory permissions (e.g., 0o755)
///
/// # Returns
/// * `Ok(())` - Directory was successfully created
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - Parent directory does not exist
/// * `EEXIST` - Directory already exists
/// * `ENOTDIR` - A component in the path is not a directory
/// * `ENOSPC` - No space left on device
///
/// # Example
/// ```ignore
/// mkdir("/tmp/newdir\0", 0o755)?;
/// ```
#[inline]
pub fn mkdir(path: &str, mode: u32) -> Result<(), Errno> {
    let ret = unsafe { raw::syscall2(nr::MKDIR, path.as_ptr() as u64, mode as u64) as i64 };
    Errno::from_syscall(ret).map(|_| ())
}

/// Remove an empty directory.
///
/// Removes the specified directory. The directory must be empty
/// (contain only "." and ".." entries).
///
/// # Arguments
/// * `path` - Path to the directory (null-terminated string)
///
/// # Returns
/// * `Ok(())` - Directory was successfully removed
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - Directory does not exist
/// * `ENOTDIR` - Path is not a directory
/// * `ENOTEMPTY` - Directory is not empty
/// * `EBUSY` - Directory is being used (e.g., is current directory)
///
/// # Example
/// ```ignore
/// rmdir("/tmp/olddir\0")?;
/// ```
#[inline]
pub fn rmdir(path: &str) -> Result<(), Errno> {
    let ret = unsafe { raw::syscall1(nr::RMDIR, path.as_ptr() as u64) as i64 };
    Errno::from_syscall(ret).map(|_| ())
}

/// Rename a file or directory.
///
/// Renames oldpath to newpath. If newpath already exists, it will be
/// atomically replaced (if it's a file). Works for both files and
/// directories, including cross-directory moves.
///
/// # Arguments
/// * `oldpath` - Current path (null-terminated string)
/// * `newpath` - New path (null-terminated string)
///
/// # Returns
/// * `Ok(())` - File/directory was successfully renamed
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - oldpath does not exist
/// * `EISDIR` - newpath is a directory but oldpath is not
/// * `ENOTDIR` - A component in path is not a directory
/// * `EEXIST` - newpath is a non-empty directory
/// * `EIO` - I/O error
///
/// # Example
/// ```ignore
/// rename("/tmp/oldname.txt\0", "/tmp/newname.txt\0")?;
/// ```
#[inline]
pub fn rename(oldpath: &str, newpath: &str) -> Result<(), Errno> {
    let ret = unsafe {
        raw::syscall2(nr::RENAME, oldpath.as_ptr() as u64, newpath.as_ptr() as u64) as i64
    };
    Errno::from_syscall(ret).map(|_| ())
}

/// Create a hard link to a file.
///
/// Creates a new hard link (directory entry) pointing to an existing file.
/// Both paths must be on the same filesystem. Hard links to directories
/// are not allowed.
///
/// # Arguments
/// * `oldpath` - Path to the existing file (null-terminated string)
/// * `newpath` - Path for the new link (null-terminated string)
///
/// # Returns
/// * `Ok(())` - Hard link was successfully created
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - oldpath does not exist
/// * `EEXIST` - newpath already exists
/// * `EPERM` - oldpath is a directory
/// * `ENOTDIR` - A component in path is not a directory
/// * `ENOSPC` - No space in target directory
/// * `EIO` - I/O error
///
/// # Example
/// ```ignore
/// // Create a hard link to an existing file
/// link("/original.txt\0", "/link_to_original.txt\0")?;
/// // Both paths now refer to the same file (same inode)
/// ```
#[inline]
pub fn link(oldpath: &str, newpath: &str) -> Result<(), Errno> {
    let ret = unsafe {
        raw::syscall2(nr::LINK, oldpath.as_ptr() as u64, newpath.as_ptr() as u64) as i64
    };
    Errno::from_syscall(ret).map(|_| ())
}

/// Create a symbolic link.
///
/// Creates a new symbolic link at linkpath pointing to target.
/// Unlike hard links, symbolic links can reference directories
/// and paths that don't exist yet.
///
/// # Arguments
/// * `target` - Path the symlink will point to (null-terminated string)
/// * `linkpath` - Path where the symlink will be created (null-terminated string)
///
/// # Returns
/// * `Ok(())` - Symbolic link was successfully created
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - A component of linkpath's parent directory does not exist
/// * `EEXIST` - linkpath already exists
/// * `ENOTDIR` - A component in the path is not a directory
/// * `ENOSPC` - No space to create the symlink
/// * `EIO` - I/O error
///
/// # Example
/// ```ignore
/// // Create a symlink to an existing file
/// symlink("/etc/passwd\0", "/tmp/passwd_link\0")?;
///
/// // Symlinks can point to paths that don't exist
/// symlink("/nonexistent/path\0", "/tmp/broken_link\0")?;
/// ```
#[inline]
pub fn symlink(target: &str, linkpath: &str) -> Result<(), Errno> {
    let ret = unsafe {
        raw::syscall2(nr::SYMLINK, target.as_ptr() as u64, linkpath.as_ptr() as u64) as i64
    };
    Errno::from_syscall(ret).map(|_| ())
}

/// Read the target of a symbolic link.
///
/// Reads the contents of the symbolic link (the path it points to)
/// and writes it to the provided buffer. The result is NOT null-terminated.
///
/// # Arguments
/// * `pathname` - Path to the symbolic link (null-terminated string)
/// * `buf` - Buffer to store the symlink target
///
/// # Returns
/// * `Ok(bytes_read)` - Number of bytes written to the buffer
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `ENOENT` - The symlink does not exist
/// * `EINVAL` - pathname is not a symbolic link
/// * `EFAULT` - Invalid buffer pointer
/// * `EIO` - I/O error
///
/// # Note
/// The buffer is NOT null-terminated. The returned length tells you
/// exactly how many bytes of the target were written.
///
/// # Example
/// ```ignore
/// let mut buf = [0u8; 256];
/// let len = readlink("/tmp/mylink\0", &mut buf)?;
/// let target = core::str::from_utf8(&buf[..len])?;
/// ```
#[inline]
pub fn readlink(pathname: &str, buf: &mut [u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(
            nr::READLINK,
            pathname.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as i64
    };
    Errno::from_syscall(ret).map(|n| n as usize)
}

/// Create a FIFO (named pipe)
///
/// Creates a special file that provides pipe-like IPC through a filesystem path.
/// FIFOs allow unrelated processes to communicate by opening the same path.
///
/// # Arguments
/// * `pathname` - Path where the FIFO should be created (null-terminated string)
/// * `mode` - Permission bits for the FIFO (e.g., 0o644)
///
/// # Returns
/// * `Ok(())` - FIFO created successfully
/// * `Err(errno)` - Error occurred
///
/// # Errors
/// * `EEXIST` - Path already exists
/// * `ENOENT` - Parent directory does not exist
/// * `ENOSPC` - No space left on device
///
/// # Example
/// ```ignore
/// // Create a FIFO at /tmp/myfifo
/// mkfifo("/tmp/myfifo\0", 0o644)?;
///
/// // Now other processes can open it for read/write
/// let fd = open("/tmp/myfifo\0", O_RDONLY)?;
/// ```
#[inline]
pub fn mkfifo(pathname: &str, mode: u32) -> Result<(), Errno> {
    // mkfifo is implemented via mknod with S_IFIFO mode
    let ret = unsafe {
        raw::syscall3(
            nr::MKNOD,
            pathname.as_ptr() as u64,
            (S_IFIFO | (mode & 0o777)) as u64,
            0, // dev number (unused for FIFOs)
        ) as i64
    };
    Errno::from_syscall(ret).map(|_| ())
}
