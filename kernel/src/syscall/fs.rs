//! Filesystem-related syscalls
//!
//! Implements: open, lseek, fstat

use crate::ipc::fd::FdKind;
use super::SyscallResult;

/// Open flags (POSIX compatible)
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_RDONLY: u32 = 0;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_WRONLY: u32 = 1;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_RDWR: u32 = 2;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_CREAT: u32 = 0x40;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_EXCL: u32 = 0x80;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_TRUNC: u32 = 0x200;
#[allow(dead_code)] // Will be used when open() is fully implemented
pub const O_APPEND: u32 = 0x400;

/// Seek whence values
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

/// File type mode constants (POSIX S_IFMT values)
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFMT: u32 = 0o170000;   // File type mask
pub const S_IFSOCK: u32 = 0o140000; // Socket
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFLNK: u32 = 0o120000;  // Symbolic link
pub const S_IFREG: u32 = 0o100000;  // Regular file
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFBLK: u32 = 0o060000;  // Block device
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFDIR: u32 = 0o040000;  // Directory
pub const S_IFCHR: u32 = 0o020000;  // Character device
pub const S_IFIFO: u32 = 0o010000;  // FIFO (pipe)

/// stat structure (Linux x86_64 compatible)
#[repr(C)]
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
    pub fn zeroed() -> Self {
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
}

