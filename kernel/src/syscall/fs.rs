//! Filesystem-related syscalls
//!
//! Implements: open, lseek, fstat, getdents64

use crate::ipc::fd::FdKind;
use crate::arch_impl::traits::CpuOps;
use super::SyscallResult;

// Architecture-specific CPU type for interrupt control
#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::X86Cpu;
#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// Open flags (POSIX compatible)
pub const O_RDONLY: u32 = 0;
#[allow(dead_code)] // Part of POSIX open() API
pub const O_WRONLY: u32 = 1;
#[allow(dead_code)] // Part of POSIX open() API
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
#[allow(dead_code)] // Part of POSIX open() API
pub const O_APPEND: u32 = 0x400;
/// O_DIRECTORY - must be a directory
pub const O_DIRECTORY: u32 = 0x10000;

/// Linux dirent64 structure for getdents64 syscall
///
/// This is a variable-length structure. The d_name field is actually
/// variable-length and null-terminated. d_reclen is the total size
/// of the structure including padding for 8-byte alignment.
///
/// Note: We don't instantiate this struct directly; instead we write
/// the fields manually to user memory due to the variable-length d_name.
#[repr(C)]
#[allow(dead_code)] // Documentation struct - we write fields manually
pub struct LinuxDirent64 {
    /// Inode number
    pub d_ino: u64,
    /// Offset to next dirent (used as position cookie)
    pub d_off: i64,
    /// Length of this dirent (including d_name and padding)
    pub d_reclen: u16,
    /// File type (DT_*)
    pub d_type: u8,
    // d_name follows immediately after d_type (variable length, null-terminated)
}

/// Size of the fixed part of LinuxDirent64 (before d_name)
const DIRENT64_HEADER_SIZE: usize = 19; // 8 + 8 + 2 + 1 = 19 bytes

// File type constants for d_type field (Linux values)
/// Unknown file type
pub const DT_UNKNOWN: u8 = 0;
/// FIFO (named pipe)
#[allow(dead_code)] // Part of dirent API
pub const DT_FIFO: u8 = 1;
/// Character device
#[allow(dead_code)] // Part of dirent API
pub const DT_CHR: u8 = 2;
/// Directory
pub const DT_DIR: u8 = 4;
/// Block device
#[allow(dead_code)] // Part of dirent API
pub const DT_BLK: u8 = 6;
/// Regular file
pub const DT_REG: u8 = 8;
/// Symbolic link
#[allow(dead_code)] // Part of dirent API
pub const DT_LNK: u8 = 10;
/// Socket
#[allow(dead_code)] // Part of dirent API
pub const DT_SOCK: u8 = 12;

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

/// stat structure (Linux compatible)
/// Note: The layout is the same for x86_64 and aarch64 Linux.
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

/// sys_open - Open a file or directory
///
/// Helper: sys_open write path (O_CREAT/O_TRUNC) — works on any Ext2Fs instance.
/// Returns (inode_num, file_type, is_directory, is_regular, mount_id) or Err.
fn sys_open_write_path(
    fs: &mut crate::fs::ext2::Ext2Fs,
    fs_path: &str,
    display_path: &str,
    want_creat: bool,
    want_excl: bool,
    want_trunc: bool,
    mode: u32,
) -> Result<(u32, crate::fs::ext2::FileType, bool, bool, usize), SyscallResult> {
    use super::errno::{EEXIST, ENOENT, ENOSPC, ENOTDIR};
    use crate::fs::ext2::FileType as Ext2FileType;

    let resolve_result = fs.resolve_path(fs_path);

    let (ino, file_created) = match resolve_result {
        Ok(ino) => {
            if want_creat && want_excl {
                log::debug!("sys_open: file exists and O_EXCL set");
                return Err(SyscallResult::Err(EEXIST as u64));
            }
            (ino, false)
        }
        Err(e) => {
            if e.contains("not found") && want_creat {
                log::debug!("sys_open: creating new file {}", display_path);

                let (parent_path, filename) = match fs_path.rfind('/') {
                    Some(0) => ("/", &fs_path[1..]),
                    Some(idx) => (&fs_path[..idx], &fs_path[idx + 1..]),
                    None => {
                        log::error!("sys_open: invalid path format");
                        return Err(SyscallResult::Err(ENOENT as u64));
                    }
                };

                if filename.is_empty() {
                    log::error!("sys_open: empty filename");
                    return Err(SyscallResult::Err(ENOENT as u64));
                }

                let parent_inode = match fs.resolve_path(parent_path) {
                    Ok(ino) => ino,
                    Err(_) => {
                        log::error!("sys_open: parent directory not found: {}", parent_path);
                        return Err(SyscallResult::Err(ENOENT as u64));
                    }
                };

                let parent = match fs.read_inode(parent_inode) {
                    Ok(ino) => ino,
                    Err(_) => {
                        log::error!("sys_open: failed to read parent inode");
                        return Err(SyscallResult::Err(5)); // EIO
                    }
                };
                if !parent.is_dir() {
                    return Err(SyscallResult::Err(ENOTDIR as u64));
                }

                let file_mode = if mode == 0 { 0o644 } else { (mode & 0o777) as u16 };
                match fs.create_file(parent_inode, filename, file_mode) {
                    Ok(new_inode) => {
                        log::info!("sys_open: created file {} with inode {}", display_path, new_inode);
                        (new_inode, true)
                    }
                    Err(e) => {
                        log::error!("sys_open: failed to create file: {}", e);
                        if e.contains("No free inodes") || e.contains("No space") {
                            return Err(SyscallResult::Err(ENOSPC as u64));
                        }
                        return Err(SyscallResult::Err(5)); // EIO
                    }
                }
            } else {
                log::debug!("sys_open: path resolution failed: {}", e);
                if e.contains("not found") {
                    return Err(SyscallResult::Err(ENOENT as u64));
                } else if e.contains("Not a directory") {
                    return Err(SyscallResult::Err(ENOTDIR as u64));
                } else {
                    return Err(SyscallResult::Err(5)); // EIO
                }
            }
        }
    };

    let inode = match fs.read_inode(ino) {
        Ok(i) => i,
        Err(_) => {
            log::error!("sys_open: failed to read inode {}", ino);
            return Err(SyscallResult::Err(5)); // EIO
        }
    };

    let ft = inode.file_type();
    let is_dir = matches!(ft, Ext2FileType::Directory);
    let is_reg = matches!(ft, Ext2FileType::Regular);

    if want_trunc && is_reg && !file_created {
        log::debug!("sys_open: truncating file inode {}", ino);
        if let Err(e) = fs.truncate_file(ino) {
            log::error!("sys_open: failed to truncate file: {}", e);
            return Err(SyscallResult::Err(5)); // EIO
        }
    }

    let mid = fs.mount_id;
    Ok((ino, ft, is_dir, is_reg, mid))
}

/// Helper: sys_open read path — works on any Ext2Fs instance.
fn sys_open_read_path(
    fs: &crate::fs::ext2::Ext2Fs,
    fs_path: &str,
) -> Result<(u32, crate::fs::ext2::FileType, bool, bool, usize), SyscallResult> {
    use super::errno::{ENOENT, ENOTDIR};
    use crate::fs::ext2::FileType as Ext2FileType;

    let ino = match fs.resolve_path(fs_path) {
        Ok(ino) => ino,
        Err(e) => {
            log::debug!("sys_open: path resolution failed: {}", e);
            if e.contains("not found") {
                return Err(SyscallResult::Err(ENOENT as u64));
            } else if e.contains("Not a directory") {
                return Err(SyscallResult::Err(ENOTDIR as u64));
            } else {
                return Err(SyscallResult::Err(5)); // EIO
            }
        }
    };

    let inode = match fs.read_inode(ino) {
        Ok(i) => i,
        Err(_) => {
            log::error!("sys_open: failed to read inode {}", ino);
            return Err(SyscallResult::Err(5)); // EIO
        }
    };

    let ft = inode.file_type();
    let is_dir = matches!(ft, Ext2FileType::Directory);
    let is_reg = matches!(ft, Ext2FileType::Regular);
    let mid = fs.mount_id;
    Ok((ino, ft, is_dir, is_reg, mid))
}

