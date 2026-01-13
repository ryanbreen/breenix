//! Filesystem-related syscalls
//!
//! Implements: open, lseek, fstat, getdents64

use crate::ipc::fd::FdKind;
use super::SyscallResult;

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

/// sys_open - Open a file or directory
///
/// # Arguments
/// * `pathname` - Path to the file (userspace pointer)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_DIRECTORY, etc.)
/// * `mode` - File creation mode (if O_CREAT)
///
/// # Returns
/// File descriptor on success, negative errno on failure
pub fn sys_open(pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EISDIR, EMFILE, ENOENT, ENOSPC, ENOTDIR};
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

    // Parse flags
    let want_creat = (flags & O_CREAT) != 0;
    let want_excl = (flags & O_EXCL) != 0;
    let want_trunc = (flags & O_TRUNC) != 0;
    let wants_directory = (flags & O_DIRECTORY) != 0;

    // Get the mutable filesystem guard since we might need to create files
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_open: ext2 root filesystem not mounted");
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Try to resolve the path to an inode number
    let resolve_result = fs.resolve_path(&path);

    // Handle O_CREAT flag
    let (inode_num, file_created) = match resolve_result {
        Ok(ino) => {
            // File exists
            if want_creat && want_excl {
                // O_CREAT | O_EXCL - fail if file exists
                log::debug!("sys_open: file exists and O_EXCL set");
                return SyscallResult::Err(EEXIST as u64);
            }
            (ino, false)
        }
        Err(e) => {
            if e.contains("not found") && want_creat {
                // File doesn't exist and O_CREAT is set - create it
                log::debug!("sys_open: creating new file {}", path);

                // Parse parent directory and filename
                let (parent_path, filename) = match path.rfind('/') {
                    Some(0) => ("/", &path[1..]), // File in root directory
                    Some(idx) => (&path[..idx], &path[idx + 1..]),
                    None => {
                        log::error!("sys_open: invalid path format");
                        return SyscallResult::Err(ENOENT as u64);
                    }
                };

                // Validate filename
                if filename.is_empty() {
                    log::error!("sys_open: empty filename");
                    return SyscallResult::Err(ENOENT as u64);
                }

                // Resolve parent directory
                let parent_inode = match fs.resolve_path(parent_path) {
                    Ok(ino) => ino,
                    Err(_) => {
                        log::error!("sys_open: parent directory not found: {}", parent_path);
                        return SyscallResult::Err(ENOENT as u64);
                    }
                };

                // Verify parent is a directory
                let parent = match fs.read_inode(parent_inode) {
                    Ok(ino) => ino,
                    Err(_) => {
                        log::error!("sys_open: failed to read parent inode");
                        return SyscallResult::Err(5); // EIO
                    }
                };
                if !parent.is_dir() {
                    return SyscallResult::Err(ENOTDIR as u64);
                }

                // Create the file with the given mode
                // Default mode is 0o644 if mode is 0
                let file_mode = if mode == 0 { 0o644 } else { (mode & 0o777) as u16 };
                match fs.create_file(parent_inode, filename, file_mode) {
                    Ok(new_inode) => {
                        log::info!("sys_open: created file {} with inode {}", path, new_inode);
                        (new_inode, true)
                    }
                    Err(e) => {
                        log::error!("sys_open: failed to create file: {}", e);
                        if e.contains("No free inodes") || e.contains("No space") {
                            return SyscallResult::Err(ENOSPC as u64);
                        }
                        return SyscallResult::Err(5); // EIO
                    }
                }
            } else {
                // File doesn't exist and O_CREAT not set, or other error
                log::debug!("sys_open: path resolution failed: {}", e);
                if e.contains("not found") {
                    return SyscallResult::Err(ENOENT as u64);
                } else if e.contains("Not a directory") {
                    return SyscallResult::Err(ENOTDIR as u64);
                } else {
                    return SyscallResult::Err(5); // EIO
                }
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

    let file_type = inode.file_type();
    let is_directory = matches!(file_type, Ext2FileType::Directory);
    let is_regular = matches!(file_type, Ext2FileType::Regular);

    // Handle O_TRUNC for regular files
    if want_trunc && is_regular && !file_created {
        // Only truncate if file existed (not just created)
        log::debug!("sys_open: truncating file inode {}", inode_num);
        if let Err(e) = fs.truncate_file(inode_num) {
            log::error!("sys_open: failed to truncate file: {}", e);
            return SyscallResult::Err(5); // EIO
        }
    }

    let mount_id = fs.mount_id;

    // Drop the filesystem lock before acquiring process lock
    drop(fs_guard);

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
                    let file_size = match get_ext2_file_size(file.inode_num) {
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
        FdKind::Directory(dir) => {
            let dir_guard = dir.lock();
            stat.st_dev = dir_guard.mount_id as u64;
            stat.st_ino = dir_guard.inode_num;
            stat.st_mode = S_IFDIR | 0o755; // Directory with rwxr-xr-x (default)
            stat.st_nlink = 2; // . and ..

            // Try to load inode metadata from ext2 filesystem
            if let Some(inode_stat) = load_ext2_inode_stat(dir_guard.inode_num) {
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
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) | FdKind::TcpConnection(_) => {
            // TCP sockets
            static TCP_SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(3000);
            stat.st_dev = 0;
            stat.st_ino = TCP_SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755; // Socket with rwxr-xr-x
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

/// Get file size from ext2 inode
///
/// Returns None if the ext2 filesystem is not available or inode cannot be read.
fn get_ext2_file_size(inode_num: u64) -> Option<u64> {
    use crate::fs::ext2;

    // Get the mounted root filesystem
    let fs_guard = ext2::root_fs();
    let fs = fs_guard.as_ref()?;

    // Read the inode using the cached filesystem
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

    // Must be a directory fd
    let dir_file = match &fd_entry.kind {
        FdKind::Directory(dir) => dir.clone(),
        _ => return SyscallResult::Err(ENOTDIR as u64),
    };

    // Get directory info
    let dir_guard = dir_file.lock();
    let inode_num = dir_guard.inode_num;
    let start_position = dir_guard.position;
    drop(dir_guard);

    // Drop process manager lock before acquiring filesystem lock
    drop(manager_guard);

    // Read directory data from ext2
    let fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => {
            log::error!("sys_getdents64: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Read the directory inode
    let inode = match fs.read_inode(inode_num as u32) {
        Ok(ino) => ino,
        Err(_) => {
            log::error!("sys_getdents64: failed to read inode {}", inode_num);
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Read directory data
    let dir_data = match fs.read_directory(&inode) {
        Ok(data) => data,
        Err(e) => {
            log::error!("sys_getdents64: failed to read directory: {}", e);
            return SyscallResult::Err(EIO as u64);
        }
    };

    drop(fs_guard);

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
    let path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    log::debug!("sys_unlink: path={:?}", path);

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_unlink: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Perform the unlink operation
    match fs.unlink_file(&path) {
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

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_rename: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Perform the rename operation
    match fs.rename_file(&old, &new) {
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

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_rmdir: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Perform the rmdir operation
    match fs.remove_directory(&path) {
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

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_link: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Perform the hard link operation
    match fs.create_hard_link(&old, &new) {
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

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_mkdir: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Create the directory
    // Default mode is 0o755 if mode is 0 (common convention)
    let dir_mode = if mode == 0 { 0o755 } else { (mode & 0o777) as u16 };
    match fs.create_directory(&path, dir_mode) {
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

    // Get the root filesystem (with mutable access)
    let mut fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_mut() {
        Some(fs) => fs,
        None => {
            log::error!("sys_symlink: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Create the symbolic link
    match fs.create_symlink(&target_str, &linkpath_str) {
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

    // Get the root filesystem
    let fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => {
            log::error!("sys_readlink: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Resolve path to inode number (don't follow the symlink itself)
    let inode_num = match fs.resolve_path(&path) {
        Ok(ino) => ino,
        Err(e) => {
            log::debug!("sys_readlink: path resolution failed: {}", e);
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Read the symlink target
    let target = match fs.read_symlink(inode_num) {
        Ok(t) => t,
        Err(e) => {
            log::debug!("sys_readlink: failed to read symlink: {}", e);
            let errno = if e.contains("Not a symbolic link") {
                EINVAL
            } else if e.contains("not found") {
                ENOENT
            } else {
                EIO
            };
            return SyscallResult::Err(errno as u64);
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

    // Get the root filesystem
    let fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => {
            log::error!("sys_access: ext2 root filesystem not mounted");
            return SyscallResult::Err(ENOENT as u64);
        }
    };

    // Try to resolve the path to an inode number
    let inode_num = match fs.resolve_path(&path) {
        Ok(ino) => ino,
        Err(e) => {
            log::debug!("sys_access: path resolution failed: {}", e);
            let errno = if e.contains("not found") {
                ENOENT
            } else if e.contains("Not a directory") {
                ENOTDIR
            } else {
                5 // EIO
            };
            return SyscallResult::Err(errno as u64);
        }
    };

    // F_OK just checks existence - we already found it
    if mode == F_OK {
        log::debug!("sys_access: file exists, F_OK check passed");
        return SyscallResult::Ok(0);
    }

    // Read the inode to check permissions
    let inode = match fs.read_inode(inode_num) {
        Ok(ino) => ino,
        Err(_) => {
            log::error!("sys_access: failed to read inode {}", inode_num);
            return SyscallResult::Err(5); // EIO
        }
    };

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
/// * `_flags` - Open flags (currently unused for devices)
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_devfs_open(device_name: &str, _flags: u32) -> SyscallResult {
    use super::errno::{EMFILE, ENOENT};
    use crate::fs::devfs;

    log::debug!("handle_devfs_open: device_name={:?}", device_name);

    // Look up the device
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
    SyscallResult::Ok(buf)
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

    // Get the root filesystem
    let fs_guard = ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => {
            log::error!("sys_chdir: ext2 root filesystem not mounted");
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Resolve the path to an inode number
    let inode_num = match fs.resolve_path(&normalized) {
        Ok(ino) => ino,
        Err(e) => {
            log::debug!("sys_chdir: path resolution failed: {}", e);
            let errno = if e.contains("not found") {
                ENOENT
            } else if e.contains("Not a directory") {
                ENOTDIR
            } else if e.contains("permission") {
                EACCES
            } else {
                EIO
            };
            return SyscallResult::Err(errno as u64);
        }
    };

    // Read the inode to verify it's a directory
    let inode = match fs.read_inode(inode_num) {
        Ok(ino) => ino,
        Err(_) => {
            log::error!("sys_chdir: failed to read inode {}", inode_num);
            return SyscallResult::Err(EIO as u64);
        }
    };

    // Check if it's a directory
    if !matches!(inode.file_type(), Ext2FileType::Directory) {
        log::debug!("sys_chdir: {} is not a directory", normalized);
        return SyscallResult::Err(ENOTDIR as u64);
    }

    // Drop filesystem lock before getting process lock
    drop(fs_guard);

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
fn normalize_path(path: &str) -> alloc::string::String {
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
fn get_current_cwd() -> Option<alloc::string::String> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let manager_guard = crate::process::manager();
    match &*manager_guard {
        Some(manager) => manager
            .find_process_by_thread(thread_id)
            .map(|(_, p)| p.cwd.clone()),
        None => None,
    }
}
