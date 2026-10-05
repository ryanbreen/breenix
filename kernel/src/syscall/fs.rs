//! Filesystem-related syscalls
//!
//! Implements: open, lseek, fstat, getdents64

use super::SyscallResult;
use crate::arch_impl::traits::CpuOps;
use crate::ipc::fd::FdKind;

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
/// O_NOFOLLOW - fail with ELOOP rather than follow a final symlink. The Linux
/// value differs by architecture.
#[cfg(target_arch = "x86_64")]
pub const O_NOFOLLOW: u32 = 0x20000;
#[cfg(target_arch = "aarch64")]
pub const O_NOFOLLOW: u32 = 0x8000;

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
pub const S_IFMT: u32 = 0o170000; // File type mask
pub const S_IFSOCK: u32 = 0o140000; // Socket
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFLNK: u32 = 0o120000; // Symbolic link
pub const S_IFREG: u32 = 0o100000; // Regular file
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFBLK: u32 = 0o060000; // Block device
#[allow(dead_code)] // Part of POSIX stat API
pub const S_IFDIR: u32 = 0o040000; // Directory
pub const S_IFCHR: u32 = 0o020000; // Character device
pub const S_IFIFO: u32 = 0o010000; // FIFO (pipe)

/// stat structure (Linux x86_64 compatible - 144 bytes)
///
/// x86_64 and aarch64 Linux have different struct stat layouts.
/// x86_64 uses the layout from arch/x86/include/uapi/asm/stat.h (144 bytes).
/// aarch64 uses the generic layout from include/uapi/asm-generic/stat.h (128 bytes).
/// Key differences: field order (nlink/mode swapped), nlink width (u64 vs u32),
/// blksize width (i64 vs i32), and trailing reserved space (24 vs 8 bytes).
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
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

#[cfg(target_arch = "x86_64")]
const _: () = assert!(core::mem::size_of::<Stat>() == 144);

#[cfg(target_arch = "x86_64")]
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

/// stat structure (Linux aarch64 compatible - 128 bytes)
///
/// Layout from include/uapi/asm-generic/stat.h used by aarch64 Linux.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    _pad1: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    _pad2: i32,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    _reserved: [u32; 2],
}

#[cfg(target_arch = "aarch64")]
const _: () = assert!(core::mem::size_of::<Stat>() == 128);

#[cfg(target_arch = "aarch64")]
impl Stat {
    /// Create a zeroed Stat structure
    pub fn zeroed() -> Self {
        Self {
            st_dev: 0,
            st_ino: 0,
            st_mode: 0,
            st_nlink: 0,
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            _pad1: 0,
            st_size: 0,
            st_blksize: 0,
            _pad2: 0,
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
            _reserved: [0; 2],
        }
    }
}

use crate::fs::permissions::Credentials as FileCredentials;

fn current_file_credentials() -> FileCredentials {
    FileCredentials::current(false)
}

/// POSIX open access check: the access mode (and O_TRUNC) must be granted by
/// the owner, group or other permission bits that apply to the caller. Root
/// is granted read and write regardless of the mode bits.
fn check_open_access(
    inode: &crate::fs::ext2::Ext2Inode,
    flags: u32,
    cred: &FileCredentials,
) -> Result<(), SyscallResult> {
    use super::errno::EACCES;

    if cred.euid == 0 {
        return Ok(());
    }
    let mut wanted = match flags & 0x3 {
        O_RDONLY => 0o4,
        O_WRONLY => 0o2,
        _ => 0o6,
    };
    if flags & O_TRUNC != 0 {
        wanted |= 0o2;
    }
    if cred.permits(inode, wanted) {
        Ok(())
    } else {
        Err(SyscallResult::Err(EACCES as u64))
    }
}

/// Whether an open of this file type reaches the permission check. A
/// directory opened for writing fails with EISDIR, and other non-regular
/// types fail later in sys_open, so only these opens are checked here.
fn open_is_access_checked(is_reg: bool, is_dir: bool, flags: u32) -> bool {
    is_reg || (is_dir && flags & 0x3 == O_RDONLY)
}

/// Copy a pathname from userspace and resolve it; `follow` says whether a
/// final symlink is followed.
fn resolve_user(pathname: u64, follow: bool) -> Result<crate::fs::namei::Resolved, SyscallResult> {
    let path = super::userptr::copy_cstr_from_user(pathname).map_err(SyscallResult::Err)?;
    crate::fs::namei::resolve(&path, follow).map_err(SyscallResult::Err)
}

/// Copy a pathname from userspace and resolve it to the directory entry an
/// unlink, rmdir or rename acts on.
fn resolve_user_entry(pathname: u64) -> Result<crate::fs::namei::Resolved, SyscallResult> {
    let path = super::userptr::copy_cstr_from_user(pathname).map_err(SyscallResult::Err)?;
    crate::fs::namei::resolve_entry(&path).map_err(SyscallResult::Err)
}

/// Copy a pathname from userspace and resolve it to the directory entry a
/// rename acts on, holding the directory the entry is in.
fn resolve_user_rename(pathname: u64) -> Result<crate::fs::namei::Resolved, SyscallResult> {
    let path = super::userptr::copy_cstr_from_user(pathname).map_err(SyscallResult::Err)?;
    crate::fs::namei::resolve_rename(&path).map_err(SyscallResult::Err)
}

/// Copy a pathname from userspace and resolve it to the name a mkdir,
/// symlink or link creates.
fn resolve_user_create(pathname: u64) -> Result<crate::fs::namei::Resolved, SyscallResult> {
    let path = super::userptr::copy_cstr_from_user(pathname).map_err(SyscallResult::Err)?;
    crate::fs::namei::resolve_create(&path).map_err(SyscallResult::Err)
}