/// # Arguments
/// * `pathname` - Path to the file (userspace pointer)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_DIRECTORY, etc.)
/// * `mode` - File creation mode (if O_CREAT)
///
/// # Returns
/// File descriptor on success, negative errno on failure
pub fn sys_open(pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EISDIR, EMFILE, ENOENT, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2::{self, FileType as Ext2FileType};
    use crate::ipc::fd::{DirectoryFile, RegularFile};
    use alloc::sync::Arc;
    use spin::Mutex;

    // Copy path from userspace
    let raw_path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_open: raw_path={:?}, flags={:#x}, mode={:#o}", raw_path, flags, mode);

    // Resolve relative paths using current working directory
    let path = if raw_path.starts_with('/') {
        raw_path
    } else {
        // Get current process's cwd
        let cwd = get_current_cwd().unwrap_or_else(|| alloc::string::String::from("/"));
        let absolute = if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, raw_path)
        } else {
            alloc::format!("{}/{}", cwd, raw_path)
        };
        normalize_path(&absolute)
    };

    log::debug!("sys_open: resolved path={:?}", path);

    // Check for /dev directory itself
    if path == "/dev" || path == "/dev/" {
        return handle_devfs_directory_open(flags);
    }

    // Check for /dev/* paths - route to devfs
    if path.starts_with("/dev/") {
        let device_name = &path[5..]; // Remove "/dev/" prefix
        return handle_devfs_open(device_name, flags);
    }

    // Check if this is a FIFO (named pipe)
    if crate::ipc::fifo::FIFO_REGISTRY.exists(&path) {
        return handle_fifo_open(&path, flags);
    }

    // Check for /proc paths - route to procfs
    if path == "/proc" || path.starts_with("/proc/") {
        return handle_procfs_open(&path, flags);
    }

    // Parse flags
    let want_creat = (flags & O_CREAT) != 0;
    let want_excl = (flags & O_EXCL) != 0;
    let want_trunc = (flags & O_TRUNC) != 0;
    let wants_directory = (flags & O_DIRECTORY) != 0;

    // Use read lock for non-modifying opens, write lock only for O_CREAT/O_TRUNC.
    // This allows concurrent exec, file reads, and directory listings without
    // being blocked by another process creating or writing files.
    let needs_write = want_creat || want_trunc;

    // Determine which filesystem to use based on path
    let is_home = ext2::is_home_path(&path);
    let fs_path = if is_home { ext2::strip_home_prefix(&path) } else { &path };

    let (inode_num, file_type, is_directory, is_regular, mount_id) = if needs_write {
        // === WRITE PATH: O_CREAT or O_TRUNC requires exclusive filesystem access ===
        let result = if is_home {
            let mut fs_guard = ext2::home_fs_write();
            match fs_guard.as_mut() {
                Some(fs) => sys_open_write_path(fs, fs_path, &path, want_creat, want_excl, want_trunc, mode),
                None => {
                    log::error!("sys_open: ext2 home filesystem not mounted");
                    return SyscallResult::Err(ENOENT as u64);
                }
            }
        } else {
            let mut fs_guard = ext2::root_fs_write();
            match fs_guard.as_mut() {
                Some(fs) => sys_open_write_path(fs, fs_path, &path, want_creat, want_excl, want_trunc, mode),
                None => {
                    log::error!("sys_open: ext2 root filesystem not mounted");
                    return SyscallResult::Err(ENOENT as u64);
                }
            }
        };
        match result {
            Ok(v) => v,
            Err(e) => return e,
        }
    } else {
        // === READ PATH: No filesystem modification needed, use shared read lock ===
        let result = if is_home {
            let fs_guard = ext2::home_fs_read();
            match fs_guard.as_ref() {
                Some(fs) => sys_open_read_path(fs, fs_path),
                None => {
                    log::error!("sys_open: ext2 home filesystem not mounted");
                    return SyscallResult::Err(ENOENT as u64);
                }
            }
        } else {
            let fs_guard = ext2::root_fs_read();
            match fs_guard.as_ref() {
                Some(fs) => sys_open_read_path(fs, fs_path),
                None => {
                    log::error!("sys_open: ext2 root filesystem not mounted");
                    return SyscallResult::Err(ENOENT as u64);
                }
            }
        };
        match result {
            Ok(v) => v,
            Err(e) => return e,
        }
    };

    let _ = file_type;
    let _ = is_regular;

    // Handle directory vs file cases
    if is_directory {
        if wants_directory || (flags & 0x3) == O_RDONLY {
            // O_DIRECTORY flag is set, or opening with O_RDONLY - allow for getdents
            // Create DirectoryFile structure
            let dir_file = DirectoryFile {
                inode_num: inode_num as u64,
                mount_id,
                position: 0,
            };

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

            // Allocate file descriptor for directory
            let fd_kind = FdKind::Directory(Arc::new(Mutex::new(dir_file)));
            match process.fd_table.alloc(fd_kind) {
                Ok(fd) => {
                    log::info!("sys_open: opened directory {} as fd {} (inode {})", path, fd, inode_num);
                    SyscallResult::Ok(fd as u64)
                }
                Err(_) => {
                    log::error!("sys_open: too many open files");
                    SyscallResult::Err(EMFILE as u64)
                }
            }
        } else {
            // Trying to open directory for writing or similar
            log::debug!("sys_open: {} is a directory (cannot write)", path);
            return SyscallResult::Err(EISDIR as u64);
        }
    } else if wants_directory {
        // O_DIRECTORY was specified but path is not a directory
        log::debug!("sys_open: {} is not a directory (O_DIRECTORY specified)", path);
        return SyscallResult::Err(ENOTDIR as u64);
    } else if !matches!(file_type, Ext2FileType::Regular) {
        // Not a regular file and not a directory
        log::debug!("sys_open: {} is not a regular file (type: {:?})", path, file_type);
        return SyscallResult::Err(EACCES as u64);
    } else {
        // Regular file
        // Create RegularFile structure
        let regular_file = RegularFile {
            inode_num: inode_num as u64,
            mount_id,
            position: 0,
            flags,
        };

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
                    // Get file size from ext2 inode
                    let file_size = match get_ext2_file_size_for_mount(file.inode_num, file.mount_id) {
                        Some(size) => size as i64,
                        None => {
                            log::error!("sys_lseek: cannot get file size for inode {}", file.inode_num);
                            return SyscallResult::Err(5); // EIO - filesystem not available
                        }
                    };
                    let new_position = file_size + offset;
                    if new_position < 0 {
                        return SyscallResult::Err(22); // EINVAL - negative position
                    }
                    new_position as u64
                }
                _ => return SyscallResult::Err(22), // EINVAL
            };
            file.position = new_pos;
            SyscallResult::Ok(new_pos)
        }
        FdKind::Directory(_) => {
            // Directories are not seekable with lseek - use getdents position instead
            SyscallResult::Err(21) // EISDIR
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
            if let Some(inode_stat) = load_ext2_inode_stat_for_mount(file_guard.inode_num, file_guard.mount_id) {
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
        FdKind::Directory(dir) => {
            let dir_guard = dir.lock();
            stat.st_dev = dir_guard.mount_id as u64;
            stat.st_ino = dir_guard.inode_num;
            stat.st_mode = S_IFDIR | 0o755; // Directory with rwxr-xr-x (default)
            stat.st_nlink = 2; // . and ..

            // Try to load inode metadata from ext2 filesystem
            if let Some(inode_stat) = load_ext2_inode_stat_for_mount(dir_guard.inode_num, dir_guard.mount_id) {
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
        FdKind::Device(device_type) => {
            // Device files from devfs
            use crate::fs::devfs;

            // Look up device node for major/minor numbers
            let device_node = devfs::lookup_by_inode(device_type.inode());
            stat.st_dev = 0; // devfs has no backing device
            stat.st_ino = device_type.inode();
            stat.st_mode = S_IFCHR | 0o666; // Character device with rw-rw-rw-
            stat.st_nlink = 1;
            stat.st_rdev = device_node.map(|d| d.rdev()).unwrap_or(0);
        }
        FdKind::DevfsDirectory { .. } => {
            // /dev directory itself
            stat.st_dev = 0; // devfs has no backing device
            stat.st_ino = 0; // Virtual inode for /dev
            stat.st_mode = S_IFDIR | 0o755; // Directory with rwxr-xr-x
            stat.st_nlink = 2; // . and ..
        }
        FdKind::DevptsDirectory { .. } => {
            // /dev/pts directory
            stat.st_dev = 0; // devpts has no backing device
            stat.st_ino = 1; // Virtual inode for /dev/pts
            stat.st_mode = S_IFDIR | 0o755; // Directory with rwxr-xr-x
            stat.st_nlink = 2; // . and ..
        }
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) | FdKind::TcpConnection(_) => {
            // TCP sockets
            static TCP_SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(3000);
            stat.st_dev = 0;
            stat.st_ino = TCP_SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755; // Socket with rwxr-xr-x
            stat.st_nlink = 1;
        }
        FdKind::PtyMaster(pty_num) | FdKind::PtySlave(pty_num) => {
            // PTY devices are character devices
            static PTY_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(4000);
            stat.st_dev = 0;
            stat.st_ino = PTY_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFCHR | 0o620; // Character device with rw--w----
            stat.st_nlink = 1;
            // Major 136 for PTY, minor is pty_num
            stat.st_rdev = make_dev(136, *pty_num as u64);
        }
        FdKind::UnixStream(_) | FdKind::UnixSocket(_) | FdKind::UnixListener(_) => {
            // Unix domain sockets
            static UNIX_SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(5000);
            stat.st_dev = 0;
            stat.st_ino = UNIX_SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755; // Socket with rwxr-xr-x
            stat.st_nlink = 1;
        }
        FdKind::FifoRead(_, _) | FdKind::FifoWrite(_, _) => {
            // Named FIFOs
            static FIFO_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(6000);
            stat.st_dev = 0;
            stat.st_ino = FIFO_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFIFO | 0o644; // FIFO with rw-r--r--
            stat.st_nlink = 1;
            stat.st_size = 0; // FIFOs don't have a seekable size
        }
        FdKind::ProcfsFile { ref content, .. } => {
            // Procfs virtual files
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFREG | 0o444; // Regular file, read-only
            stat.st_nlink = 1;
            stat.st_size = content.len() as i64;
        }
        FdKind::ProcfsDirectory { .. } => {
            // Procfs directory
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFDIR | 0o555; // Directory with r-xr-xr-x
            stat.st_nlink = 2; // . and ..
            stat.st_size = 0;
        }
        FdKind::Epoll(_) => {
            // epoll fds report as anonymous inodes
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFREG | 0o600;
            stat.st_nlink = 1;
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

/// Extract InodeStat from an already-loaded ext2 inode
fn load_inode_stat_from_inode(inode: &crate::fs::ext2::Ext2Inode) -> Option<InodeStat> {
    let mode = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mode)) };
    let uid = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_uid)) };
    let gid = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_gid)) };
    let links_count = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) };
    let atime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_atime)) };
    let mtime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mtime)) };
    let ctime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_ctime)) };
    let blocks = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_blocks)) };

    Some(InodeStat {
        mode: mode as u32,
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

/// Load inode metadata from ext2 filesystem, dispatching to correct mount
///
/// Returns None if the ext2 filesystem is not available or inode cannot be read.
fn load_ext2_inode_stat_for_mount(inode_num: u64, mount_id: usize) -> Option<InodeStat> {
    use crate::fs::ext2;

    // Dispatch to home or root filesystem based on mount_id
    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = fs_guard.as_ref()?;
        let inode = fs.read_inode(inode_num as u32).ok()?;
        return load_inode_stat_from_inode(&inode);
    }

    let fs_guard = ext2::root_fs_read();
    let fs = fs_guard.as_ref()?;
    let inode = fs.read_inode(inode_num as u32).ok()?;
    load_inode_stat_from_inode(&inode)
}

/// Get file size from ext2 inode, dispatching to correct mount
fn get_ext2_file_size_for_mount(inode_num: u64, mount_id: usize) -> Option<u64> {
    use crate::fs::ext2;

    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = fs_guard.as_ref()?;
        let inode = fs.read_inode(inode_num as u32).ok()?;
        return Some(inode.size());
    }

    let fs_guard = ext2::root_fs_read();
    let fs = fs_guard.as_ref()?;
    let inode = fs.read_inode(inode_num as u32).ok()?;
    Some(inode.size())
}

/// Convert ext2 file type to Linux dirent d_type
fn ext2_file_type_to_dt(ext2_type: u8) -> u8 {
    use crate::fs::ext2::dir;
    match ext2_type {
        dir::EXT2_FT_REG_FILE => DT_REG,
        dir::EXT2_FT_DIR => DT_DIR,
        dir::EXT2_FT_CHRDEV => DT_CHR,
        dir::EXT2_FT_BLKDEV => DT_BLK,
        dir::EXT2_FT_FIFO => DT_FIFO,
        dir::EXT2_FT_SOCK => DT_SOCK,
        dir::EXT2_FT_SYMLINK => DT_LNK,
        _ => DT_UNKNOWN,
    }
}

/// Align a value up to the nearest multiple of 8
fn align_up_8(value: usize) -> usize {
    (value + 7) & !7
}