/// sys_open - Open a file
///
/// # Arguments
/// * `pathname` - Path to the file (userspace pointer)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
/// * `mode` - File creation mode (if O_CREAT)
///
/// # Returns
/// File descriptor on success, negative errno on failure
pub fn sys_open(pathname: u64, flags: u32, _mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EISDIR, EMFILE, ENOENT};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2::{self, FileType as Ext2FileType};
    use crate::ipc::fd::RegularFile;
    use alloc::sync::Arc;
    use spin::Mutex;

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_open: path={:?}, flags={:#x}", path, flags);

    // Check if ext2 root filesystem is mounted
    let fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => {
            log::error!("sys_open: ext2 root filesystem not mounted");
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Resolve the path to an inode number
    let inode_num = match fs.resolve_path(&path) {
        Ok(ino) => ino,
        Err(e) => {
            // Map error to appropriate errno
            log::debug!("sys_open: path resolution failed: {}", e);
            if e.contains("not found") {
                return SyscallResult::Err(ENOENT as u64);
            } else if e.contains("Not a directory") {
                return SyscallResult::Err(20); // ENOTDIR
            } else {
                return SyscallResult::Err(5); // EIO
            }
        }
    };

    // Read the inode to check its type
    let inode = match fs.read_inode(inode_num) {
        Ok(ino) => ino,
        Err(_) => {
            log::error!("sys_open: failed to read inode {}", inode_num);
            return SyscallResult::Err(5); // EIO
        }
    };

    // Check if it's a directory (can't open directories with O_RDONLY/O_WRONLY/O_RDWR for read/write)
    // Note: O_RDONLY with O_DIRECTORY is allowed for reading directory entries
    let file_type = inode.file_type();
    if matches!(file_type, Ext2FileType::Directory) {
        // For now, we don't support opening directories
        // (would need O_DIRECTORY flag and getdents syscall)
        log::debug!("sys_open: {} is a directory", path);
        return SyscallResult::Err(EISDIR as u64);
    }

    // Check if it's a regular file
    if !matches!(file_type, Ext2FileType::Regular) {
        log::debug!("sys_open: {} is not a regular file (type: {:?})", path, file_type);
        return SyscallResult::Err(EACCES as u64);
    }

    // Create RegularFile structure
    let regular_file = RegularFile {
        inode_num: inode_num as u64,
        mount_id: fs.mount_id,
        position: 0,
        flags,
    };

    // Drop the filesystem lock before acquiring process lock
    drop(fs_guard);

    // Get current process and allocate fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_open: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_open: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor
    let fd_kind = FdKind::RegularFile(Arc::new(Mutex::new(regular_file)));
    match process.fd_table.alloc(fd_kind) {
        Ok(fd) => {
            log::info!("sys_open: opened {} as fd {} (inode {})", path, fd, inode_num);
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => {
            log::error!("sys_open: too many open files");
            SyscallResult::Err(EMFILE as u64)
        }
    }
}

/// sys_lseek - Reposition file offset
///
/// # Arguments
/// * `fd` - File descriptor
/// * `offset` - Offset value
/// * `whence` - SEEK_SET, SEEK_CUR, or SEEK_END
///
/// # Returns
/// New file position on success, negative errno on failure
pub fn sys_lseek(fd: i32, offset: i64, whence: i32) -> SyscallResult {
    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_lseek: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_lseek: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_lseek: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let fd_entry = match process.fd_table.get(fd) {
        Some(entry) => entry,
        None => return SyscallResult::Err(9), // EBADF
    };

    match &fd_entry.kind {
        FdKind::RegularFile(file) => {
            let mut file = file.lock();
            let new_pos = match whence {
                SEEK_SET => offset as u64,
                SEEK_CUR => (file.position as i64 + offset) as u64,
                SEEK_END => {
                    // Would need file size from inode
                    return SyscallResult::Err(22); // EINVAL for now
                }
                _ => return SyscallResult::Err(22), // EINVAL
            };
            file.position = new_pos;
            SyscallResult::Ok(new_pos)
        }
        _ => SyscallResult::Err(29), // ESPIPE - not seekable
    }
}

/// sys_fstat - Get file status
///
/// # Arguments
/// * `fd` - File descriptor
/// * `statbuf` - Pointer to stat structure (userspace)
///
/// # Returns
/// 0 on success, negative errno on failure
pub fn sys_fstat(fd: i32, statbuf: u64) -> SyscallResult {
    use super::errno::{EBADF, EFAULT};
    use core::ptr;

    // Validate statbuf pointer
    if statbuf == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_fstat: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_fstat: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_fstat: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let fd_entry = match process.fd_table.get(fd) {
        Some(entry) => entry,
        None => return SyscallResult::Err(EBADF as u64),
    };

    let mut stat = Stat::zeroed();
    stat.st_blksize = 4096; // Standard block size

    match &fd_entry.kind {
        FdKind::StdIo(io_fd) => {
            // stdin/stdout/stderr are character devices (TTY)
            stat.st_dev = 0;
            stat.st_ino = (*io_fd + 1) as u64; // Use fd+1 as pseudo-inode
            stat.st_mode = S_IFCHR | 0o666; // Character device with rw-rw-rw-
            stat.st_nlink = 1;
            stat.st_rdev = make_dev(5, *io_fd as u64); // Major 5 (TTY), minor = fd
        }
        FdKind::PipeRead(_) | FdKind::PipeWrite(_) => {
            // Pipes are FIFOs
            static PIPE_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(1000);
            stat.st_dev = 0;
            stat.st_ino = PIPE_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFIFO | 0o600; // FIFO with rw-------
            stat.st_nlink = 1;
            stat.st_size = 0; // Pipes don't have a seekable size
        }
        FdKind::UdpSocket(_) => {
            // Sockets
            static SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(2000);
            stat.st_dev = 0;
            stat.st_ino = SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755; // Socket with rwxr-xr-x
            stat.st_nlink = 1;
        }
        FdKind::RegularFile(file) => {
            let file_guard = file.lock();
            stat.st_dev = file_guard.mount_id as u64;
            stat.st_ino = file_guard.inode_num;
            stat.st_mode = S_IFREG | 0o644; // Regular file with rw-r--r-- (default)
            stat.st_nlink = 1;

            // Try to load inode metadata from ext2 filesystem
            if let Some(inode_stat) = load_ext2_inode_stat(file_guard.inode_num) {
                stat.st_mode = inode_stat.mode;
                stat.st_uid = inode_stat.uid;
                stat.st_gid = inode_stat.gid;
                stat.st_size = inode_stat.size;
                stat.st_nlink = inode_stat.nlink;
                stat.st_atime = inode_stat.atime;
                stat.st_mtime = inode_stat.mtime;
                stat.st_ctime = inode_stat.ctime;
                stat.st_blocks = inode_stat.blocks;
            }
        }
    }

    // Copy stat structure to userspace
    let statbuf_ptr = statbuf as *mut Stat;
    unsafe {
        ptr::write(statbuf_ptr, stat);
    }

    SyscallResult::Ok(0)
}

/// Helper to create device ID from major/minor numbers
fn make_dev(major: u64, minor: u64) -> u64 {
    (major << 8) | (minor & 0xff)
}

/// Inode metadata from ext2 filesystem
struct InodeStat {
    mode: u32,
    uid: u32,
    gid: u32,
    size: i64,
    nlink: u64,
    atime: i64,
    mtime: i64,
    ctime: i64,
    blocks: i64,
}

/// Load inode metadata from ext2 filesystem
///
/// Returns None if the ext2 filesystem is not available or inode cannot be read.
fn load_ext2_inode_stat(inode_num: u64) -> Option<InodeStat> {
    use crate::fs::ext2;

    // Get the mounted root filesystem
    let fs_guard = ext2::root_fs();
    let fs = fs_guard.as_ref()?;

    // Read the inode using the cached filesystem
    let inode = fs.read_inode(inode_num as u32).ok()?;

    // Extract metadata from packed struct using safe unaligned reads
    let mode = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mode)) };
    let uid = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_uid)) };
    let gid = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_gid)) };
    let links_count = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) };
    let atime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_atime)) };
    let mtime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mtime)) };
    let ctime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_ctime)) };
    let blocks = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_blocks)) };

    Some(InodeStat {
        mode: mode as u32, // ext2 mode is 16-bit, fstat expects 32-bit
        uid: uid as u32,
        gid: gid as u32,
        size: inode.size() as i64,
        nlink: links_count as u64,
        atime: atime as i64,
        mtime: mtime as i64,
        ctime: ctime as i64,
        blocks: blocks as i64,
    })
}