/// sys_open - Open a file or directory
///
/// Helper: sys_open write path (O_CREAT/O_TRUNC) — works on any Ext2Fs instance.
/// Returns (inode_num, file_type, is_directory, is_regular, mount_id) or Err.
fn sys_open_write_path(
    fs: &mut crate::fs::ext2::Ext2Fs,
    resolved: &crate::fs::namei::Resolved,
    flags: u32,
    mode: u32,
    cred: FileCredentials,
) -> Result<(u32, crate::fs::ext2::FileType, bool, bool, usize, Option<crate::fs::ext2::live_inode::FileHandle>), SyscallResult> {
    use super::errno::{EEXIST, EISDIR, ENOENT, ENOSPC, ENOTDIR};
    use crate::fs::ext2::FileType as Ext2FileType;
    use crate::fs::namei::Target;

    let want_creat = (flags & O_CREAT) != 0;
    let want_excl = (flags & O_EXCL) != 0;
    let want_trunc = (flags & O_TRUNC) != 0;

    let (ino, file_created) = match resolved.target {
        Target::Inode { ino, .. } => {
            if want_creat && want_excl {
                log::debug!("sys_open: file exists and O_EXCL set");
                return Err(SyscallResult::Err(EEXIST as u64));
            }
            (ino, false)
        }
        Target::Absent { parent, .. } => {
            if !want_creat {
                return Err(SyscallResult::Err(ENOENT as u64));
            }
            if resolved.trailing_slash {
                return Err(SyscallResult::Err(EISDIR as u64));
            }
            let filename = resolved.path.rsplit('/').next().unwrap_or("");
            let parent_inode = match fs.read_inode(parent) {
                Ok(ino) => ino,
                Err(_) => {
                    log::error!("sys_open: failed to read parent inode");
                    return Err(SyscallResult::Err(5)); // EIO
                }
            };
            if !parent_inode.is_dir() {
                return Err(SyscallResult::Err(ENOTDIR as u64));
            }
            // Another creator may have won since the pathname was resolved.
            match fs.lookup_in_dir(&parent_inode, filename) {
                Ok(Some(_)) if want_excl => return Err(SyscallResult::Err(EEXIST as u64)),
                Ok(Some(ino)) => (ino, false),
                Ok(None) => {
                    log::debug!("sys_open: creating new file {}", resolved.path);
                    // The requested mode is used as given, less the umask: mode 0
                    // creates a file nobody but root may open.
                    let file_mode = (mode & 0o7777 & !cred.umask) as u16;
                    // The new file belongs to its creator (Linux: the effective
                    // uid and gid), so a restrictive mode still lets it reopen it.
                    match fs.create_file(
                        parent,
                        filename,
                        file_mode,
                        cred.euid,
                        cred.egid,
                    ) {
                        Ok(new_inode) => {
                            log::info!(
                                "sys_open: created file {} with inode {}",
                                resolved.path,
                                new_inode
                            );
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
                }
                Err(_) => return Err(SyscallResult::Err(5)), // EIO
            }
        }
        Target::Virtual => return Err(SyscallResult::Err(ENOENT as u64)),
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

    // A file this open created is opened with the requested access whatever
    // its new mode; an existing one must grant that access first.
    if !file_created && open_is_access_checked(is_reg, is_dir, flags) {
        check_open_access(&inode, flags, &cred)?;
    }

    if want_trunc && is_reg && !file_created {
        log::debug!("sys_open: truncating file inode {}", ino);
        let result = if cred.euid == 0 { fs.truncate_file(ino) } else { fs.resize_file_as(ino, 0, true) };
        if let Err(e) = result {
            log::error!("sys_open: failed to truncate file: {}", e);
            return Err(SyscallResult::Err(5)); // EIO
        }
    }

    let mid = fs.mount_id;
    let handle = if is_reg || is_dir {
        Some(fs.pin_loaded_inode(ino, if want_trunc && !file_created { 0 } else { inode.size() })
            .map_err(|_| SyscallResult::Err(super::errno::EIO as u64))?)
    } else { None };
    Ok((ino, ft, is_dir, is_reg, mid, handle))
}

/// Helper: sys_open read path — works on any Ext2Fs instance.
fn sys_open_read_path(
    fs: &crate::fs::ext2::Ext2Fs,
    resolved: &crate::fs::namei::Resolved,
    flags: u32,
    cred: FileCredentials,
) -> Result<(u32, crate::fs::ext2::FileType, bool, bool, usize, Option<crate::fs::ext2::live_inode::FileHandle>), SyscallResult> {
    use crate::fs::ext2::FileType as Ext2FileType;
    use crate::fs::namei::Target;

    let ino = match resolved.target {
        Target::Inode { ino, .. } => ino,
        _ => return Err(SyscallResult::Err(super::errno::ENOENT as u64)),
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
    if open_is_access_checked(is_reg, is_dir, flags) {
        check_open_access(&inode, flags, &cred)?;
    }
    let mid = fs.mount_id;
    let handle = if is_reg || is_dir {
        Some(fs.pin_loaded_inode(ino, inode.size())
            .map_err(|_| SyscallResult::Err(super::errno::EIO as u64))?)
    } else { None };
    Ok((ino, ft, is_dir, is_reg, mid, handle))
}

/// # Arguments
/// * `pathname` - Path to the file (userspace pointer)
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_DIRECTORY, etc.)
/// * `mode` - File creation mode (if O_CREAT)
///
/// # Returns
/// File descriptor on success, negative errno on failure
pub fn sys_open(pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EISDIR, ELOOP, EMFILE, ENOENT, ENOTDIR};
    use crate::fs::ext2::FileType as Ext2FileType;
    use crate::fs::namei::Target;
    use crate::ipc::fd::{DirectoryFile, FileDescriptor, RegularFile};
    use alloc::sync::Arc;
    use spin::Mutex;

    let want_creat = (flags & O_CREAT) != 0;
    let want_excl = (flags & O_EXCL) != 0;
    // O_NOFOLLOW, and O_CREAT with O_EXCL, act on a final symlink itself.
    let follow = (flags & O_NOFOLLOW) == 0 && !(want_creat && want_excl);
    let resolved = match resolve_user(pathname, follow) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let path = resolved.path.as_str();

    log::debug!(
        "sys_open: path={:?}, flags={:#x}, mode={:#o}",
        path,
        flags,
        mode
    );

    if resolved.target == Target::Virtual {
        if path == "/dev" {
            return handle_devfs_directory_open(flags);
        }
        if let Some(device_name) = path.strip_prefix("/dev/") {
            return handle_devfs_open(device_name, flags);
        }
        return handle_procfs_open(path, flags);
    }

    // Check if this is a FIFO (named pipe)
    if let Some(entry) = crate::ipc::fifo::FIFO_REGISTRY.get(path) {
        let cred = current_file_credentials();
        if let Err(error) = check_open_access(&entry.lock().inode(), flags, &cred) {
            return error;
        }
        return handle_fifo_open(path, flags, entry);
    }

    let mount = match resolved.target {
        // Only an unfollowed final symlink resolves to one.
        Target::Inode {
            file_type: Ext2FileType::SymLink,
            ..
        } => {
            let errno = if want_creat && want_excl { EEXIST } else { ELOOP };
            return SyscallResult::Err(errno as u64);
        }
        Target::Inode { mount, .. } | Target::Absent { mount, .. } => mount,
        Target::Virtual => return SyscallResult::Err(ENOENT as u64),
    };

    // Parse flags
    let want_trunc = (flags & O_TRUNC) != 0;
    let wants_directory = (flags & O_DIRECTORY) != 0;

    // Use read lock for non-modifying opens, write lock only for O_CREAT/O_TRUNC.
    // This allows concurrent exec, file reads, and directory listings without
    // being blocked by another process creating or writing files.
    let needs_write = want_creat || want_trunc;
    // Read before taking a filesystem lock; the process manager is not
    // acquired under one.
    let cred = current_file_credentials();

    let result = if needs_write {
        // === WRITE PATH: O_CREAT or O_TRUNC requires exclusive filesystem access ===
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => sys_open_write_path(fs, &resolved, flags, mode, cred),
            None => {
                log::error!("sys_open: ext2 filesystem not mounted");
                return SyscallResult::Err(ENOENT as u64);
            }
        }
    } else {
        // === READ PATH: No filesystem modification needed, use shared read lock ===
        let fs_guard = mount.read();
        match fs_guard.as_ref() {
            Some(fs) => sys_open_read_path(fs, &resolved, flags, cred),
            None => {
                log::error!("sys_open: ext2 filesystem not mounted");
                return SyscallResult::Err(ENOENT as u64);
            }
        }
    };
    let (inode_num, file_type, is_directory, _, mount_id, handle) = match result {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Handle directory vs file cases
    if is_directory {
        if (flags & 0x3) == O_RDONLY {
            // Read-only, for getdents. A directory opened for writing fails with
            // EISDIR below whether or not O_DIRECTORY is given, as on Linux; the
            // permission check relies on that.
            // Create DirectoryFile structure
            let dir_file = DirectoryFile {
                handle: match handle {
                    Some(handle) => handle,
                    None => return SyscallResult::Err(super::errno::EIO as u64),
                },
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
            let fd_entry =
                FileDescriptor::opened(FdKind::Directory(Arc::new(Mutex::new(dir_file))), flags);
            match process.fd_table.alloc_with_entry(fd_entry) {
                Ok(fd) => {
                    log::info!(
                        "sys_open: opened directory {} as fd {} (inode {})",
                        path,
                        fd,
                        inode_num
                    );
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
        log::debug!(
            "sys_open: {} is not a directory (O_DIRECTORY specified)",
            path
        );
        return SyscallResult::Err(ENOTDIR as u64);
    } else if !matches!(file_type, Ext2FileType::Regular) {
        // Not a regular file and not a directory
        log::debug!(
            "sys_open: {} is not a regular file (type: {:?})",
            path,
            file_type
        );
        return SyscallResult::Err(EACCES as u64);
    } else {
        // Regular file
        // Create RegularFile structure
        let regular_file = RegularFile {
            handle: handle.expect("regular inode pinned during open"),
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

        // Allocate a descriptor for the new open file description, carrying
        // its access mode, status flags and O_CLOEXEC from the open flags.
        let fd_entry =
            FileDescriptor::opened(FdKind::RegularFile(Arc::new(Mutex::new(regular_file))), flags);
        match process.fd_table.alloc_with_entry(fd_entry) {
            Ok(fd) => {
                log::info!(
                    "sys_open: opened {} as fd {} (inode {})",
                    path,
                    fd,
                    inode_num
                );
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

    // SEEK_END requires a disk read to get the file size.  We MUST NOT hold the
    // process manager lock (manager_guard) or the file lock during that I/O,
    // because the dispatch path calls set_next_ttbr0_for_thread() which tries
    // to acquire the PM lock.  If we are holding it while blocked on I/O, every
    // attempt to re-dispatch us returns PmLockBusy and we spin forever.
    //
    // Strategy: extract the inode/mount info under the lock, drop all locks,
    // do the disk read unlocked, then re-acquire to update the position.
    if whence == SEEK_END {
        // Phase 1: extract file metadata under PM lock
        let (handle, current_position) = {
            let mut manager_guard = crate::process::manager();
            let process = match &mut *manager_guard {
                Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
                    Some((_, p)) => p,
                    None => return SyscallResult::Err(3), // ESRCH
                },
                None => return SyscallResult::Err(3), // ESRCH
            };
            let fd_entry = match process.fd_table.get(fd) {
                Some(entry) => entry,
                None => return SyscallResult::Err(9), // EBADF
            };
            match &fd_entry.kind {
                FdKind::RegularFile(file) => {
                    let f = file.lock();
                    (f.handle.clone(), f.position)
                }
                FdKind::Directory(_) => return SyscallResult::Err(21), // EISDIR
                _ => return SyscallResult::Err(29),                    // ESPIPE
            }
            // PM lock and file lock both dropped here
        };

        // Phase 2: disk read WITHOUT any lock held
        let file_size = match get_ext2_file_size_for_handle(&handle) {
            Some(size) => size as i64,
            None => {
                log::error!("sys_lseek: cannot get pinned inode size");
                return SyscallResult::Err(5); // EIO
            }
        };
        let new_position = match file_size.checked_add(offset) {
            Some(position) if position >= 0 => position,
            _ => return SyscallResult::Err(22), // EINVAL
        };
        let new_pos = new_position as u64;
        let _ = current_position; // not needed, position updated below

        // Phase 3: update position under PM lock
        let mut manager_guard = crate::process::manager();
        let process = match &mut *manager_guard {
            Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
                Some((_, p)) => p,
                None => return SyscallResult::Err(3), // ESRCH
            },
            None => return SyscallResult::Err(3), // ESRCH
        };
        let fd_entry = match process.fd_table.get(fd) {
            Some(entry) => entry,
            None => return SyscallResult::Err(9), // EBADF
        };
        match &fd_entry.kind {
            FdKind::RegularFile(file) => {
                file.lock().position = new_pos;
                return SyscallResult::Ok(new_pos);
            }
            _ => return SyscallResult::Err(29), // ESPIPE
        }
    }

    // SEEK_SET and SEEK_CUR: no disk I/O needed, handle under lock.
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
                SEEK_SET => Some(offset),
                SEEK_CUR => (file.position as i64).checked_add(offset),
                _ => return SyscallResult::Err(22), // EINVAL
            };
            // A resulting offset that is negative or overflows is EINVAL.
            let new_pos = match new_pos {
                Some(position) if position >= 0 => position as u64,
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
    use super::userptr::copy_to_user;

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

    // Describe what kind of fstat this is, extracting only the data we need
    // under the PM lock.  We must NOT hold the PM lock while doing ext2 disk
    // I/O: on ARM64 the PM lock disables ALL IRQs, and AHCI completions arrive
    // as interrupts — holding the lock during disk I/O deadlocks the system.
    enum FstatKind {
        StdIo(i32),
        Pipe,
        UdpSocket,
        RegularFile { inode_num: u64, mount_id: usize, handle: crate::fs::ext2::live_inode::FileHandle },
        Directory { inode_num: u64, mount_id: usize },
        Device { inode: u64, rdev: u64 },
        DevfsDirectory,
        DevptsDirectory,
        TcpSocket,
        PtyDevice { pty_num: u32 },
        UnixSocket,
        Fifo(alloc::sync::Arc<spin::Mutex<crate::ipc::fifo::FifoEntry>>),
        ProcfsFile { size: i64 },
        ProcfsDirectory,
        Epoll,
    }

    let kind = {
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

        match &fd_entry.kind {
            FdKind::StdIo(io_fd) => FstatKind::StdIo(*io_fd),
            FdKind::PipeRead(_) | FdKind::PipeWrite(_) => FstatKind::Pipe,
            FdKind::UdpSocket(_) => FstatKind::UdpSocket,
            FdKind::RegularFile(file) => {
                let file_guard = file.lock();
                FstatKind::RegularFile {
                    inode_num: file_guard.inode_num,
                    mount_id: file_guard.mount_id,
                    handle: file_guard.handle.clone(),
                }
            }
            FdKind::Directory(dir) => {
                let dir_guard = dir.lock();
                FstatKind::Directory {
                    inode_num: dir_guard.inode_num,
                    mount_id: dir_guard.mount_id,
                }
            }
            FdKind::Device(device_type) => {
                use crate::fs::devfs;
                let device_node = devfs::lookup_by_inode(device_type.inode());
                FstatKind::Device {
                    inode: device_type.inode(),
                    rdev: device_node.map(|d| d.rdev()).unwrap_or(0),
                }
            }
            FdKind::DevfsDirectory { .. } => FstatKind::DevfsDirectory,
            FdKind::DevptsDirectory { .. } => FstatKind::DevptsDirectory,
            FdKind::TcpSocket(_) | FdKind::TcpListener(_) | FdKind::TcpConnection(_) => {
                FstatKind::TcpSocket
            }
            FdKind::PtyMaster(pty_num) | FdKind::PtySlave(pty_num) => {
                FstatKind::PtyDevice { pty_num: *pty_num }
            }
            FdKind::UnixStream(_) | FdKind::UnixSocket(_) | FdKind::UnixListener(_) => {
                FstatKind::UnixSocket
            }
            FdKind::FifoRead(_, _, entry) | FdKind::FifoWrite(_, _, entry) => FstatKind::Fifo(entry.clone()),
            FdKind::ProcfsFile { ref content, .. } => FstatKind::ProcfsFile {
                size: content.len() as i64,
            },
            FdKind::ProcfsDirectory { .. } => FstatKind::ProcfsDirectory,
            FdKind::Epoll(_) => FstatKind::Epoll,
        }
        // manager_guard drops here — PM lock released, IRQs restored
    };

    // Now build the stat structure.  For ext2 files/directories, disk I/O
    // happens here with the PM lock fully released and IRQs enabled.
    let mut stat = Stat::zeroed();
    stat.st_blksize = 4096; // Standard block size

    match kind {
        FstatKind::StdIo(io_fd) => {
            // stdin/stdout/stderr are character devices (TTY)
            stat.st_dev = 0;
            stat.st_ino = (io_fd + 1) as u64; // Use fd+1 as pseudo-inode
            stat.st_mode = S_IFCHR | 0o666; // Character device with rw-rw-rw-
            stat.st_nlink = 1;
            stat.st_rdev = make_dev(5, io_fd as u64); // Major 5 (TTY), minor = fd number
        }
        FstatKind::Pipe => {
            static PIPE_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(1000);
            stat.st_dev = 0;
            stat.st_ino = PIPE_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFIFO | 0o600; // FIFO with rw-------
            stat.st_nlink = 1;
            stat.st_size = 0;
        }
        FstatKind::UdpSocket => {
            static SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(2000);
            stat.st_dev = 0;
            stat.st_ino = SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755;
            stat.st_nlink = 1;
        }
        FstatKind::RegularFile {
            inode_num,
            mount_id,
            handle,
        } => {
            // Disk I/O happens here — PM lock is NOT held.
            stat.st_dev = mount_id as u64;
            stat.st_ino = inode_num;
            stat.st_mode = S_IFREG | 0o644;
            stat.st_nlink = 1;
            let inode_stat = match load_ext2_inode_stat_for_handle(&handle) {
                Some(stat) => stat,
                None => return SyscallResult::Err(super::errno::EIO as u64),
            };
            {
                stat.st_mode = inode_stat.mode;
                stat.st_uid = inode_stat.uid;
                stat.st_gid = inode_stat.gid;
                stat.st_size = inode_stat.size;
                stat.st_nlink = inode_stat.nlink as _;
                stat.st_atime = inode_stat.atime;
                stat.st_mtime = inode_stat.mtime;
                stat.st_ctime = inode_stat.ctime;
                stat.st_blocks = inode_stat.blocks;
            }
        }
        FstatKind::Directory {
            inode_num,
            mount_id,
        } => {
            // Disk I/O happens here — PM lock is NOT held.
            stat.st_dev = mount_id as u64;
            stat.st_ino = inode_num;
            stat.st_mode = S_IFDIR | 0o755;
            stat.st_nlink = 2;
            if let Some(inode_stat) = load_ext2_inode_stat_for_mount(inode_num, mount_id) {
                stat.st_mode = inode_stat.mode;
                stat.st_uid = inode_stat.uid;
                stat.st_gid = inode_stat.gid;
                stat.st_size = inode_stat.size;
                stat.st_nlink = inode_stat.nlink as _;
                stat.st_atime = inode_stat.atime;
                stat.st_mtime = inode_stat.mtime;
                stat.st_ctime = inode_stat.ctime;
                stat.st_blocks = inode_stat.blocks;
            }
        }
        FstatKind::Device { inode, rdev } => {
            stat.st_dev = 0;
            stat.st_ino = inode;
            stat.st_mode = S_IFCHR | 0o666;
            stat.st_nlink = 1;
            stat.st_rdev = rdev;
        }
        FstatKind::DevfsDirectory => {
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFDIR | 0o755;
            stat.st_nlink = 2;
        }
        FstatKind::DevptsDirectory => {
            stat.st_dev = 0;
            stat.st_ino = 1;
            stat.st_mode = S_IFDIR | 0o755;
            stat.st_nlink = 2;
        }
        FstatKind::TcpSocket => {
            static TCP_SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(3000);
            stat.st_dev = 0;
            stat.st_ino =
                TCP_SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755;
            stat.st_nlink = 1;
        }
        FstatKind::PtyDevice { pty_num } => {
            static PTY_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(4000);
            stat.st_dev = 0;
            stat.st_ino = PTY_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFCHR | 0o620;
            stat.st_nlink = 1;
            stat.st_rdev = make_dev(136, pty_num as u64);
        }
        FstatKind::UnixSocket => {
            static UNIX_SOCKET_INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(5000);
            stat.st_dev = 0;
            stat.st_ino =
                UNIX_SOCKET_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            stat.st_mode = S_IFSOCK | 0o755;
            stat.st_nlink = 1;
        }
        FstatKind::Fifo(entry) => fill_fifo_stat(&mut stat, &entry.lock()),
        FstatKind::ProcfsFile { size } => {
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFREG | 0o444;
            stat.st_nlink = 1;
            stat.st_size = size;
        }
        FstatKind::ProcfsDirectory => {
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFDIR | 0o555;
            stat.st_nlink = 2;
            stat.st_size = 0;
        }
        FstatKind::Epoll => {
            stat.st_dev = 0;
            stat.st_ino = 0;
            stat.st_mode = S_IFREG | 0o600;
            stat.st_nlink = 1;
        }
    }

    if let Err(errno) = copy_to_user(statbuf as *mut Stat, &stat) {
        return SyscallResult::Err(errno);
    }

    SyscallResult::Ok(0)
}

/// What fstat and newfstatat report for a FIFO, which has no inode.
fn fill_fifo_stat(stat: &mut Stat, entry: &crate::ipc::fifo::FifoEntry) {
    static FIFO_INODE_COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(6000);
    stat.st_dev = 0;
    stat.st_ino = FIFO_INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    stat.st_mode = S_IFIFO | entry.mode;
    stat.st_uid = entry.uid;
    stat.st_gid = entry.gid;
    stat.st_nlink = 1;
    stat.st_size = 0;
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
    let uid = inode.uid();
    let gid = inode.gid();
    let links_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) };
    let atime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_atime)) };
    let mtime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_mtime)) };
    let ctime = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_ctime)) };
    let blocks = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_blocks)) };

    Some(InodeStat {
        mode: mode as u32,
        uid,
        gid,
        size: inode.size() as i64,
        nlink: links_count as u64,
        atime: atime as i64,
        mtime: mtime as i64,
        ctime: ctime as i64,
        blocks: blocks as i64,
    })
}