/// sys_getdents64 - Get directory entries
///
/// Reads directory entries into a buffer in Linux dirent64 format.
///
/// # Arguments
/// * `fd` - File descriptor for an open directory
/// * `dirp` - Pointer to user buffer for directory entries
/// * `count` - Size of the buffer in bytes
///
/// # Returns
/// * On success: Number of bytes written to the buffer
/// * On success with no more entries: 0
/// * On error: Negative errno
pub fn sys_getdents64(fd: i32, dirp: u64, count: u64) -> SyscallResult {
    use super::errno::{EBADF, EFAULT, EINVAL, EIO, ENOTDIR};
    use crate::fs::ext2::{self, dir::DirReader};

    log::debug!("sys_getdents64: fd={}, dirp={:#x}, count={}", fd, dirp, count);

    // Validate buffer pointer
    if dirp == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Validate count
    if count == 0 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Get current process and find the fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getdents64: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_getdents64: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_getdents64: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get fd entry
    let fd_entry = match process.fd_table.get(fd) {
        Some(entry) => entry,
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Handle DevfsDirectory specially
    if let FdKind::DevfsDirectory { position } = &fd_entry.kind {
        let start_position = *position;
        drop(manager_guard);
        return handle_devfs_getdents64(fd, dirp, count as usize, start_position, thread_id);
    }

    // Handle DevptsDirectory specially
    if let FdKind::DevptsDirectory { position } = &fd_entry.kind {
        let start_position = *position;
        drop(manager_guard);
        return handle_devpts_getdents64(fd, dirp, count as usize, start_position, thread_id);
    }

    // Handle ProcfsDirectory specially
    if let FdKind::ProcfsDirectory { path, position } = &fd_entry.kind {
        let dir_path = path.clone();
        let start_position = *position;
        drop(manager_guard);
        return handle_procfs_getdents64(fd, dirp, count as usize, &dir_path, start_position, thread_id);
    }

    // Must be a directory fd
    let dir_file = match &fd_entry.kind {
        FdKind::Directory(dir) => dir.clone(),
        _ => return SyscallResult::Err(ENOTDIR as u64),
    };

    // Get directory info
    let dir_guard = dir_file.lock();
    let inode_num = dir_guard.inode_num;
    let dir_mount_id = dir_guard.mount_id;
    let start_position = dir_guard.position;
    drop(dir_guard);

    // Drop process manager lock before acquiring filesystem lock
    drop(manager_guard);

    // Read directory data from ext2, dispatching to correct filesystem
    let is_home_dir = ext2::home_mount_id().map_or(false, |id| id == dir_mount_id);
    let (inode, dir_data) = if is_home_dir {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_getdents64: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode = match fs.read_inode(inode_num as u32) {
            Ok(ino) => ino,
            Err(_) => {
                log::error!("sys_getdents64: failed to read inode {}", inode_num);
                return SyscallResult::Err(EIO as u64);
            }
        };
        let dir_data = match fs.read_directory(&inode) {
            Ok(data) => data,
            Err(e) => {
                log::error!("sys_getdents64: failed to read directory: {}", e);
                return SyscallResult::Err(EIO as u64);
            }
        };
        (inode, dir_data)
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_getdents64: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode = match fs.read_inode(inode_num as u32) {
            Ok(ino) => ino,
            Err(_) => {
                log::error!("sys_getdents64: failed to read inode {}", inode_num);
                return SyscallResult::Err(EIO as u64);
            }
        };
        let dir_data = match fs.read_directory(&inode) {
            Ok(data) => data,
            Err(e) => {
                log::error!("sys_getdents64: failed to read directory: {}", e);
                return SyscallResult::Err(EIO as u64);
            }
        };
        (inode, dir_data)
    };
    let _ = inode; // inode used above, data extracted

    // Parse directory entries and write to user buffer
    let buffer = dirp as *mut u8;
    let buffer_size = count as usize;
    let mut bytes_written = 0usize;
    let mut entry_index = 0usize;
    let mut new_position = start_position;

    for entry in DirReader::new(&dir_data) {
        // Skip entries before our current position
        // Position is stored as entry index for simplicity
        if (entry_index as u64) < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = entry.name.len();
        // d_reclen = header + name + null terminator, aligned to 8 bytes
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        // Check if this entry fits in remaining buffer
        if bytes_written + reclen > buffer_size {
            // No more room - stop here
            break;
        }

        // Write entry to user buffer
        // SAFETY: We've validated the buffer pointer and size
        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // Write d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, entry.inode as u64);

            // Write d_off (i64) at offset 8 - offset to NEXT entry (entry_index + 1)
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // Write d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // Write d_type (u8) at offset 18
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = ext2_file_type_to_dt(entry.file_type);

            // Write d_name (variable length, null-terminated) at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(entry.name.as_ptr(), d_name_ptr, name_len);
            // Null terminator
            *d_name_ptr.add(name_len) = 0;

            // Zero-fill padding to maintain alignment
            let padding_start = 19 + name_len + 1;
            for i in padding_start..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index as u64;
    }

    // Update directory position
    let mut manager_guard = crate::process::manager();
    if let Some(manager) = &mut *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(fd_entry) = process.fd_table.get(fd) {
                if let FdKind::Directory(dir) = &fd_entry.kind {
                    dir.lock().position = new_position;
                }
            }
        }
    }

    log::debug!("sys_getdents64: wrote {} bytes, new_position={}", bytes_written, new_position);
    SyscallResult::Ok(bytes_written as u64)
}


/// sys_unlink - Delete a file
///
/// Removes a directory entry for the specified pathname. If this is the
/// last link to the file and no processes have it open, the file is deleted.
///
/// # Arguments
/// * `pathname` - Path to the file (userspace pointer to null-terminated string)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - File does not exist
/// * EISDIR - pathname refers to a directory
/// * EACCES - Permission denied
/// * EIO - I/O error
pub fn sys_unlink(pathname: u64) -> SyscallResult {
    use super::errno::{EACCES, EIO, EISDIR, ENOENT};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy path from userspace
    let raw_path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    // Normalize path
    let path = if raw_path.starts_with('/') {
        raw_path
    } else {
        let cwd = get_current_cwd().unwrap_or_else(|| alloc::string::String::from("/"));
        let absolute = if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, raw_path)
        } else {
            alloc::format!("{}/{}", cwd, raw_path)
        };
        normalize_path(&absolute)
    };

    log::debug!("sys_unlink: path={:?}", path);

    // Check if this is a FIFO - if so, remove from registry
    {
        use crate::ipc::fifo::FIFO_REGISTRY;
        if FIFO_REGISTRY.exists(&path) {
            match FIFO_REGISTRY.unlink(&path) {
                Ok(()) => {
                    log::info!("sys_unlink: successfully unlinked FIFO {}", path);
                    return SyscallResult::Ok(0);
                }
                Err(errno) => {
                    return SyscallResult::Err(errno as u64);
                }
            }
        }
    }

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&path);
    let fs_path: alloc::string::String = if is_home {
        alloc::string::String::from(ext2::strip_home_prefix(&path))
    } else {
        path.clone()
    };

    // Get the filesystem (with mutable access)
    let unlink_result = if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.unlink_file(&fs_path),
            None => {
                log::error!("sys_unlink: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.unlink_file(&fs_path),
            None => {
                log::error!("sys_unlink: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    // Handle the unlink result
    match unlink_result {
        Ok(()) => {
            log::info!("sys_unlink: successfully unlinked {}", path);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_unlink: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") {
                ENOENT
            } else if e.contains("directory") {
                EISDIR
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_rename - Rename/move a file or directory
///
/// Renames oldpath to newpath. If newpath already exists, it will be atomically
/// replaced (if it's a file) or the operation will fail (if it's a directory).
///
/// # Arguments
/// * `oldpath` - Current path (userspace pointer to null-terminated string)
/// * `newpath` - New path (userspace pointer to null-terminated string)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - oldpath does not exist
/// * EISDIR - newpath is a directory but oldpath is not
/// * ENOTDIR - Component in path is not a directory
/// * EEXIST/ENOTEMPTY - newpath is a non-empty directory
/// * EIO - I/O error
pub fn sys_rename(oldpath: u64, newpath: u64) -> SyscallResult {
    use super::errno::{EACCES, EIO, EISDIR, ENOENT};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy paths from userspace
    let old = match copy_cstr_from_user(oldpath) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let new = match copy_cstr_from_user(newpath) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_rename: old={:?}, new={:?}", old, new);

    // Both paths must be on the same filesystem
    let old_is_home = ext2::is_home_path(&old);
    let new_is_home = ext2::is_home_path(&new);
    if old_is_home != new_is_home {
        log::error!("sys_rename: cross-filesystem rename not supported");
        return SyscallResult::Err(18); // EXDEV
    }

    let fs_old = if old_is_home { alloc::string::String::from(ext2::strip_home_prefix(&old)) } else { old.clone() };
    let fs_new = if new_is_home { alloc::string::String::from(ext2::strip_home_prefix(&new)) } else { new.clone() };

    // Perform the rename operation on the correct filesystem
    let rename_result = if old_is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.rename_file(&fs_old, &fs_new),
            None => {
                log::error!("sys_rename: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.rename_file(&fs_old, &fs_new),
            None => {
                log::error!("sys_rename: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match rename_result {
        Ok(()) => {
            log::info!("sys_rename: successfully renamed {} to {}", old, new);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_rename: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") {
                ENOENT
            } else if e.contains("is a directory") {
                EISDIR
            } else if e.contains("Not a directory") {
                super::errno::ENOTDIR
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_rmdir - Remove an empty directory
///
/// Removes the directory specified by pathname if it is empty
/// (contains only "." and ".." entries).
///
/// # Arguments
/// * `pathname` - Path to the directory (userspace pointer to null-terminated string)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - Directory does not exist
/// * ENOTDIR - pathname is not a directory
/// * ENOTEMPTY - Directory is not empty
/// * EBUSY - Directory is in use (e.g., mount point or current directory)
/// * EINVAL - pathname is "." or ends with "/."
/// * EIO - I/O error
pub fn sys_rmdir(pathname: u64) -> SyscallResult {
    use super::errno::{EACCES, EINVAL, EIO, ENOENT, ENOTEMPTY, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_rmdir: path={:?}", path);

    // Check for invalid paths like "." or ending with "/."
    if path == "." || path.ends_with("/.") {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&path);
    let fs_path: alloc::string::String = if is_home {
        alloc::string::String::from(ext2::strip_home_prefix(&path))
    } else {
        path.clone()
    };

    // Perform the rmdir operation on the correct filesystem
    let rmdir_result = if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.remove_directory(&fs_path),
            None => {
                log::error!("sys_rmdir: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.remove_directory(&fs_path),
            None => {
                log::error!("sys_rmdir: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match rmdir_result {
        Ok(()) => {
            log::info!("sys_rmdir: successfully removed directory {}", path);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_rmdir: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") {
                ENOENT
            } else if e.contains("Not a directory") || e.contains("not a directory") {
                ENOTDIR
            } else if e.contains("not empty") || e.contains("Directory not empty") {
                ENOTEMPTY
            } else if e.contains("root directory") {
                // Cannot remove root directory - treat as busy
                super::errno::EBUSY
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else if e.contains("Invalid") {
                EINVAL
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_link - Create a hard link to a file
///
/// Creates a new hard link pointing to an existing file. Both paths
/// must be on the same filesystem. Hard links to directories are not allowed.
///
/// # Arguments
/// * `oldpath` - Path to the existing file (userspace pointer to null-terminated string)
/// * `newpath` - Path for the new link (userspace pointer to null-terminated string)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - oldpath does not exist
/// * EEXIST - newpath already exists
/// * EPERM - oldpath is a directory
/// * ENOTDIR - A component in path is not a directory
/// * ENOSPC - No space in target directory
/// * EIO - I/O error
pub fn sys_link(oldpath: u64, newpath: u64) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EIO, ENOENT, ENOTDIR, EPERM};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy paths from userspace
    let old = match copy_cstr_from_user(oldpath) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let new = match copy_cstr_from_user(newpath) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_link: oldpath={:?}, newpath={:?}", old, new);

    // Both paths must be on the same filesystem
    let old_is_home = ext2::is_home_path(&old);
    let new_is_home = ext2::is_home_path(&new);
    if old_is_home != new_is_home {
        log::error!("sys_link: cross-filesystem link not supported");
        return SyscallResult::Err(18); // EXDEV
    }

    let fs_old = if old_is_home { alloc::string::String::from(ext2::strip_home_prefix(&old)) } else { old.clone() };
    let fs_new = if new_is_home { alloc::string::String::from(ext2::strip_home_prefix(&new)) } else { new.clone() };

    // Perform the hard link operation on the correct filesystem
    let link_result = if old_is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_hard_link(&fs_old, &fs_new),
            None => {
                log::error!("sys_link: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_hard_link(&fs_old, &fs_new),
            None => {
                log::error!("sys_link: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match link_result {
        Ok(()) => {
            log::info!("sys_link: successfully created hard link {} -> {}", new, old);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_link: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") {
                ENOENT
            } else if e.contains("already exists") || e.contains("Destination already exists") {
                EEXIST
            } else if e.contains("directory") && e.contains("hard link") {
                EPERM // Cannot create hard link to directory
            } else if e.contains("Not a directory") {
                ENOTDIR
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else if e.contains("No space") {
                super::errno::ENOSPC
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_mkdir - Create a directory
///
/// Creates a new directory with the specified pathname and mode.
///
/// # Arguments
/// * `pathname` - Path for the new directory (userspace pointer to null-terminated string)
/// * `mode` - Directory permission bits (e.g., 0o755)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - Parent directory does not exist
/// * EEXIST - Directory already exists
/// * ENOTDIR - Component in path is not a directory
/// * ENOSPC - No space for new directory
/// * EIO - I/O error
pub fn sys_mkdir(pathname: u64, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EIO, ENOENT, ENOSPC, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_mkdir: path={:?}, mode={:#o}", path, mode);

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&path);
    let fs_path: alloc::string::String = if is_home {
        alloc::string::String::from(ext2::strip_home_prefix(&path))
    } else {
        path.clone()
    };

    // Create the directory on the correct filesystem
    let dir_mode = if mode == 0 { 0o755 } else { (mode & 0o777) as u16 };
    let mkdir_result = if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_directory(&fs_path, dir_mode),
            None => {
                log::error!("sys_mkdir: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_directory(&fs_path, dir_mode),
            None => {
                log::error!("sys_mkdir: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match mkdir_result {
        Ok(inode_num) => {
            log::info!("sys_mkdir: successfully created directory {} (inode {})", path, inode_num);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_mkdir: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") || e.contains("Path component not found") {
                ENOENT
            } else if e.contains("already exists") || e.contains("Directory already exists") {
                EEXIST
            } else if e.contains("Not a directory") || e.contains("not a directory") {
                ENOTDIR
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else if e.contains("No space") || e.contains("No free") {
                ENOSPC
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_symlink - Create a symbolic link
///
/// Creates a new symbolic link at linkpath pointing to target.
/// Unlike hard links, symbolic links can reference directories and
/// can cross filesystem boundaries (though in our case we only have ext2).
///
/// # Arguments
/// * `target` - The target path the symlink will point to (userspace pointer)
/// * `linkpath` - Path where the symlink will be created (userspace pointer)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - A component of linkpath's parent directory does not exist
/// * EEXIST - linkpath already exists
/// * ENOTDIR - A component of the path is not a directory
/// * ENOSPC - No space to create the symlink
/// * EIO - I/O error
pub fn sys_symlink(target: u64, linkpath: u64) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EINVAL, EIO, ENOENT, ENOSPC, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Copy paths from userspace
    let target_str = match copy_cstr_from_user(target) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let linkpath_str = match copy_cstr_from_user(linkpath) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_symlink: target={:?}, linkpath={:?}", target_str, linkpath_str);

    // Validate target is not empty
    if target_str.is_empty() {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Determine which filesystem to use based on linkpath
    let is_home = ext2::is_home_path(&linkpath_str);
    let fs_target = target_str.clone();
    let fs_linkpath: alloc::string::String = if is_home {
        alloc::string::String::from(ext2::strip_home_prefix(&linkpath_str))
    } else {
        linkpath_str.clone()
    };

    // Create the symbolic link on the correct filesystem
    let symlink_result = if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_symlink(&fs_target, &fs_linkpath),
            None => {
                log::error!("sys_symlink: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_symlink(&fs_target, &fs_linkpath),
            None => {
                log::error!("sys_symlink: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match symlink_result {
        Ok(()) => {
            log::info!("sys_symlink: successfully created symlink {} -> {}", linkpath_str, target_str);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_symlink: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("not found") || e.contains("not exist") || e.contains("Path component not found") {
                ENOENT
            } else if e.contains("already exists") || e.contains("File already exists") {
                EEXIST
            } else if e.contains("Not a directory") || e.contains("not a directory") {
                ENOTDIR
            } else if e.contains("permission") || e.contains("Cannot") {
                EACCES
            } else if e.contains("No space") || e.contains("No free") {
                ENOSPC
            } else if e.contains("empty") || e.contains("Invalid") {
                EINVAL
            } else {
                EIO
            };
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_readlink - Read the target of a symbolic link
///
/// Reads the contents of the symbolic link (i.e., the path it points to)
/// and writes it to the provided buffer. The result is NOT null-terminated.
///
/// # Arguments
/// * `pathname` - Path to the symbolic link (userspace pointer)
/// * `buf` - Buffer to store the symlink target (userspace pointer)
/// * `bufsize` - Size of the buffer
///
/// # Returns
/// Number of bytes placed in buf on success, negative errno on failure
///
/// # Errors
/// * ENOENT - The symlink does not exist
/// * EINVAL - pathname is not a symbolic link
/// * EFAULT - Invalid buffer pointer
/// * EIO - I/O error
pub fn sys_readlink(pathname: u64, buf: u64, bufsize: u64) -> SyscallResult {
    use super::errno::{EFAULT, EINVAL, EIO, ENOENT};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Validate buffer pointer
    if buf == 0 || bufsize == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_readlink: pathname={:?}, bufsize={}", path, bufsize);

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&path);
    let fs_path = if is_home { ext2::strip_home_prefix(&path) } else { &path };

    // Resolve path and read symlink from the correct filesystem
    let target = if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_readlink: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode_num = match fs.resolve_path_no_follow(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_readlink: path resolution failed: {}", e);
                return SyscallResult::Err(ENOENT as u64);
            }
        };
        match fs.read_symlink(inode_num) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("sys_readlink: failed to read symlink: {}", e);
                let errno = if e.contains("Not a symbolic link") { EINVAL } else if e.contains("not found") { ENOENT } else { EIO };
                return SyscallResult::Err(errno as u64);
            }
        }
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_readlink: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode_num = match fs.resolve_path_no_follow(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_readlink: path resolution failed: {}", e);
                return SyscallResult::Err(ENOENT as u64);
            }
        };
        match fs.read_symlink(inode_num) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("sys_readlink: failed to read symlink: {}", e);
                let errno = if e.contains("Not a symbolic link") { EINVAL } else if e.contains("not found") { ENOENT } else { EIO };
                return SyscallResult::Err(errno as u64);
            }
        }
    };

    // Calculate how many bytes to copy (capped by buffer size)
    let target_bytes = target.as_bytes();
    let bytes_to_copy = core::cmp::min(target_bytes.len(), bufsize as usize);

    // Copy to user buffer (NOT null-terminated, per readlink semantics)
    let user_buf = buf as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(target_bytes.as_ptr(), user_buf, bytes_to_copy);
    }

    log::debug!("sys_readlink: returning {} bytes: {:?}", bytes_to_copy, &target[..bytes_to_copy]);
    SyscallResult::Ok(bytes_to_copy as u64)
}

/// sys_access - Check user's permissions for a file
///
/// # Arguments
/// * `pathname` - Path to the file (userspace pointer to null-terminated string)
/// * `mode` - Access mode to check (R_OK=4, W_OK=2, X_OK=1, F_OK=0)
///
/// # Returns
/// 0 on success (access allowed), negative errno on failure
///
/// # Errors
/// * ENOENT - File does not exist
/// * EACCES - Access would be denied
/// * ENOTDIR - A component of path is not a directory
pub fn sys_access(pathname: u64, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, ENOENT, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    // Access mode constants
    const F_OK: u32 = 0;  // Test for existence
    const X_OK: u32 = 1;  // Test for execute permission
    const W_OK: u32 = 2;  // Test for write permission
    const R_OK: u32 = 4;  // Test for read permission

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_access: path={:?}, mode={:#o}", path, mode);

    // Handle /dev paths specially
    if path == "/dev" || path == "/dev/" {
        // /dev directory exists with rwxr-xr-x permissions
        if mode == F_OK {
            return SyscallResult::Ok(0);
        }
        // Owner has rwx, so all access checks pass
        return SyscallResult::Ok(0);
    }
    if path.starts_with("/dev/") {
        use crate::fs::devfs;
        let device_name = &path[5..];
        if devfs::lookup(device_name).is_some() {
            // Device exists with rw-rw-rw- permissions
            if mode == F_OK {
                return SyscallResult::Ok(0);
            }
            // Devices have rw permissions (no execute)
            if (mode & X_OK) != 0 {
                return SyscallResult::Err(EACCES as u64);
            }
            return SyscallResult::Ok(0);
        }
        return SyscallResult::Err(ENOENT as u64);
    }

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&path);
    let fs_path = if is_home { ext2::strip_home_prefix(&path) } else { &path };

    // Resolve path and read inode from the correct filesystem
    let (inode_num, inode) = if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_access: ext2 home filesystem not mounted");
                return SyscallResult::Err(ENOENT as u64);
            }
        };
        let ino = match fs.resolve_path(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_access: path resolution failed: {}", e);
                let errno = if e.contains("not found") { ENOENT } else if e.contains("Not a directory") { ENOTDIR } else { 5 };
                return SyscallResult::Err(errno as u64);
            }
        };
        if mode == F_OK {
            return SyscallResult::Ok(0);
        }
        let inode = match fs.read_inode(ino) {
            Ok(i) => i,
            Err(_) => return SyscallResult::Err(5),
        };
        (ino, inode)
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_access: ext2 root filesystem not mounted");
                return SyscallResult::Err(ENOENT as u64);
            }
        };
        let ino = match fs.resolve_path(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_access: path resolution failed: {}", e);
                let errno = if e.contains("not found") { ENOENT } else if e.contains("Not a directory") { ENOTDIR } else { 5 };
                return SyscallResult::Err(errno as u64);
            }
        };
        if mode == F_OK {
            return SyscallResult::Ok(0);
        }
        let inode = match fs.read_inode(ino) {
            Ok(i) => i,
            Err(_) => return SyscallResult::Err(5),
        };
        (ino, inode)
    };
    let _ = inode_num;

    // Get permission bits from inode mode (owner permissions in bits 8-6)
    let inode_mode = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mode)) };
    let owner_perms = (inode_mode >> 6) & 0o7;

    // Check requested permissions against owner permissions
    // (We don't have real users yet, so we check owner bits only)
    if (mode & R_OK) != 0 && (owner_perms & 0o4) == 0 {
        log::debug!("sys_access: read permission denied");
        return SyscallResult::Err(EACCES as u64);
    }
    if (mode & W_OK) != 0 && (owner_perms & 0o2) == 0 {
        log::debug!("sys_access: write permission denied");
        return SyscallResult::Err(EACCES as u64);
    }
    if (mode & X_OK) != 0 && (owner_perms & 0o1) == 0 {
        log::debug!("sys_access: execute permission denied");
        return SyscallResult::Err(EACCES as u64);
    }

    log::debug!("sys_access: access check passed");
    SyscallResult::Ok(0)
}

/// Handle opening a device file from /dev/*
///
/// # Arguments
/// * `device_name` - Name of the device (without /dev/ prefix)
/// Handle opening a /proc file or directory
///
/// For directories (/proc, /proc/trace, /proc/[pid]), returns a ProcfsDirectory fd.
/// For files, generates the content at open time and stores it in a ProcfsFile fd.
fn handle_procfs_open(path: &str, _flags: u32) -> SyscallResult {
    use crate::ipc::fd::{FdKind, FileDescriptor};

    let normalized = path.trim_end_matches('/');

    // Check if this is a directory path
    // /proc itself is the root directory; for sub-paths, check if the entry is a directory type
    let is_directory = if normalized == "/proc" {
        true
    } else if let Some(entry) = crate::fs::procfs::lookup_by_path(normalized) {
        entry.entry_type.is_directory()
    } else {
        false
    };

    if is_directory {
        let dir_path = alloc::string::String::from(normalized);
        let fd_kind = FdKind::ProcfsDirectory { path: dir_path, position: 0 };
        let fd_entry = FileDescriptor::new(fd_kind);

        let thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => return SyscallResult::Err(3), // ESRCH
        };
        let mut manager_guard = crate::process::manager();
        let process = match &mut *manager_guard {
            Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
                Some((_pid, p)) => p,
                None => return SyscallResult::Err(3),
            },
            None => return SyscallResult::Err(3),
        };

        return match process.fd_table.alloc_with_entry(fd_entry) {
            Ok(fd) => {
                log::debug!("handle_procfs_open: opened {} as directory fd={}", normalized, fd);
                SyscallResult::Ok(fd as u64)
            }
            Err(e) => SyscallResult::Err(e as u64),
        };
    }

    // Regular file open
    let content = match crate::fs::procfs::read_file(path) {
        Ok(c) => c,
        Err(_) => return SyscallResult::Err(super::errno::ENOENT as u64),
    };

    let fd_kind = FdKind::ProcfsFile { content, position: 0 };
    let fd_entry = FileDescriptor::new(fd_kind);

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(3), // ESRCH
    };
    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Err(3),
        },
        None => return SyscallResult::Err(3),
    };

    match process.fd_table.alloc_with_entry(fd_entry) {
        Ok(fd) => {
            log::debug!("handle_procfs_open: opened {} as fd={}", path, fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// * `_flags` - Open flags (currently unused for devices)
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_devfs_open(device_name: &str, _flags: u32) -> SyscallResult {
    use super::errno::{EMFILE, ENOENT};
    use crate::fs::devfs;

    log::debug!("handle_devfs_open: device_name={:?}", device_name);

    // Check for /dev/pts/* paths - route to devptsfs
    if device_name.starts_with("pts/") {
        let pty_name = &device_name[4..]; // Remove "pts/" prefix
        return handle_devpts_open(pty_name);
    }

    // Check for /dev/pts directory itself
    if device_name == "pts" {
        return handle_devpts_directory_open();
    }

    // Look up the device in static devfs
    let device = match devfs::lookup(device_name) {
        Some(d) => d,
        None => {
            log::debug!("handle_devfs_open: device not found: {}", device_name);
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Get current process and allocate fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("handle_devfs_open: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("handle_devfs_open: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devfs_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with Device kind
    let fd_kind = FdKind::Device(device.device_type);
    match process.fd_table.alloc(fd_kind) {
        Ok(fd) => {
            log::info!("handle_devfs_open: opened /dev/{} as fd {}", device_name, fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => {
            log::error!("handle_devfs_open: too many open files");
            SyscallResult::Err(EMFILE as u64)
        }
    }
}

/// Handle opening a PTY slave device from /dev/pts/*
///
/// # Arguments
/// * `pty_name` - PTY number as string (e.g., "0", "1")
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_devpts_open(pty_name: &str) -> SyscallResult {
    use super::errno::{EMFILE, ENOENT};
    use crate::fs::devptsfs;

    // Look up the PTY slave in devptsfs
    let pty_num = match devptsfs::lookup(pty_name) {
        Some(num) => num,
        None => {
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Get current process and allocate fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with PtySlave kind
    let fd_kind = FdKind::PtySlave(pty_num);
    match process.fd_table.alloc(fd_kind) {
        Ok(fd) => {
            // Increment slave reference count so master can detect hangup
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                pair.slave_open();
            }
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => {
            SyscallResult::Err(EMFILE as u64)
        }
    }
}

/// Handle opening the /dev/pts directory itself
///
/// Returns a directory fd that can be used with getdents64 to list PTY slaves.
fn handle_devpts_directory_open() -> SyscallResult {
    use super::errno::EMFILE;

    log::debug!("handle_devpts_directory_open: opening /dev/pts directory");

    // Get current process and allocate fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("handle_devpts_directory_open: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("handle_devpts_directory_open: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devpts_directory_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with DevptsDirectory kind
    let fd_kind = FdKind::DevptsDirectory { position: 0 };
    match process.fd_table.alloc(fd_kind) {
        Ok(fd) => {
            log::info!("handle_devpts_directory_open: opened /dev/pts as fd {}", fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => {
            log::error!("handle_devpts_directory_open: too many open files");
            SyscallResult::Err(EMFILE as u64)
        }
    }
}

/// Handle opening the /dev directory itself
///
/// Returns a DevfsDirectory fd that can be used with getdents64.
///
/// # Arguments
/// * `_flags` - Open flags (O_DIRECTORY expected)
fn handle_devfs_directory_open(_flags: u32) -> SyscallResult {
    use super::errno::EMFILE;

    log::debug!("handle_devfs_directory_open: opening /dev directory");

    // Get current process and allocate fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("handle_devfs_directory_open: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("handle_devfs_directory_open: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devfs_directory_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with DevfsDirectory kind
    let fd_kind = FdKind::DevfsDirectory { position: 0 };
    match process.fd_table.alloc(fd_kind) {
        Ok(fd) => {
            log::info!("handle_devfs_directory_open: opened /dev as fd {}", fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => {
            log::error!("handle_devfs_directory_open: too many open files");
            SyscallResult::Err(EMFILE as u64)
        }
    }
}

/// Handle getdents64 for the /dev directory
///
/// Returns virtual directory entries for all registered devices.
fn handle_devfs_getdents64(
    fd: i32,
    dirp: u64,
    buffer_size: usize,
    start_position: u64,
    thread_id: u64,
) -> SyscallResult {
    use crate::fs::devfs;

    // Get the list of device names
    let devices = devfs::list_devices();

    // Build entries: ".", "..", then each device
    // We treat position as entry index (0 = ".", 1 = "..", 2+ = devices)
    let buffer = dirp as *mut u8;
    let mut bytes_written = 0usize;
    let mut entry_index = 0u64;
    let mut new_position = start_position;

    // Helper entries
    let special_entries: [(&str, u64); 2] = [
        (".", 0),   // inode 0 for /dev directory itself
        ("..", 2),  // inode 2 = root directory
    ];

    // Iterate through special entries first
    for (name, inode) in special_entries.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, *inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18 - DT_DIR for . and ..
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = DT_DIR;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Now iterate through device entries
    for device_name in devices.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = device_name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        // Get device inode
        let inode = devfs::lookup(device_name)
            .map(|d| d.device_type.inode())
            .unwrap_or(0);

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18 - DT_CHR for character devices
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = DT_CHR;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(device_name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Update directory position in the fd
    // Need to get process again since we dropped manager_guard
    let mut manager_guard = crate::process::manager();
    if let Some(manager) = &mut *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(fd_entry) = process.fd_table.get_mut(fd) {
                if let FdKind::DevfsDirectory { ref mut position } = fd_entry.kind {
                    *position = new_position;
                }
            }
        }
    }

    log::debug!("handle_devfs_getdents64: wrote {} bytes, new_position={}", bytes_written, new_position);
    SyscallResult::Ok(bytes_written as u64)
}

/// Handle getdents64 for the /dev/pts directory
///
/// Returns virtual directory entries for all active and unlocked PTY slaves.
fn handle_devpts_getdents64(
    fd: i32,
    dirp: u64,
    buffer_size: usize,
    start_position: u64,
    thread_id: u64,
) -> SyscallResult {
    use crate::fs::devptsfs;

    // Get the list of PTY slave entries
    let entries = devptsfs::list_entries();

    // Build entries: ".", "..", then each PTY slave
    let buffer = dirp as *mut u8;
    let mut bytes_written = 0usize;
    let mut entry_index = 0u64;
    let mut new_position = start_position;

    // Special entries: . and ..
    let special_entries: [(&str, u64, u8); 2] = [
        (".", 1, DT_DIR),      // inode 1 for /dev/pts directory itself
        ("..", 0, DT_DIR),     // inode 0 = /dev directory (parent)
    ];

    // Iterate through special entries first
    for (name, inode, dtype) in special_entries.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, *inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = *dtype;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Now iterate through PTY slave entries
    for entry in entries.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name = entry.name();
        let name_len = name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, entry.inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18 - DT_CHR for character devices
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = DT_CHR;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Update directory position in the fd
    let mut manager_guard = crate::process::manager();
    if let Some(manager) = &mut *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(fd_entry) = process.fd_table.get_mut(fd) {
                if let FdKind::DevptsDirectory { ref mut position } = fd_entry.kind {
                    *position = new_position;
                }
            }
        }
    }

    log::debug!("handle_devpts_getdents64: wrote {} bytes, new_position={}", bytes_written, new_position);
    SyscallResult::Ok(bytes_written as u64)
}

/// Handle getdents64 for /proc directories
///
/// Returns virtual directory entries for procfs. Handles:
/// - "/proc" - top-level directory (static entries + PID directories)
/// - "/proc/trace" - trace subdirectory
/// - "/proc/[pid]" - per-process directory
fn handle_procfs_getdents64(
    fd: i32,
    dirp: u64,
    buffer_size: usize,
    dir_path: &str,
    start_position: u64,
    thread_id: u64,
) -> SyscallResult {
    use alloc::string::String;
    use alloc::vec::Vec;

    // Get the list of entries for this directory
    let entries: Vec<String> = if dir_path == "/proc" {
        crate::fs::procfs::list_entries()
    } else if dir_path == "/proc/trace" {
        crate::fs::procfs::list_trace_entries()
    } else if dir_path.starts_with("/proc/") {
        // Per-PID directory - only contains "status"
        let relative = dir_path.strip_prefix("/proc/").unwrap_or("");
        if !relative.is_empty() && relative.chars().all(|c| c.is_ascii_digit()) {
            alloc::vec![String::from("status")]
        } else {
            alloc::vec![]
        }
    } else {
        alloc::vec![]
    };

    // Build entries: ".", "..", then each entry
    let buffer = dirp as *mut u8;
    let mut bytes_written = 0usize;
    let mut entry_index = 0u64;
    let mut new_position = start_position;

    // Helper entries: . and ..
    let special_entries: [(&str, u64); 2] = [
        (".", 0),   // inode 0 for this directory
        ("..", 2),  // inode 2 = root directory
    ];

    // Iterate through special entries first
    for (name, inode) in special_entries.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, *inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18 - DT_DIR for . and ..
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = DT_DIR;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Now iterate through the directory entries
    for entry_name in entries.iter() {
        if entry_index < start_position {
            entry_index += 1;
            continue;
        }

        let name_len = entry_name.len();
        let reclen = align_up_8(DIRENT64_HEADER_SIZE + name_len + 1);

        if bytes_written + reclen > buffer_size {
            break;
        }

        // Determine the entry type and inode
        // Look up the entry in procfs to get its type
        let full_path = alloc::format!("{}/{}", dir_path, entry_name);
        let (inode, dtype) = if let Some(entry) = crate::fs::procfs::lookup_by_path(&full_path) {
            let ino = entry.entry_type.inode();
            let dt = if entry.entry_type.is_directory() { DT_DIR } else { DT_REG };
            (ino, dt)
        } else {
            // Check if the entry name is a PID (numeric) - it's a directory
            if entry_name.chars().all(|c| c.is_ascii_digit()) {
                let pid: u64 = entry_name.parse().unwrap_or(0);
                (10000 + pid, DT_DIR)
            } else {
                (0, DT_REG)
            }
        };

        unsafe {
            let entry_ptr = buffer.add(bytes_written);

            // d_ino (u64) at offset 0
            let d_ino_ptr = entry_ptr as *mut u64;
            core::ptr::write_unaligned(d_ino_ptr, inode);

            // d_off (i64) at offset 8 - offset to NEXT entry
            let d_off_ptr = entry_ptr.add(8) as *mut i64;
            core::ptr::write_unaligned(d_off_ptr, (entry_index + 1) as i64);

            // d_reclen (u16) at offset 16
            let d_reclen_ptr = entry_ptr.add(16) as *mut u16;
            core::ptr::write_unaligned(d_reclen_ptr, reclen as u16);

            // d_type (u8) at offset 18
            let d_type_ptr = entry_ptr.add(18);
            *d_type_ptr = dtype;

            // d_name at offset 19
            let d_name_ptr = entry_ptr.add(19);
            core::ptr::copy_nonoverlapping(entry_name.as_ptr(), d_name_ptr, name_len);
            *d_name_ptr.add(name_len) = 0;

            // Zero padding
            for i in (19 + name_len + 1)..reclen {
                *entry_ptr.add(i) = 0;
            }
        }

        bytes_written += reclen;
        entry_index += 1;
        new_position = entry_index;
    }

    // Update directory position in the fd
    let mut manager_guard = crate::process::manager();
    if let Some(manager) = &mut *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(fd_entry) = process.fd_table.get_mut(fd) {
                if let FdKind::ProcfsDirectory { ref mut position, .. } = fd_entry.kind {
                    *position = new_position;
                }
            }
        }
    }

    log::debug!("handle_procfs_getdents64: wrote {} bytes, new_position={}", bytes_written, new_position);
    SyscallResult::Ok(bytes_written as u64)
}

/// sys_getcwd - Get current working directory
///
/// Returns the absolute pathname of the current working directory.
///
/// # Arguments
/// * `buf` - Buffer to store the path (userspace pointer)
/// * `size` - Size of the buffer
///
/// # Returns
/// Pointer to buf on success (as u64), negative errno on failure
///
/// # Errors
/// * EFAULT - Invalid buffer pointer
/// * ERANGE - Buffer too small
/// * ENOENT - cwd has been unlinked (not implemented yet)
pub fn sys_getcwd(buf: u64, size: u64) -> SyscallResult {
    use super::errno::{EFAULT, EINVAL, ERANGE};

    log::debug!("sys_getcwd: buf={:#x}, size={}", buf, size);

    // Validate buffer pointer
    if buf == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Size must be at least 1 for the null terminator
    if size == 0 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getcwd: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_getcwd: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_getcwd: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get the cwd from the process
    let cwd = &process.cwd;
    let cwd_bytes = cwd.as_bytes();
    let required_size = cwd_bytes.len() + 1; // +1 for null terminator

    // Check if buffer is large enough
    if required_size > size as usize {
        log::debug!("sys_getcwd: buffer too small ({} < {})", size, required_size);
        return SyscallResult::Err(ERANGE as u64);
    }

    // Copy to user buffer with null terminator
    let user_buf = buf as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(cwd_bytes.as_ptr(), user_buf, cwd_bytes.len());
        *user_buf.add(cwd_bytes.len()) = 0; // Null terminator
    }

    log::debug!("sys_getcwd: returning {:?}", cwd);
    SyscallResult::Ok(cwd_bytes.len() as u64)
}

/// sys_chdir - Change current working directory
///
/// Changes the current working directory to the specified path.
///
/// # Arguments
/// * `pathname` - Path to the new working directory (userspace pointer)
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// # Errors
/// * ENOENT - Directory does not exist
/// * ENOTDIR - Path is not a directory
/// * EACCES - Permission denied
/// * EIO - I/O error
pub fn sys_chdir(pathname: u64) -> SyscallResult {
    use super::errno::{EACCES, EIO, ENOENT, ENOTDIR};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2::{self, FileType as Ext2FileType};
    use alloc::string::String;

    // Copy path from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_chdir: path={:?}", path);

    // Handle empty path
    if path.is_empty() {
        return SyscallResult::Err(ENOENT as u64);
    }

    // Get current process cwd for resolving relative paths
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_chdir: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // First, get the current cwd for relative path resolution
    let current_cwd = {
        let manager_guard = crate::process::manager();
        match &*manager_guard {
            Some(manager) => match manager.find_process_by_thread(thread_id) {
                Some((_, p)) => p.cwd.clone(),
                None => return SyscallResult::Err(3), // ESRCH
            },
            None => return SyscallResult::Err(3), // ESRCH
        }
    };

    // Normalize the path (handle relative paths)
    let absolute_path = if path.starts_with('/') {
        path.clone()
    } else {
        // Combine current cwd with relative path
        if current_cwd.ends_with('/') {
            alloc::format!("{}{}", current_cwd, path)
        } else {
            alloc::format!("{}/{}", current_cwd, path)
        }
    };

    // Normalize the path (resolve . and ..)
    let normalized = normalize_path(&absolute_path);

    // Handle /dev directory and its contents specially
    if normalized == "/dev" {
        // /dev is always accessible as a directory
        let mut manager_guard = crate::process::manager();
        if let Some(manager) = &mut *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                process.cwd = String::from("/dev");
                log::info!("sys_chdir: changed cwd to /dev");
                return SyscallResult::Ok(0);
            }
        }
        return SyscallResult::Err(3); // ESRCH
    }

    // Handle paths under /dev - these are device files, not directories
    if normalized.starts_with("/dev/") {
        let device_name = &normalized[5..]; // Strip "/dev/" prefix
        if crate::fs::devfs::lookup(device_name).is_some() {
            // Device exists but is a file, not a directory
            log::debug!("sys_chdir: /dev/{} is a device file, not a directory", device_name);
            return SyscallResult::Err(ENOTDIR as u64);
        } else {
            // Device doesn't exist
            log::debug!("sys_chdir: /dev/{} not found", device_name);
            return SyscallResult::Err(ENOENT as u64);
        }
    }

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&normalized);
    let fs_path = if is_home { ext2::strip_home_prefix(&normalized) } else { normalized.as_str() };

    // Resolve path and verify it's a directory
    let is_dir = if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_chdir: ext2 home filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode_num = match fs.resolve_path(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_chdir: path resolution failed: {}", e);
                let errno = if e.contains("not found") { ENOENT } else if e.contains("Not a directory") { ENOTDIR } else if e.contains("permission") { EACCES } else { EIO };
                return SyscallResult::Err(errno as u64);
            }
        };
        let inode = match fs.read_inode(inode_num) {
            Ok(ino) => ino,
            Err(_) => return SyscallResult::Err(EIO as u64),
        };
        matches!(inode.file_type(), Ext2FileType::Directory)
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_chdir: ext2 root filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        let inode_num = match fs.resolve_path(fs_path) {
            Ok(ino) => ino,
            Err(e) => {
                log::debug!("sys_chdir: path resolution failed: {}", e);
                let errno = if e.contains("not found") { ENOENT } else if e.contains("Not a directory") { ENOTDIR } else if e.contains("permission") { EACCES } else { EIO };
                return SyscallResult::Err(errno as u64);
            }
        };
        let inode = match fs.read_inode(inode_num) {
            Ok(ino) => ino,
            Err(_) => return SyscallResult::Err(EIO as u64),
        };
        matches!(inode.file_type(), Ext2FileType::Directory)
    };

    if !is_dir {
        log::debug!("sys_chdir: {} is not a directory", normalized);
        return SyscallResult::Err(ENOTDIR as u64);
    }

    // Update the process's cwd
    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_, p)) => p,
            None => {
                log::error!("sys_chdir: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_chdir: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    process.cwd = normalized.clone();
    log::info!("sys_chdir: changed cwd to {}", normalized);
    SyscallResult::Ok(0)
}

/// Normalize a path by resolving . and .. components
///
/// Examples:
/// - "/foo/bar/../baz" -> "/foo/baz"
/// - "/foo/./bar" -> "/foo/bar"
/// - "/../foo" -> "/foo" (can't go above root)
pub fn normalize_path(path: &str) -> alloc::string::String {
    use alloc::string::String;
    use alloc::vec::Vec;

    let mut components: Vec<&str> = Vec::new();

    for component in path.split('/') {
        match component {
            "" | "." => continue, // Skip empty and current directory
            ".." => {
                // Go up one level (but not above root)
                components.pop();
            }
            _ => components.push(component),
        }
    }

    if components.is_empty() {
        String::from("/")
    } else {
        let mut result = String::new();
        for component in components {
            result.push('/');
            result.push_str(component);
        }
        result
    }
}

/// Get the current working directory for the current process
///
/// Returns None if the current thread or process cannot be determined.
pub fn get_current_cwd() -> Option<alloc::string::String> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let manager_guard = crate::process::manager();
    match &*manager_guard {
        Some(manager) => manager
            .find_process_by_thread(thread_id)
            .map(|(_, p)| p.cwd.clone()),
        None => None,
    }
}

/// Handle opening a FIFO (named pipe)
///
/// # Arguments
/// * `path` - Absolute path to the FIFO
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_NONBLOCK, etc.)
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_fifo_open(path: &str, flags: u32) -> SyscallResult {
    use super::errno::EMFILE;
    use crate::ipc::fd::{FdKind, FileDescriptor, status_flags};
    use crate::ipc::fifo::{FifoOpenResult, open_fifo_read, open_fifo_write, complete_fifo_open};
    use alloc::string::String;

    let access_mode = flags & 3; // O_RDONLY=0, O_WRONLY=1, O_RDWR=2
    let nonblock = (flags & status_flags::O_NONBLOCK) != 0;

    log::debug!("handle_fifo_open: path={}, access_mode={}, nonblock={}", path, access_mode, nonblock);

    // O_RDWR on a FIFO is not well-defined in POSIX, but we can support it
    // by opening both read and write ends. For simplicity, treat it as read.
    let for_write = access_mode == O_WRONLY;

    // Attempt to open the FIFO
    let result = if for_write {
        open_fifo_write(path, nonblock)
    } else {
        open_fifo_read(path, nonblock)
    };

    match result {
        FifoOpenResult::Ready(buffer) => {
            // FIFO is ready - create fd
            let kind = if for_write {
                FdKind::FifoWrite(String::from(path), buffer)
            } else {
                FdKind::FifoRead(String::from(path), buffer)
            };

            let mut fd_entry = FileDescriptor::new(kind);
            if nonblock {
                fd_entry.status_flags |= status_flags::O_NONBLOCK;
            }

            // Allocate fd in current process
            let thread_id = match crate::task::scheduler::current_thread_id() {
                Some(tid) => tid,
                None => return SyscallResult::Err(3), // ESRCH
            };

            let mut manager_guard = crate::process::manager();
            let manager = match manager_guard.as_mut() {
                Some(m) => m,
                None => return SyscallResult::Err(3), // ESRCH
            };

            let (_, process) = match manager.find_process_by_thread_mut(thread_id) {
                Some(p) => p,
                None => return SyscallResult::Err(3), // ESRCH
            };

            match process.fd_table.alloc_with_entry(fd_entry) {
                Ok(fd) => {
                    log::info!("handle_fifo_open: opened FIFO {} as fd {} ({})",
                        path, fd, if for_write { "write" } else { "read" });
                    SyscallResult::Ok(fd as u64)
                }
                Err(_) => {
                    log::error!("handle_fifo_open: too many open files");
                    SyscallResult::Err(EMFILE as u64)
                }
            }
        }
        FifoOpenResult::Block => {
            // Need to block waiting for the other end
            // Following the TCP blocking pattern with proper HLT loop
            let path_owned = String::from(path);

            let thread_id = match crate::task::scheduler::current_thread_id() {
                Some(tid) => tid,
                None => return SyscallResult::Err(3), // ESRCH
            };

            log::debug!("handle_fifo_open: thread {} blocking for {} end on {}",
                thread_id, if for_write { "reader" } else { "writer" }, path);

            // Block the current thread AND set blocked_in_syscall flag.
            // CRITICAL: Setting blocked_in_syscall is essential because:
            // 1. The thread will enter a kernel-mode HLT loop below
            // 2. If a context switch happens while in HLT, the scheduler sees
            //    from_userspace=false (kernel mode) but blocked_in_syscall tells
            //    it to save/restore kernel context, not userspace context
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current();
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = true;
                }
            });

            // CRITICAL RACE CONDITION FIX:
            // Check if other end opened AGAIN after setting Blocked state.
            // The other end might have opened between:
            //   - when open_fifo_read/write returned Block
            //   - when we set thread state to Blocked
            // If other end opened during that window, add_reader/add_writer
            // would have tried to wake us but unblock() would have done nothing.
            let other_end_ready = Cpu::without_interrupts(|| {
                match complete_fifo_open(&path_owned, for_write) {
                    FifoOpenResult::Ready(_) => true,
                    _ => false,
                }
            });
            if other_end_ready {
                log::debug!("FIFO: Thread {} caught race - other end opened during block setup", thread_id);
                // Other end opened during the race window - unblock and complete
                crate::task::scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = false;
                        thread.set_ready();
                    }
                });
                // Fall through to complete the open below
            } else {
                // CRITICAL: Re-enable preemption before entering blocking loop!
                // The syscall handler called preempt_disable() at entry, but we need
                // to allow timer interrupts to schedule other threads while we're blocked.
                crate::per_cpu::preempt_enable();

                // HLT loop - wait for timer interrupt which will switch to another thread
                // When other end opens, add_reader/add_writer will call unblock(tid)
                loop {
                    // Check for pending signals that should interrupt this syscall
                    if let Some(e) = crate::syscall::check_signals_for_eintr() {
                        // Signal pending - clean up thread state and return EINTR
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
                        crate::per_cpu::preempt_disable();
                        log::debug!("handle_fifo_open: Thread {} interrupted by signal (EINTR)", thread_id);
                        return SyscallResult::Err(e as u64);
                    }

                    crate::task::scheduler::yield_current();
                    Cpu::halt_with_interrupts();

                    // Check if we were unblocked (thread state changed from Blocked)
                    let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.state == crate::task::thread::ThreadState::Blocked
                        } else {
                            false
                        }
                    }).unwrap_or(false);

                    if !still_blocked {
                        // CRITICAL: Disable preemption BEFORE breaking from HLT loop!
                        crate::per_cpu::preempt_disable();
                        log::debug!("FIFO: Thread {} woken from blocking", thread_id);
                        break;
                    }
                    // else: still blocked, continue HLT loop
                }

                // Clear blocked_in_syscall now that we're resuming normal syscall execution
                crate::task::scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = false;
                    }
                });
                // Reset quantum to prevent immediate preemption after long blocking wait
                #[cfg(target_arch = "x86_64")]
                crate::interrupts::timer::reset_quantum();
                #[cfg(target_arch = "aarch64")]
                {}
                crate::task::scheduler::check_and_clear_need_resched();
            }

            // Now complete the FIFO open
            match complete_fifo_open(&path_owned, for_write) {
                FifoOpenResult::Ready(buffer) => {
                    // Now ready - create fd
                    let kind = if for_write {
                        FdKind::FifoWrite(path_owned.clone(), buffer)
                    } else {
                        FdKind::FifoRead(path_owned.clone(), buffer)
                    };

                    let mut fd_entry = FileDescriptor::new(kind);
                    if nonblock {
                        fd_entry.status_flags |= status_flags::O_NONBLOCK;
                    }

                    let mut manager_guard = crate::process::manager();
                    let manager = match manager_guard.as_mut() {
                        Some(m) => m,
                        None => return SyscallResult::Err(3),
                    };

                    let (_, process) = match manager.find_process_by_thread_mut(thread_id) {
                        Some(p) => p,
                        None => return SyscallResult::Err(3),
                    };

                    match process.fd_table.alloc_with_entry(fd_entry) {
                        Ok(fd) => {
                            log::info!("handle_fifo_open: opened FIFO {} as fd {} ({})",
                                path_owned, fd, if for_write { "write" } else { "read" });
                            SyscallResult::Ok(fd as u64)
                        }
                        Err(_) => {
                            SyscallResult::Err(EMFILE as u64)
                        }
                    }
                }
                FifoOpenResult::Block => {
                    // Should not happen after being woken
                    log::error!("FIFO: Thread {} still blocked after wake!", thread_id);
                    SyscallResult::Err(11) // EAGAIN
                }
                FifoOpenResult::Error(errno) => {
                    SyscallResult::Err(errno as u64)
                }
            }
        }
        FifoOpenResult::Error(errno) => {
            log::debug!("handle_fifo_open: error {}", errno);
            SyscallResult::Err(errno as u64)
        }
    }
}

/// newfstatat(dirfd, pathname, statbuf, flags) - Get file status by path
///
/// Linux syscall 262. Supports AT_FDCWD (-100) as dirfd to stat relative
/// to the current working directory. Required by musl libc.
pub fn sys_newfstatat(dirfd: i32, pathname: u64, statbuf: u64, _flags: u32) -> SyscallResult {
    use super::errno::{EFAULT, ENOENT};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::ext2;

    const AT_FDCWD: i32 = -100;

    if statbuf == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }
    if pathname == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Read pathname from userspace
    let path = match copy_cstr_from_user(pathname) {
        Ok(s) => s,
        Err(e) => return SyscallResult::Err(e as u64),
    };

    // We only support AT_FDCWD for now
    if dirfd != AT_FDCWD && !path.starts_with('/') {
        // Relative paths with non-AT_FDCWD dirfd not yet supported
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }

    // Resolve the path to a full path (handle CWD for relative paths)
    let full_path = if path.starts_with('/') {
        path.clone()
    } else {
        // Get CWD from current process
        let cwd = get_current_cwd().unwrap_or_else(|| alloc::string::String::from("/"));
        if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, path)
        } else {
            alloc::format!("{}/{}", cwd, path)
        }
    };

    // Determine which filesystem to use
    let is_home = ext2::is_home_path(&full_path);
    let fs_path = if is_home { ext2::strip_home_prefix(&full_path) } else { &full_path };

    // Look up inode by path
    let (inode_num, mount_id) = if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(f) => f,
            None => return SyscallResult::Err(ENOENT as u64),
        };
        let mid = fs.mount_id;
        match fs.resolve_path(fs_path) {
            Ok(inum) => (inum as u64, mid),
            Err(_) => return SyscallResult::Err(ENOENT as u64),
        }
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(f) => f,
            None => return SyscallResult::Err(ENOENT as u64),
        };
        let mid = fs.mount_id;
        match fs.resolve_path(fs_path) {
            Ok(inum) => (inum as u64, mid),
            Err(_) => return SyscallResult::Err(ENOENT as u64),
        }
    };

    // Build stat from inode
    let mut stat = Stat::zeroed();
    stat.st_dev = mount_id as u64;
    stat.st_ino = inode_num;
    stat.st_blksize = 4096;
    stat.st_nlink = 1;
    stat.st_mode = S_IFREG | 0o644; // Default

    if let Some(inode_stat) = load_ext2_inode_stat_for_mount(inode_num, mount_id) {
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

    // Copy stat to userspace using raw pointer write
    // (Stat doesn't implement Copy, so we can't use copy_to_user)
    unsafe {
        let user_stat = statbuf as *mut Stat;
        core::ptr::write(user_stat, stat);
    }

    SyscallResult::Ok(0)
}