fn load_ext2_inode_stat_for_handle(handle: &crate::fs::ext2::live_inode::FileHandle) -> Option<InodeStat> {
    let guard = crate::fs::ext2::read_mount(handle.object.mount).ok()?;
    let fs = guard.as_ref()?;
    let inode = fs.read_inode(handle.verify(fs).ok()?).ok()?;
    load_inode_stat_from_inode(&inode)
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
        if fs.mount_id != mount_id { return None; }
    let inode = fs.read_inode(inode_num as u32).ok()?;
        return load_inode_stat_from_inode(&inode);
    }

    let fs_guard = ext2::root_fs_read();
    let fs = fs_guard.as_ref()?;
    if fs.mount_id != mount_id { return None; }
    let inode = fs.read_inode(inode_num as u32).ok()?;
    load_inode_stat_from_inode(&inode)
}

pub(crate) fn get_ext2_file_size_for_handle(handle: &crate::fs::ext2::live_inode::FileHandle) -> Option<u64> {
    let guard = crate::fs::ext2::read_mount(handle.object.mount).ok()?;
    let fs = guard.as_ref()?;
    Some(fs.read_inode(handle.verify(fs).ok()?).ok()?.size())
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

    log::debug!(
        "sys_getdents64: fd={}, dirp={:#x}, count={}",
        fd,
        dirp,
        count
    );

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
        return handle_procfs_getdents64(
            fd,
            dirp,
            count as usize,
            &dir_path,
            start_position,
            thread_id,
        );
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

    log::debug!(
        "sys_getdents64: wrote {} bytes, new_position={}",
        bytes_written,
        new_position
    );
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
    use super::errno::{EACCES, EIO, EISDIR, ENOENT, EPERM};
    use crate::fs::ext2::FileType;
    use crate::fs::namei::{Last, Target};

    let resolved = match resolve_user_entry(pathname) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let path = resolved.path.as_str();

    log::debug!("sys_unlink: path={:?}", path);

    // Check if this is a FIFO - if so, remove from registry
    {
        use crate::ipc::fifo::FIFO_REGISTRY;
        if FIFO_REGISTRY.exists(path) {
            match FIFO_REGISTRY.unlink(path) {
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

    if resolved.last != Last::Name {
        return SyscallResult::Err(EISDIR as u64);
    }
    let mount = match resolved.target {
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => return SyscallResult::Err(EPERM as u64),
        Target::Inode {
            file_type: FileType::Directory,
            ..
        } => return SyscallResult::Err(EISDIR as u64),
        Target::Inode { mount, .. } => mount,
    };

    let unlink_result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.unlink_file(resolved.fs_path()),
            None => {
                log::error!("sys_unlink: ext2 filesystem not mounted");
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
/// Renames oldpath to newpath as one transaction: newpath names either what
/// it named before or the renamed entry at every instant, and a failure
/// leaves the tree as it was. An existing newpath is replaced when it is a
/// non-directory and oldpath is too, or an empty directory and oldpath is a
/// directory. Neither final symlink is followed.
///
/// # Arguments
/// * `oldpath` - Current path (userspace pointer to null-terminated string)
/// * `newpath` - New path (userspace pointer to null-terminated string)
///
/// # Returns
/// 0 on success, negative errno on failure. When both names are links to the
/// same file, rename succeeds and changes nothing.
///
/// # Errors
/// * ENOENT - oldpath does not exist
/// * EISDIR - newpath is a directory but oldpath is not
/// * ENOTDIR - oldpath is a directory but newpath is not, or a pathname
///   ending in `/` names a non-directory
/// * ENOTEMPTY - newpath is a non-empty directory
/// * EINVAL - a final component is `.` or `..`, or a directory would move
///   into itself or a descendant
/// * EBUSY - an operand is a filesystem root, or newpath is a directory in use
/// * EXDEV - the names are on different filesystems
/// * ENOSPC - newpath's directory has no room for the name
/// * EMLINK - a directory would move into a directory with the most links
///   ext2 allows
/// * EIO - I/O error
pub fn sys_rename(oldpath: u64, newpath: u64) -> SyscallResult {
    use super::errno::{
        EBUSY, EINVAL, EIO, EISDIR, EMLINK, ENOENT, ENOSPC, ENOTDIR, ENOTEMPTY, EXDEV,
    };
    use crate::fs::ext2::RenameError;
    use crate::fs::namei::{Last, Target};

    // rename acts on the names themselves: neither final symlink is followed.
    let old = match resolve_user_rename(oldpath) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let new = match resolve_user_rename(newpath) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!("sys_rename: old={:?}, new={:?}", old.path, new.path);

    for operand in [&old, &new] {
        match operand.last {
            Last::Dot | Last::DotDot => return SyscallResult::Err(EINVAL as u64),
            Last::Root => return SyscallResult::Err(EBUSY as u64),
            Last::Name => {}
        }
    }
    let mount = match old.target {
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => return SyscallResult::Err(EXDEV as u64),
        Target::Inode { mount, .. } => mount,
    };
    if old.is_mount_point() || new.is_mount_point() {
        return SyscallResult::Err(EBUSY as u64);
    }
    // Both names must be on the same filesystem
    if new.mount() != Some(mount) {
        return SyscallResult::Err(EXDEV as u64);
    }
    let (Some(old_dir), Some(new_dir)) = (old.entry_dir(), new.entry_dir()) else {
        return SyscallResult::Err(EIO as u64);
    };
    // A pathname ending in `/` names a directory.
    let want_dir = old.trailing_slash || new.trailing_slash;

    let result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.rename(
                old_dir,
                old.final_name(),
                new_dir,
                new.final_name(),
                want_dir,
            ),
            None => return SyscallResult::Err(EIO as u64),
        }
    };

    match result {
        Ok(()) => SyscallResult::Ok(0),
        Err(e) => {
            log::debug!("sys_rename: failed: {:?}", e);
            let errno = match e {
                RenameError::NotFound => ENOENT,
                RenameError::NotDirectory => ENOTDIR,
                RenameError::IsDirectory => EISDIR,
                RenameError::NotEmpty => ENOTEMPTY,
                RenameError::Invalid => EINVAL,
                RenameError::Busy => EBUSY,
                RenameError::NoSpace => ENOSPC,
                RenameError::TooManyLinks => EMLINK,
                RenameError::Io => EIO,
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
    use super::errno::{EACCES, EBUSY, EINVAL, EIO, ENOENT, ENOTDIR, ENOTEMPTY};
    use crate::fs::ext2::FileType;
    use crate::fs::namei::{Last, Target};

    // rmdir removes the name itself: a final symlink is not followed.
    let resolved = match resolve_user_entry(pathname) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!("sys_rmdir: path={:?}", resolved.path);

    match resolved.last {
        Last::Dot => return SyscallResult::Err(EINVAL as u64),
        Last::DotDot => return SyscallResult::Err(ENOTEMPTY as u64),
        Last::Root => return SyscallResult::Err(EBUSY as u64),
        Last::Name => {}
    }
    if resolved.is_mount_point() {
        return SyscallResult::Err(EBUSY as u64);
    }
    let mount = match resolved.target {
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => return SyscallResult::Err(ENOTDIR as u64),
        Target::Inode {
            mount,
            file_type: FileType::Directory,
            ..
        } => mount,
        Target::Inode { .. } => return SyscallResult::Err(ENOTDIR as u64),
    };

    // Perform the rmdir operation on the correct filesystem
    let rmdir_result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.remove_directory(resolved.fs_path()),
            None => {
                log::error!("sys_rmdir: ext2 filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match rmdir_result {
        Ok(()) => {
            log::info!("sys_rmdir: successfully removed directory {}", resolved.path);
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_rmdir: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("busy") {
                EBUSY
            } else if e.contains("not found") || e.contains("not exist") {
                ENOENT
            } else if e.contains("Not a directory") || e.contains("not a directory") {
                ENOTDIR
            } else if e.contains("not empty") || e.contains("Directory not empty") {
                ENOTEMPTY
            } else if e.contains("root directory") {
                // Cannot remove root directory - treat as busy
                EBUSY
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
/// * EMLINK - oldpath has the most links ext2 allows
/// * EIO - I/O error
pub fn sys_link(oldpath: u64, newpath: u64) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EIO, ENOENT, ENOTDIR, EPERM, EXDEV};
    use crate::fs::ext2::FileType;
    use crate::fs::namei::{Last, Target};

    // As on Linux, link() names a final symlink in oldpath itself.
    let old = match resolve_user(oldpath, false) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let new = match resolve_user_create(newpath) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!("sys_link: oldpath={:?}, newpath={:?}", old.path, new.path);

    let mount = match old.target {
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => return SyscallResult::Err(EXDEV as u64),
        Target::Inode {
            file_type: FileType::Directory,
            ..
        } => return SyscallResult::Err(EPERM as u64),
        Target::Inode { mount, .. } => mount,
    };
    match new.target {
        Target::Inode { .. } => return SyscallResult::Err(EEXIST as u64),
        _ if new.last != Last::Name => return SyscallResult::Err(EEXIST as u64),
        // Only a directory is created at a name ending in `/`.
        Target::Absent { .. } if new.trailing_slash => return SyscallResult::Err(ENOENT as u64),
        Target::Absent { mount: m, .. } if m == mount => {}
        _ => {
            log::error!("sys_link: cross-filesystem link not supported");
            return SyscallResult::Err(EXDEV as u64);
        }
    }

    // Perform the hard link operation on the correct filesystem
    let link_result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_hard_link(old.fs_path(), new.fs_path()),
            None => {
                log::error!("sys_link: ext2 filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match link_result {
        Ok(()) => {
            log::info!(
                "sys_link: successfully created hard link {} -> {}",
                new.path,
                old.path
            );
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_link: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("Too many links") {
                super::errno::EMLINK
            } else if e.contains("not found") || e.contains("not exist") {
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
/// * EMLINK - The parent directory has the most links ext2 allows
/// * EIO - I/O error
pub fn sys_mkdir(pathname: u64, mode: u32) -> SyscallResult {
    use super::errno::{EACCES, EEXIST, EIO, ENOENT, ENOSPC, ENOTDIR, EPERM};
    use crate::fs::namei::{Last, Target};

    // A final symlink, even a dangling one, is an existing name.
    let resolved = match resolve_user_create(pathname) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let path = resolved.path.as_str();

    log::debug!("sys_mkdir: path={:?}, mode={:#o}", path, mode);

    if resolved.last != Last::Name {
        return SyscallResult::Err(EEXIST as u64);
    }
    let mount = match resolved.target {
        Target::Inode { .. } => return SyscallResult::Err(EEXIST as u64),
        Target::Virtual if resolved.is_mount_point() => return SyscallResult::Err(EEXIST as u64),
        Target::Virtual => return SyscallResult::Err(EPERM as u64),
        Target::Absent { mount, .. } => mount,
    };

    // Create the directory on the correct filesystem
    let cred = current_file_credentials();
    let dir_mode = (mode & 0o7777 & !cred.umask) as u16;
    let mkdir_result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_directory_owned(resolved.fs_path(), dir_mode, cred.euid, cred.egid),
            None => {
                log::error!("sys_mkdir: ext2 filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match mkdir_result {
        Ok(inode_num) => {
            log::info!(
                "sys_mkdir: successfully created directory {} (inode {})",
                path,
                inode_num
            );
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_mkdir: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("Too many links") {
                super::errno::EMLINK
            } else if e.contains("not found")
                || e.contains("not exist")
                || e.contains("Path component not found")
            {
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
    use super::errno::{EACCES, EEXIST, EINVAL, EIO, ENOENT, ENOSPC, ENOTDIR, EPERM};
    use super::userptr::copy_cstr_from_user;
    use crate::fs::namei::{Last, Target};

    // Copy the target from userspace; it is stored as written.
    let target_str = match copy_cstr_from_user(target) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let link = match resolve_user_create(linkpath) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!(
        "sys_symlink: target={:?}, linkpath={:?}",
        target_str,
        link.path
    );

    // Validate target is not empty
    if target_str.is_empty() {
        return SyscallResult::Err(EINVAL as u64);
    }
    if link.last != Last::Name {
        return SyscallResult::Err(EEXIST as u64);
    }
    let mount = match link.target {
        Target::Inode { .. } => return SyscallResult::Err(EEXIST as u64),
        Target::Virtual if link.is_mount_point() => return SyscallResult::Err(EEXIST as u64),
        Target::Virtual => return SyscallResult::Err(EPERM as u64),
        // Only a directory is created at a name ending in `/`.
        Target::Absent { .. } if link.trailing_slash => return SyscallResult::Err(ENOENT as u64),
        Target::Absent { mount, .. } => mount,
    };

    let cred = current_file_credentials();
    // Create the symbolic link on the correct filesystem
    let symlink_result = {
        let mut fs_guard = mount.write();
        match fs_guard.as_mut() {
            Some(fs) => fs.create_symlink_as(&target_str, link.fs_path(), cred.euid, cred.egid),
            None => {
                log::error!("sys_symlink: ext2 filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        }
    };

    match symlink_result {
        Ok(()) => {
            log::info!(
                "sys_symlink: successfully created symlink {} -> {}",
                link.path,
                target_str
            );
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::debug!("sys_symlink: failed: {}", e);
            // Map error to appropriate errno
            let errno = if e.contains("too long") {
                super::errno::ENAMETOOLONG
            } else if e.contains("not found")
                || e.contains("not exist")
                || e.contains("Path component not found")
            {
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
    use crate::fs::ext2::FileType;
    use crate::fs::namei::Target;

    // Validate buffer pointer
    if buf == 0 || bufsize == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // readlink reads the final symlink itself.
    let resolved = match resolve_user(pathname, false) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!("sys_readlink: pathname={:?}, bufsize={}", resolved.path, bufsize);

    let (mount, ino) = match resolved.target {
        Target::Inode {
            mount,
            ino,
            file_type: FileType::SymLink,
        } => (mount, ino),
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Inode { .. } | Target::Virtual => return SyscallResult::Err(EINVAL as u64),
    };
    let target = {
        let fs_guard = mount.read();
        let fs = match fs_guard.as_ref() {
            Some(fs) => fs,
            None => {
                log::error!("sys_readlink: ext2 filesystem not mounted");
                return SyscallResult::Err(EIO as u64);
            }
        };
        match fs.read_symlink(ino) {
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
        }
    };

    // Calculate how many bytes to copy (capped by buffer size)
    let target_bytes = target.as_bytes();
    let bytes_to_copy = core::cmp::min(target_bytes.len(), bufsize as usize);

    // Copy to user buffer (NOT null-terminated, per readlink semantics)
    if let Err(errno) = super::userptr::write_user_bytes(buf, target_bytes.as_ptr(), bytes_to_copy) {
        return SyscallResult::Err(errno);
    }

    log::debug!(
        "sys_readlink: returning {} bytes: {:?}",
        bytes_to_copy,
        &target[..bytes_to_copy]
    );
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
    sys_faccessat(AT_FDCWD, pathname, mode, 0)
}

fn access_resolved(resolved: &crate::fs::namei::Resolved, mode: u32, cred: &FileCredentials) -> SyscallResult {
    use super::errno::{EACCES, EIO, ENOENT};
    use crate::fs::namei::Target;
    if let Some(entry) = crate::ipc::fifo::FIFO_REGISTRY.get(&resolved.path) {
        return if cred.permits(&entry.lock().inode(), mode) { SyscallResult::Ok(0) } else { SyscallResult::Err(EACCES as u64) };
    }
    let (mount, ino) = match resolved.target {
        Target::Inode { mount, ino, .. } => (mount, ino),
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => {
            if resolved.is_mount_point() { return SyscallResult::Ok(0); }
            if let Some(device) = resolved.path.strip_prefix("/dev/") {
                if crate::fs::devfs::lookup(device).is_none() {
                    return SyscallResult::Err(ENOENT as u64);
                }
                return if mode & 1 == 0 { SyscallResult::Ok(0) } else { SyscallResult::Err(EACCES as u64) };
            }
            return if crate::fs::procfs::lookup_by_path(&resolved.path).is_some() {
                SyscallResult::Ok(0)
            } else { SyscallResult::Err(ENOENT as u64) };
        }
    };
    let guard = mount.read();
    let Some(fs) = guard.as_ref() else { return SyscallResult::Err(EIO as u64); };
    let inode = match fs.read_inode(ino) {
        Ok(inode) => inode, Err(_) => return SyscallResult::Err(EIO as u64),
    };
    if cred.permits(&inode, mode) { SyscallResult::Ok(0) } else { SyscallResult::Err(EACCES as u64) }
}

/// Handle opening a device file from /dev/*
///
/// # Arguments
/// * `device_name` - Name of the device (without /dev/ prefix)
/// Handle opening a /proc file or directory
///
/// For directories (/proc, /proc/trace, /proc/[pid]), returns a ProcfsDirectory fd.
/// For files, generates the content at open time and stores it in a ProcfsFile fd.
fn handle_procfs_open(path: &str, flags: u32) -> SyscallResult {
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
        let fd_kind = FdKind::ProcfsDirectory {
            path: dir_path,
            position: 0,
        };
        let fd_entry = FileDescriptor::opened(fd_kind, flags);

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
                log::debug!(
                    "handle_procfs_open: opened {} as directory fd={}",
                    normalized,
                    fd
                );
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

    let fd_kind = FdKind::ProcfsFile {
        content,
        position: 0,
    };
    let fd_entry = FileDescriptor::opened(fd_kind, flags);

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

/// * `flags` - Open flags. Every device descriptor records their access mode,
///   status flags and O_CLOEXEC.
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_devfs_open(device_name: &str, flags: u32) -> SyscallResult {
    use super::errno::{EMFILE, ENOENT};
    use crate::fs::devfs;
    use crate::ipc::fd::FileDescriptor;

    log::debug!("handle_devfs_open: device_name={:?}", device_name);

    // Check for /dev/pts/* paths - route to devptsfs
    if device_name.starts_with("pts/") {
        let pty_name = &device_name[4..]; // Remove "pts/" prefix
        return handle_devpts_open(pty_name, flags);
    }

    // Check for /dev/pts directory itself
    if device_name == "pts" {
        return handle_devpts_directory_open(flags);
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
                log::error!(
                    "handle_devfs_open: Process not found for thread {}",
                    thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devfs_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // For /dev/tty, redirect to the controlling PTY if one exists.
    // On Linux, /dev/tty is a magic device that refers to the calling process's
    // controlling terminal. The controlling terminal belongs to the SESSION,
    // not a single process. Any process in the session can open /dev/tty.
    if matches!(device.device_type, devfs::DeviceType::Tty) {
        let sid = process.sid.as_u64() as u32;
        // Drop manager lock before accessing PTY subsystem to avoid lock ordering issues
        drop(manager_guard);

        // Search active PTYs for one controlled by this session.
        // controlling_pid stores the session leader's PID (== SID).
        for pty_num in crate::tty::pty::list_active() {
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                if pair.controlling_pid.lock().map_or(false, |p| p == sid) {
                    // Found the controlling PTY — open as PtySlave
                    //
                    // NB6: these three lookups used to be `.unwrap()`-chained
                    // and would panic the kernel on a racing thread/process
                    // teardown. This arm runs on every production boot (the
                    // TTY oracle's `ctty` arm, arm 13, drives it), so a panic
                    // here is a live production risk, not a theoretical one.
                    // Graceful ESRCH returns, mirroring the pattern already
                    // used above in this same function.
                    let thread_id2 = match crate::task::scheduler::current_thread_id() {
                        Some(id) => id,
                        None => {
                            log::error!("handle_devfs_open: /dev/tty: no current thread");
                            return SyscallResult::Err(3); // ESRCH
                        }
                    };
                    let mut mg = crate::process::manager();
                    let proc2 = match mg
                        .as_mut()
                        .and_then(|manager| manager.find_process_by_thread_mut(thread_id2))
                    {
                        Some((_, p)) => p,
                        None => {
                            log::error!(
                                "handle_devfs_open: /dev/tty: process not found for thread {}",
                                thread_id2
                            );
                            return SyscallResult::Err(3); // ESRCH
                        }
                    };
                    // Same status-flag plumbing as handle_devpts_open: this is
                    // the third site that hands out a PtySlave fd, and a
                    // /dev/tty opened O_NONBLOCK must read non-blocking too.
                    // Exercised on every production boot by the TTY oracle's
                    // `ctty` arm (arm 13), which drives exactly this branch.
                    let entry = FileDescriptor::opened(FdKind::PtySlave(pty_num), flags);
                    return match proc2.fd_table.alloc_with_entry(entry) {
                        Ok(fd) => {
                            // #704 (found and fixed on feat/green-tty): this
                            // site handed out an FdKind::PtySlave fd without a
                            // matching slave_open(), while every retire path
                            // decrements on the assumption every handout path
                            // incremented. As of this round every retire path
                            // does its matching half -- including
                            // close_cloexec() (kernel/src/ipc/fd.rs), which
                            // was found to have the identical asymmetry (the
                            // exec-time close-on-exec path, unfiled, same
                            // round) and was fixed alongside this site rather
                            // than left as a sibling defect. Mirror
                            // handle_devpts_open exactly: the open and close
                            // accounting must be symmetric regardless of which
                            // path produced the fd.
                            pair.slave_open();
                            log::info!(
                                "handle_devfs_open: /dev/tty -> PTY slave {} as fd {}",
                                pty_num,
                                fd
                            );
                            SyscallResult::Ok(fd as u64)
                        }
                        Err(_) => SyscallResult::Err(EMFILE as u64),
                    };
                }
            }
        }
        // No controlling terminal found — fall through to generic device
        // Re-acquire manager lock for the generic path. Same NB6 shape as the
        // ctty-found arm above: graceful ESRCH returns instead of unwrap().
        let thread_id2 = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => {
                log::error!("handle_devfs_open: /dev/tty (no ctty): no current thread");
                return SyscallResult::Err(3); // ESRCH
            }
        };
        let mut manager_guard = crate::process::manager();
        let process = match manager_guard
            .as_mut()
            .and_then(|manager| manager.find_process_by_thread_mut(thread_id2))
        {
            Some((_, p)) => p,
            None => {
                log::error!(
                    "handle_devfs_open: /dev/tty (no ctty): process not found for thread {}",
                    thread_id2
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };
        let fd_kind = FileDescriptor::opened(FdKind::Device(device.device_type), flags);
        return match process.fd_table.alloc_with_entry(fd_kind) {
            Ok(fd) => {
                log::info!("handle_devfs_open: /dev/tty (no ctty) as fd {}", fd);
                SyscallResult::Ok(fd as u64)
            }
            Err(_) => SyscallResult::Err(EMFILE as u64),
        };
    }

    // Allocate file descriptor with Device kind
    let fd_kind = FileDescriptor::opened(FdKind::Device(device.device_type), flags);
    match process.fd_table.alloc_with_entry(fd_kind) {
        Ok(fd) => {
            log::info!(
                "handle_devfs_open: opened /dev/{} as fd {}",
                device_name,
                fd
            );
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
/// * `flags` - The caller's open flags. `O_NONBLOCK` and `O_CLOEXEC` are
///   recorded on the fd; the slave read path in `handlers.rs` tests
///   `status_flags & O_NONBLOCK` to decide between `EAGAIN` and blocking, so
///   dropping the flags here made a non-blocking slave impossible to open and
///   left that branch reachable only through `fcntl(F_SETFL)`.
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_devpts_open(pty_name: &str, flags: u32) -> SyscallResult {
    use super::errno::{EMFILE, ENOENT};
    use crate::fs::devptsfs;
    use crate::ipc::fd::FileDescriptor;

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

    // Allocate file descriptor with PtySlave kind, carrying the caller's
    // status flags so a slave opened O_NONBLOCK actually reads non-blocking.
    let entry = FileDescriptor::opened(FdKind::PtySlave(pty_num), flags);
    match process.fd_table.alloc_with_entry(entry) {
        Ok(fd) => {
            // Increment slave reference count so master can detect hangup
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                pair.slave_open();
            }
            SyscallResult::Ok(fd as u64)
        }
        Err(_) => SyscallResult::Err(EMFILE as u64),
    }
}

/// Handle opening the /dev/pts directory itself
///
/// Returns a directory fd that can be used with getdents64 to list PTY slaves.
fn handle_devpts_directory_open(flags: u32) -> SyscallResult {
    use super::errno::EMFILE;
    use crate::ipc::fd::FileDescriptor;

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
                log::error!(
                    "handle_devpts_directory_open: Process not found for thread {}",
                    thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devpts_directory_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with DevptsDirectory kind
    let fd_entry = FileDescriptor::opened(FdKind::DevptsDirectory { position: 0 }, flags);
    match process.fd_table.alloc_with_entry(fd_entry) {
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
fn handle_devfs_directory_open(flags: u32) -> SyscallResult {
    use super::errno::EMFILE;
    use crate::ipc::fd::FileDescriptor;

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
                log::error!(
                    "handle_devfs_directory_open: Process not found for thread {}",
                    thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("handle_devfs_directory_open: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate file descriptor with DevfsDirectory kind
    let fd_entry = FileDescriptor::opened(FdKind::DevfsDirectory { position: 0 }, flags);
    match process.fd_table.alloc_with_entry(fd_entry) {
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
        (".", 0),  // inode 0 for /dev directory itself
        ("..", 2), // inode 2 = root directory
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

    log::debug!(
        "handle_devfs_getdents64: wrote {} bytes, new_position={}",
        bytes_written,
        new_position
    );
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
        (".", 1, DT_DIR),  // inode 1 for /dev/pts directory itself
        ("..", 0, DT_DIR), // inode 0 = /dev directory (parent)
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

    log::debug!(
        "handle_devpts_getdents64: wrote {} bytes, new_position={}",
        bytes_written,
        new_position
    );
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
        (".", 0),  // inode 0 for this directory
        ("..", 2), // inode 2 = root directory
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
            let dt = if entry.entry_type.is_directory() {
                DT_DIR
            } else {
                DT_REG
            };
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
                if let FdKind::ProcfsDirectory {
                    ref mut position, ..
                } = fd_entry.kind
                {
                    *position = new_position;
                }
            }
        }
    }

    log::debug!(
        "handle_procfs_getdents64: wrote {} bytes, new_position={}",
        bytes_written,
        new_position
    );
    SyscallResult::Ok(bytes_written as u64)
}

/// sys_getcwd - Get current working directory
///
/// Writes the physical absolute pathname of the current working directory,
/// with its terminating NUL, to `buf`.
///
/// # Arguments
/// * `buf` - Buffer to store the path (userspace pointer)
/// * `size` - Size of the buffer
///
/// # Returns
/// The number of bytes written, NUL included (the Linux syscall ABI), or a
/// negative errno
///
/// # Errors
/// * EFAULT - Invalid buffer pointer
/// * ERANGE - Buffer too small for the pathname and its NUL
/// * ENOENT - The working directory no longer has a name
pub fn sys_getcwd(buf: u64, size: u64) -> SyscallResult {
    use super::errno::{EFAULT, ERANGE};

    log::debug!("sys_getcwd: buf={:#x}, size={}", buf, size);

    // Validate buffer pointer
    if buf == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // The pathname is derived from the working directory itself, after the
    // process manager is released: the walk reads the filesystem, and the
    // user buffer may be a CoW page whose fault handler takes the manager.
    let dir = crate::fs::namei::current_working_dir();
    let cwd = match crate::fs::namei::working_dir_path(&dir) {
        Ok(path) => path,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let mut bytes = cwd.into_bytes();
    bytes.push(0);

    // Nothing is written unless the whole pathname and its NUL fit.
    if bytes.len() > size as usize {
        log::debug!(
            "sys_getcwd: buffer too small ({} < {})",
            size,
            bytes.len()
        );
        return SyscallResult::Err(ERANGE as u64);
    }
    if let Err(errno) = super::userptr::write_user_bytes(buf, bytes.as_ptr(), bytes.len()) {
        return SyscallResult::Err(errno);
    }

    // The Linux syscall returns the length of the buffer used, NUL included.
    SyscallResult::Ok(bytes.len() as u64)
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
    let resolved = match resolve_user(pathname, true) {
        Ok(r) => r,
        Err(e) => return e,
    };

    log::debug!("sys_chdir: path={:?}", resolved.path);

    let dir = match crate::fs::namei::working_dir(&resolved) {
        Ok(dir) => dir,
        Err(errno) => return SyscallResult::Err(errno),
    };

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_chdir: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Update the process's cwd. The directory it replaces is released when
    // the manager is, so its handle is not dropped under the lock.
    let previous = {
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
        core::mem::replace(&mut process.cwd, dir)
    };
    drop(previous);
    log::info!("sys_chdir: changed cwd to {}", resolved.path);
    SyscallResult::Ok(0)
}

/// Handle opening a FIFO (named pipe)
///
/// # Arguments
/// * `path` - Absolute path to the FIFO
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_NONBLOCK, etc.)
///
/// # Returns
/// File descriptor on success, negative errno on failure
fn handle_fifo_open(
    path: &str,
    flags: u32,
    entry: alloc::sync::Arc<spin::Mutex<crate::ipc::fifo::FifoEntry>>,
) -> SyscallResult {
    use super::errno::EMFILE;
    use crate::ipc::fd::{status_flags, FdKind, FileDescriptor};
    use crate::ipc::fifo::{complete_fifo_open, open_fifo_read, open_fifo_write, FifoOpenResult};
    use alloc::string::String;

    let access_mode = flags & 3; // O_RDONLY=0, O_WRONLY=1, O_RDWR=2
    let nonblock = (flags & status_flags::O_NONBLOCK) != 0;

    log::debug!(
        "handle_fifo_open: path={}, access_mode={}, nonblock={}",
        path,
        access_mode,
        nonblock
    );

    // O_RDWR on a FIFO is not well-defined in POSIX, but we can support it
    // by opening both read and write ends. For simplicity, treat it as read.
    let for_write = access_mode == O_WRONLY;

    // Attempt to open the FIFO
    let result = if for_write {
        open_fifo_write(&entry, nonblock)
    } else {
        open_fifo_read(&entry, nonblock)
    };

    match result {
        FifoOpenResult::Ready(buffer) => {
            // FIFO is ready - create fd
            let kind = if for_write {
                FdKind::FifoWrite(String::from(path), buffer, entry)
            } else {
                FdKind::FifoRead(String::from(path), buffer, entry)
            };

            let fd_entry = FileDescriptor::opened(kind, flags);

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
                    log::info!(
                        "handle_fifo_open: opened FIFO {} as fd {} ({})",
                        path,
                        fd,
                        if for_write { "write" } else { "read" }
                    );
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

            log::debug!(
                "handle_fifo_open: thread {} blocking for {} end on {}",
                thread_id,
                if for_write { "reader" } else { "writer" },
                path
            );

            // Block the current thread AND set blocked_in_syscall flag.
            // CRITICAL: Setting blocked_in_syscall is essential because:
            // 1. The thread will enter a kernel-mode HLT loop below
            // 2. If a context switch happens while in HLT, the scheduler sees
            //    from_userspace=false (kernel mode) but blocked_in_syscall tells
            //    it to save/restore kernel context, not userspace context
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_in_syscall();
            });

            // CRITICAL RACE CONDITION FIX:
            // Check if other end opened AGAIN after setting Blocked state.
            // The other end might have opened between:
            //   - when open_fifo_read/write returned Block
            //   - when we set thread state to Blocked
            // If other end opened during that window, add_reader/add_writer
            // would have tried to wake us but unblock() would have done nothing.
            let other_end_ready =
                Cpu::without_interrupts(|| match complete_fifo_open(&entry, for_write) {
                    FifoOpenResult::Ready(_) => true,
                    _ => false,
                });
            if other_end_ready {
                log::debug!(
                    "FIFO: Thread {} caught race - other end opened during block setup",
                    thread_id
                );
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
                    if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                        // Signal pending - clean up thread state and return EINTR
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
                        crate::per_cpu::preempt_disable();
                        log::debug!(
                            "handle_fifo_open: Thread {} interrupted by signal (EINTR)",
                            thread_id
                        );
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
                    })
                    .unwrap_or(false);

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
            match complete_fifo_open(&entry, for_write) {
                FifoOpenResult::Ready(buffer) => {
                    // Now ready - create fd
                    let kind = if for_write {
                        FdKind::FifoWrite(path_owned.clone(), buffer, entry)
                    } else {
                        FdKind::FifoRead(path_owned.clone(), buffer, entry)
                    };

                    let fd_entry = FileDescriptor::opened(kind, flags);

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
                            log::info!(
                                "handle_fifo_open: opened FIFO {} as fd {} ({})",
                                path_owned,
                                fd,
                                if for_write { "write" } else { "read" }
                            );
                            SyscallResult::Ok(fd as u64)
                        }
                        Err(_) => SyscallResult::Err(EMFILE as u64),
                    }
                }
                FifoOpenResult::Block => {
                    // Should not happen after being woken
                    log::error!("FIFO: Thread {} still blocked after wake!", thread_id);
                    SyscallResult::Err(11) // EAGAIN
                }
                FifoOpenResult::Error(errno) => SyscallResult::Err(errno as u64),
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
pub fn sys_newfstatat(dirfd: i32, pathname: u64, statbuf: u64, flags: u32) -> SyscallResult {
    use super::errno::{EFAULT, ENOENT};
    use super::userptr::{copy_cstr_from_user, copy_to_user};
    use crate::fs::namei::Target;

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

    let resolved = match crate::fs::namei::resolve(&path, flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(r) => r,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let full_path = resolved.path.as_str();

    // Paths outside ext2 have no inode to read. A FIFO is described without
    // opening it, since an open could block or release a waiting writer.
    // devfs and procfs paths are described by the descriptor an open of them
    // yields; those opens make no permission check.
    if let Some(entry) = crate::ipc::fifo::FIFO_REGISTRY.get(full_path) {
        let mut stat = Stat::zeroed();
        stat.st_blksize = 4096;
        fill_fifo_stat(&mut stat, &entry.lock());
        return match copy_to_user(statbuf as *mut Stat, &stat) {
            Ok(()) => SyscallResult::Ok(0),
            Err(errno) => SyscallResult::Err(errno),
        };
    }
    let (mount, inode_num) = match resolved.target {
        Target::Inode { mount, ino, .. } => (mount, ino as u64),
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual => {
            let fd = match sys_open(pathname, O_RDONLY, 0) {
                SyscallResult::Ok(fd) => fd as i32,
                error => return error,
            };
            let result = sys_fstat(fd, statbuf);
            let _ = super::pipe::sys_close(fd);
            return result;
        }
    };
    let mount_id = match mount.read().as_ref() {
        Some(fs) => fs.mount_id,
        None => return SyscallResult::Err(ENOENT as u64),
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
        stat.st_nlink = inode_stat.nlink as _;
        stat.st_atime = inode_stat.atime;
        stat.st_mtime = inode_stat.mtime;
        stat.st_ctime = inode_stat.ctime;
        stat.st_blocks = inode_stat.blocks;
    }

    if let Err(errno) = copy_to_user(statbuf as *mut Stat, &stat) {
        return SyscallResult::Err(errno);
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
pub fn sys_faccessat(dirfd: i32, pathname: u64, mode: u32, flags: u32) -> SyscallResult {
    const AT_EACCESS: u32 = 0x200;
    if mode & !7 != 0 || flags & !(AT_SYMLINK_NOFOLLOW | AT_EACCESS) != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    let path = match super::userptr::copy_cstr_from_user(pathname) {
        Ok(path) => path, Err(errno) => return SyscallResult::Err(errno),
    };
    let cred = FileCredentials::current(flags & AT_EACCESS == 0);
    let resolved = match super::metadata::resolve_at(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0, &cred) {
        Ok(r) => r, Err(errno) => return SyscallResult::Err(errno),
    };
    access_resolved(&resolved, mode, &cred)
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
pub fn sys_linkat(
    olddirfd: i32,
    oldpath: u64,
    newdirfd: i32,
    newpath: u64,
    _flags: i32,
) -> SyscallResult {
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
    let now = crate::time::current_unix_time() as u32;

    // Determine what atime/mtime to set
    let (set_atime, set_mtime) = if times_ptr == 0 {
        // NULL times = set both to current time
        (Some(now), Some(now))
    } else {
        // Read two Timespec structs from userspace
        let times: [UtimeTimespec; 2] =
            match super::userptr::copy_from_user(times_ptr as *const [UtimeTimespec; 2]) {
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
                            return Some(file.handle.clone());
                        }
                    }
                }
            }
            None
        });

        let handle = match fd_info {
            Some(handle) => handle,
            None => return SyscallResult::Err(super::errno::EBADF as u64),
        };

        return update_inode_timestamps(&handle, set_atime, set_mtime);
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

    let no_follow = (flags & AT_SYMLINK_NOFOLLOW) != 0;
    let resolved = match crate::fs::namei::resolve(&path, !no_follow) {
        Ok(resolved) => resolved,
        Err(errno) => return SyscallResult::Err(errno),
    };
    // The resolution holds the inode it found; the update acts on that one.
    match resolved.handle() {
        Some(handle) => update_inode_timestamps(handle, set_atime, set_mtime),
        None => SyscallResult::Err(super::errno::ENOENT as u64),
    }
}

/// Helper: update an inode's atime/mtime on the ext2 filesystem
fn update_inode_timestamps(
    handle: &crate::fs::ext2::live_inode::FileHandle,
    set_atime: Option<u32>,
    set_mtime: Option<u32>,
) -> SyscallResult {
    use crate::fs::ext2;

    let do_update = |fs: &mut ext2::Ext2Fs| -> SyscallResult {
        let inode_num = match handle.verify(fs) {
            Ok(ino) => ino,
            Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
        };
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

    let mut fs_guard = match ext2::write_mount(handle.object.mount) {
        Ok(guard) => guard,
        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
    };
    match fs_guard.as_mut() {
        Some(fs) => do_update(fs),
        None => SyscallResult::Err(super::errno::EIO as u64),
    }
}

/// Snapshot an ext2 file or directory descriptor before disk I/O, releasing the process
/// lock (which masks IRQs) before waiting for device completion.
fn ext2_fd_info(fd: i32, writable: bool) -> Result<(u32, usize, Option<crate::fs::ext2::live_inode::FileHandle>), u64> {
    use super::errno::{EBADF, EINVAL};
    crate::arch_without_interrupts(|| {
        let thread = crate::task::scheduler::current_thread_id().ok_or(EBADF as u64)?;
        let manager_guard = crate::process::manager();
        let manager = manager_guard.as_ref().ok_or(EBADF as u64)?;
        let (_, process) = manager.find_process_by_thread(thread).ok_or(EBADF as u64)?;
        let entry = process.fd_table.get(fd).ok_or(EBADF as u64)?;
        match &entry.kind {
            FdKind::RegularFile(file) => {
                let file = file.lock();
                if writable && !entry.writable() {
                    return Err(EINVAL as u64);
                }
                Ok((file.inode_num as u32, file.mount_id, Some(file.handle.clone())))
            }
            FdKind::Directory(dir) if !writable => {
                let dir = dir.lock();
                Ok((dir.inode_num as u32, dir.mount_id, None))
            }
            _ => Err(EINVAL as u64),
        }
    })
}

/// fsync/fdatasync write back all dirty shared pages of the pinned inode,
/// then flush data and metadata to the device. Without dirty mapped pages,
/// ext2 already wrote both synchronously and only the device flush remains.
pub fn sys_fsync(fd: i32) -> SyscallResult {
    use crate::fs::ext2;
    let (inode_num, mount_id, handle) = match ext2_fd_info(fd, false) {
        Ok(info) => info,
        Err(errno) => return SyscallResult::Err(errno),
    };
    if let Some(handle) = handle.as_ref().filter(|handle| handle.object.map.has_dirty()) {
        let result = crate::memory::file_map::sync_range(handle, 0, u64::MAX);
        return match result {
            Ok(()) => SyscallResult::Ok(0),
            Err(_) => SyscallResult::Err(super::errno::EIO as u64),
        };
    }
    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    let guard = if is_home {
        ext2::home_fs_read()
    } else {
        ext2::root_fs_read()
    };
    if guard.as_ref().is_none_or(|fs| fs.mount_id != mount_id) {
        return SyscallResult::Err(super::errno::EIO as u64);
    }
    if let Some(handle) = handle {
        if guard.as_ref().is_none_or(|fs| handle.verify(fs).is_err()) {
            return SyscallResult::Err(super::errno::EIO as u64);
        }
    }
    match guard.as_ref().map(|fs| fs.check_shrink(inode_num).and_then(|()| fs.sync())) {
        Some(Ok(())) => SyscallResult::Ok(0),
        _ => SyscallResult::Err(super::errno::EIO as u64),
    }
}

fn resize_inode(fs: &mut crate::fs::ext2::Ext2Fs, ino: u32, length: u64, unprivileged: bool) -> SyscallResult {
    use super::errno::{EFBIG, EINVAL, EIO, EISDIR};
    match fs.read_inode(ino) {
        Ok(inode) if inode.is_dir() => return SyscallResult::Err(EISDIR as u64),
        Ok(inode) if !inode.is_file() => return SyscallResult::Err(EINVAL as u64),
        Err(_) => return SyscallResult::Err(EIO as u64),
        _ => {}
    }
    if length > fs.max_file_size() {
        return SyscallResult::Err(EFBIG as u64);
    }
    match fs.resize_file_as(ino, length, unprivileged) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(EIO as u64),
    }
}

/// ftruncate changes the inode, leaving every open file description's offset alone.
pub fn sys_ftruncate(fd: i32, length: i64) -> SyscallResult {
    use crate::fs::ext2;
    if length < 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    let (ino, _, handle) = match ext2_fd_info(fd, true) {
        Ok(info) => info,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let Some(handle) = handle else { return SyscallResult::Err(super::errno::EINVAL as u64); };
    let unprivileged = current_file_credentials().euid != 0;
    let mut guard = match ext2::write_mount(handle.object.mount) {
        Ok(guard) => guard,
        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
    };
    match guard.as_mut() {
        Some(fs) if handle.verify(fs).is_ok() => resize_inode(fs, ino, length as u64, unprivileged),
        _ => SyscallResult::Err(super::errno::EIO as u64),
    }
}

/// truncate resolves the final symlink and resizes the same inode open FDs use.
pub fn sys_truncate(pathname: u64, length: i64) -> SyscallResult {
    use super::errno::{EINVAL, EIO, ENOENT};
    use crate::fs::namei::Target;
    if length < 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    let path = match super::userptr::copy_cstr_from_user(pathname) {
        Ok(path) => path,
        Err(errno) => return SyscallResult::Err(errno as u64),
    };
    if path.is_empty() {
        return SyscallResult::Err(ENOENT as u64);
    }
    let resolved = match crate::fs::namei::resolve(&path, true) {
        Ok(r) => r,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let full_path = resolved.path.as_str();
    if crate::ipc::fifo::FIFO_REGISTRY.exists(full_path) {
        return SyscallResult::Err(EINVAL as u64);
    }
    let (mount, ino) = match resolved.target {
        Target::Inode { mount, ino, .. } => (mount, ino),
        Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        Target::Virtual if resolved.is_mount_point() => {
            return SyscallResult::Err(super::errno::EISDIR as u64)
        }
        Target::Virtual => {
            // Resolve virtual paths through their filesystem before rejecting resize.
            let fd = match sys_open(pathname, O_RDONLY, 0) {
                SyscallResult::Ok(fd) => fd as i32,
                error => return error,
            };
            let _ = super::pipe::sys_close(fd);
            return SyscallResult::Err(EINVAL as u64);
        }
    };
    let cred = current_file_credentials();
    let mut guard = mount.write();
    let fs = match guard.as_mut() {
        Some(fs) => fs,
        None => return SyscallResult::Err(EIO as u64),
    };
    let inode = match fs.read_inode(ino) {
        Ok(inode) => inode,
        Err(_) => return SyscallResult::Err(EIO as u64),
    };
    if inode.is_file() {
        if let Err(error) = check_open_access(&inode, O_WRONLY, &cred) {
            return error;
        }
    }
    resize_inode(fs, ino, length as u64, cred.euid != 0)
}

/// Record a successful nonempty read after releasing the read-side FS lock.
pub(super) fn update_read_atime(ino: u32, mount_id: usize) -> Result<(), u64> {
    use crate::fs::ext2;
    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    let mut guard = if is_home {
        ext2::home_fs_write()
    } else {
        ext2::root_fs_write()
    };
    let fs = guard.as_mut().ok_or(super::errno::EIO as u64)?;
    fs.update_atime(ino).map_err(|_| super::errno::EIO as u64)
}