// =============================================================================
// *at syscall variants (Linux ARM64 uses these instead of legacy syscalls)
// =============================================================================
//
// ARM64 Linux has no open, mkdir, rmdir, link, unlink, symlink, readlink,
// mknod, rename, access. Instead it has *at variants that take a dirfd.
// These wrappers validate AT_FDCWD and delegate to the existing implementations.

/// AT_FDCWD: Use current working directory for relative paths
const AT_FDCWD: i32 = -100;
/// AT_REMOVEDIR flag for unlinkat (behave like rmdir)
const AT_REMOVEDIR: i32 = 0x200;

/// openat(dirfd, pathname, flags, mode) - replacement for open
pub fn sys_openat(dirfd: i32, pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_open(pathname, flags, mode)
}

/// faccessat(dirfd, pathname, mode, flags) - replacement for access
pub fn sys_faccessat(dirfd: i32, pathname: u64, mode: u32, _flags: u32) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_access(pathname, mode)
}

/// mkdirat(dirfd, pathname, mode) - replacement for mkdir
pub fn sys_mkdirat(dirfd: i32, pathname: u64, mode: u32) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_mkdir(pathname, mode)
}

/// mknodat(dirfd, pathname, mode, dev) - replacement for mknod
pub fn sys_mknodat(dirfd: i32, pathname: u64, mode: u32, dev: u64) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    super::fifo::sys_mknod(pathname, mode, dev)
}

/// unlinkat(dirfd, pathname, flags) - replacement for unlink and rmdir
///
/// If flags contains AT_REMOVEDIR, behaves like rmdir.
/// Otherwise behaves like unlink.
pub fn sys_unlinkat(dirfd: i32, pathname: u64, flags: i32) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    if (flags & AT_REMOVEDIR) != 0 {
        sys_rmdir(pathname)
    } else {
        sys_unlink(pathname)
    }
}

/// symlinkat(target, newdirfd, linkpath) - replacement for symlink
pub fn sys_symlinkat(target: u64, newdirfd: i32, linkpath: u64) -> SyscallResult {
    if newdirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_symlink(target, linkpath)
}

/// linkat(olddirfd, oldpath, newdirfd, newpath, flags) - replacement for link
pub fn sys_linkat(olddirfd: i32, oldpath: u64, newdirfd: i32, newpath: u64, _flags: i32) -> SyscallResult {
    if olddirfd != AT_FDCWD || newdirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_link(oldpath, newpath)
}

/// renameat(olddirfd, oldpath, newdirfd, newpath) - replacement for rename
pub fn sys_renameat(olddirfd: i32, oldpath: u64, newdirfd: i32, newpath: u64) -> SyscallResult {
    if olddirfd != AT_FDCWD || newdirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_rename(oldpath, newpath)
}

/// readlinkat(dirfd, pathname, buf, bufsiz) - replacement for readlink
pub fn sys_readlinkat(dirfd: i32, pathname: u64, buf: u64, bufsiz: u64) -> SyscallResult {
    if dirfd != AT_FDCWD {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }
    sys_readlink(pathname, buf, bufsiz)
}

// =============================================================================
// utimensat - Update file timestamps
// =============================================================================

/// Special timespec value: set timestamp to current time
const UTIME_NOW: i64 = 0x3FFFFFFF;
/// Special timespec value: leave timestamp unchanged
const UTIME_OMIT: i64 = 0x3FFFFFFE;
/// AT_SYMLINK_NOFOLLOW flag
#[allow(dead_code)]
const AT_SYMLINK_NOFOLLOW: u32 = 0x100;

/// Timespec layout for utimensat (matches Linux ABI)
#[repr(C)]
#[derive(Copy, Clone)]
struct UtimeTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

/// utimensat(dirfd, pathname, times, flags) - Update file timestamps
///
/// If pathname is NULL and dirfd is a valid fd: operate on that fd (futimens behavior).
/// If times is NULL: set atime and mtime to current time.
/// Otherwise: read two Timespec structs from userspace for atime and mtime.
/// UTIME_NOW (0x3FFFFFFF): use current time for that field.
/// UTIME_OMIT (0x3FFFFFFE): don't change that timestamp.
pub fn sys_utimensat(dirfd: i32, path_ptr: u64, times_ptr: u64, flags: u32) -> SyscallResult {
    use crate::fs::ext2;

    let now = crate::time::current_unix_time() as u32;

    // Determine what atime/mtime to set
    let (set_atime, set_mtime) = if times_ptr == 0 {
        // NULL times = set both to current time
        (Some(now), Some(now))
    } else {
        // Read two Timespec structs from userspace
        let times: [UtimeTimespec; 2] = match super::userptr::copy_from_user(times_ptr as *const [UtimeTimespec; 2]) {
            Ok(t) => t,
            Err(e) => return SyscallResult::Err(e),
        };

        let atime = if times[0].tv_nsec == UTIME_NOW {
            Some(now)
        } else if times[0].tv_nsec == UTIME_OMIT {
            None
        } else {
            Some(times[0].tv_sec as u32)
        };

        let mtime = if times[1].tv_nsec == UTIME_NOW {
            Some(now)
        } else if times[1].tv_nsec == UTIME_OMIT {
            None
        } else {
            Some(times[1].tv_sec as u32)
        };

        (atime, mtime)
    };

    // If both are OMIT, nothing to do
    if set_atime.is_none() && set_mtime.is_none() {
        return SyscallResult::Ok(0);
    }

    // Determine the target inode
    if path_ptr == 0 {
        // futimens behavior: operate on dirfd
        if dirfd < 0 {
            return SyscallResult::Err(super::errno::EBADF as u64);
        }

        let thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => return SyscallResult::Err(super::errno::EBADF as u64),
        };

        let fd_info = crate::arch_without_interrupts(|| {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    if let Some(fd_entry) = process.fd_table.get(dirfd) {
                        if let FdKind::RegularFile(file_ref) = &fd_entry.kind {
                            let file = file_ref.lock();
                            return Some((file.inode_num as u32, file.mount_id));
                        }
                    }
                }
            }
            None
        });

        let (inode_num, mount_id) = match fd_info {
            Some((ino, mid)) => (ino, mid),
            None => return SyscallResult::Err(super::errno::EBADF as u64),
        };

        return update_inode_timestamps(inode_num, mount_id, set_atime, set_mtime);
    }

    // Path-based: resolve path
    let path = match super::userptr::copy_cstr_from_user(path_ptr) {
        Ok(s) => s,
        Err(e) => return SyscallResult::Err(e as u64),
    };

    // Handle AT_FDCWD
    if dirfd != AT_FDCWD && !path.starts_with('/') {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }

    let full_path = if path.starts_with('/') {
        path.clone()
    } else {
        let cwd = get_current_cwd().unwrap_or_else(|| alloc::string::String::from("/"));
        if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, path)
        } else {
            alloc::format!("{}/{}", cwd, path)
        }
    };

    let is_home = ext2::is_home_path(&full_path);
    let fs_path = if is_home { ext2::strip_home_prefix(&full_path) } else { &full_path };
    let no_follow = (flags & AT_SYMLINK_NOFOLLOW) != 0;

    // Resolve path first (read lock), then update timestamps (write lock)
    let (inode_num, mount_id) = if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(f) => f,
            None => return SyscallResult::Err(super::errno::ENOENT as u64),
        };
        let ino = if no_follow {
            fs.resolve_path_no_follow(fs_path)
        } else {
            fs.resolve_path(fs_path)
        };
        match ino {
            Ok(n) => (n, fs.mount_id),
            Err(_) => return SyscallResult::Err(super::errno::ENOENT as u64),
        }
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = match fs_guard.as_ref() {
            Some(f) => f,
            None => return SyscallResult::Err(super::errno::ENOENT as u64),
        };
        let ino = if no_follow {
            fs.resolve_path_no_follow(fs_path)
        } else {
            fs.resolve_path(fs_path)
        };
        match ino {
            Ok(n) => (n, fs.mount_id),
            Err(_) => return SyscallResult::Err(super::errno::ENOENT as u64),
        }
    };

    update_inode_timestamps(inode_num, mount_id, set_atime, set_mtime)
}

/// Helper: update an inode's atime/mtime on the ext2 filesystem
fn update_inode_timestamps(
    inode_num: u32,
    mount_id: usize,
    set_atime: Option<u32>,
    set_mtime: Option<u32>,
) -> SyscallResult {
    use crate::fs::ext2;

    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);

    let do_update = |fs: &mut ext2::Ext2Fs| -> SyscallResult {
        let mut inode = match fs.read_inode(inode_num) {
            Ok(i) => i,
            Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
        };

        if let Some(atime) = set_atime {
            inode.i_atime = atime;
        }
        if let Some(mtime) = set_mtime {
            inode.i_mtime = mtime;
        }
        // Always update ctime when timestamps change
        inode.i_ctime = crate::time::current_unix_time() as u32;

        match fs.write_inode(inode_num, &inode) {
            Ok(()) => SyscallResult::Ok(0),
            Err(_) => SyscallResult::Err(super::errno::EIO as u64),
        }
    };

    if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => do_update(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => do_update(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    }
}

