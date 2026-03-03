//! VFS (Virtual File System) Types
//!
//! Provides abstract representations of filesystem objects, independent of
//! the underlying filesystem implementation (ext2, ramfs, devfs, etc.).
//!
//! Combined from kernel/src/fs/vfs/{error,inode,file}.rs

use spin::Mutex;

// ============================================================================
// VFS Errors
// ============================================================================

/// VFS error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    /// File or directory not found
    NotFound,
    /// Permission denied
    PermissionDenied,
    /// Is a directory (when file expected)
    IsDirectory,
    /// Not a directory (when directory expected)
    NotDirectory,
    /// File or directory already exists
    AlreadyExists,
    /// No space left on device
    NoSpace,
    /// I/O error occurred
    IoError,
    /// Invalid path
    InvalidPath,
    /// Filesystem not mounted
    NotMounted,
    /// Filesystem is read-only
    ReadOnly,
    /// Too many open files
    TooManyOpenFiles,
}

// ============================================================================
// File Types and Permissions
// ============================================================================

/// File type (matches POSIX conventions)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// Regular file
    Regular,
    /// Directory
    Directory,
    /// Symbolic link
    SymLink,
    /// Character device
    CharDevice,
    /// Block device
    BlockDevice,
    /// FIFO (named pipe)
    Fifo,
    /// Socket
    Socket,
}

/// File permissions
#[derive(Debug, Clone, Copy)]
pub struct FilePermissions {
    /// Owner can read
    pub owner_read: bool,
    /// Owner can write
    pub owner_write: bool,
    /// Owner can execute
    pub owner_exec: bool,
    /// Group can read
    pub group_read: bool,
    /// Group can write
    pub group_write: bool,
    /// Group can execute
    pub group_exec: bool,
    /// Others can read
    pub other_read: bool,
    /// Others can write
    pub other_write: bool,
    /// Others can execute
    pub other_exec: bool,
}

impl FilePermissions {
    /// Create permissions from a POSIX mode value (lower 9 bits)
    pub fn from_mode(mode: u16) -> Self {
        Self {
            owner_read: (mode & 0o400) != 0,
            owner_write: (mode & 0o200) != 0,
            owner_exec: (mode & 0o100) != 0,
            group_read: (mode & 0o040) != 0,
            group_write: (mode & 0o020) != 0,
            group_exec: (mode & 0o010) != 0,
            other_read: (mode & 0o004) != 0,
            other_write: (mode & 0o002) != 0,
            other_exec: (mode & 0o001) != 0,
        }
    }

    /// Convert permissions to POSIX mode value
    pub fn to_mode(&self) -> u16 {
        let mut mode = 0u16;
        if self.owner_read { mode |= 0o400; }
        if self.owner_write { mode |= 0o200; }
        if self.owner_exec { mode |= 0o100; }
        if self.group_read { mode |= 0o040; }
        if self.group_write { mode |= 0o020; }
        if self.group_exec { mode |= 0o010; }
        if self.other_read { mode |= 0o004; }
        if self.other_write { mode |= 0o002; }
        if self.other_exec { mode |= 0o001; }
        mode
    }
}

/// VFS Inode - abstract representation of a file/directory
#[derive(Debug, Clone)]
pub struct VfsInode {
    /// Inode number (unique within filesystem)
    pub inode_num: u64,
    /// File type
    pub file_type: FileType,
    /// File size in bytes
    pub size: u64,
    /// File permissions
    pub permissions: FilePermissions,
    /// Owner user ID
    pub uid: u32,
    /// Owner group ID
    pub gid: u32,
    /// Number of hard links
    pub link_count: u16,
    /// Last access time (Unix timestamp)
    pub atime: u64,
    /// Last modification time (Unix timestamp)
    pub mtime: u64,
    /// Creation/status change time (Unix timestamp)
    pub ctime: u64,
}

impl VfsInode {
    /// Check if this is a directory
    pub fn is_dir(&self) -> bool {
        matches!(self.file_type, FileType::Directory)
    }

    /// Check if this is a regular file
    pub fn is_file(&self) -> bool {
        matches!(self.file_type, FileType::Regular)
    }

    /// Check if this is a symbolic link
    pub fn is_symlink(&self) -> bool {
        matches!(self.file_type, FileType::SymLink)
    }
}

// ============================================================================
// Open Flags and Seek
// ============================================================================

/// Open file flags (POSIX O_* flags)
#[derive(Debug, Clone, Copy)]
pub struct OpenFlags {
    /// File is open for reading
    pub read: bool,
    /// File is open for writing
    pub write: bool,
    /// Writes append to end of file
    pub append: bool,
    /// Create file if it doesn't exist
    pub create: bool,
    /// Truncate file to zero length on open
    pub truncate: bool,
}

impl OpenFlags {
    /// O_RDONLY - Open for reading only
    pub const O_RDONLY: u32 = 0;
    /// O_WRONLY - Open for writing only
    pub const O_WRONLY: u32 = 1;
    /// O_RDWR - Open for reading and writing
    pub const O_RDWR: u32 = 2;
    /// O_CREAT - Create file if it doesn't exist
    pub const O_CREAT: u32 = 0x40;
    /// O_TRUNC - Truncate file to zero length
    pub const O_TRUNC: u32 = 0x200;
    /// O_APPEND - Append mode (writes go to end of file)
    pub const O_APPEND: u32 = 0x400;

    /// Parse POSIX open flags into OpenFlags structure
    pub fn from_flags(flags: u32) -> Self {
        let access_mode = flags & 0x3;
        let read = access_mode == Self::O_RDONLY || access_mode == Self::O_RDWR;
        let write = access_mode == Self::O_WRONLY || access_mode == Self::O_RDWR;

        Self {
            read,
            write,
            append: (flags & Self::O_APPEND) != 0,
            create: (flags & Self::O_CREAT) != 0,
            truncate: (flags & Self::O_TRUNC) != 0,
        }
    }

    /// Convert OpenFlags back to POSIX flags
    pub fn to_flags(&self) -> u32 {
        let mut flags = if self.read && self.write {
            Self::O_RDWR
        } else if self.write {
            Self::O_WRONLY
        } else {
            Self::O_RDONLY
        };

        if self.append { flags |= Self::O_APPEND; }
        if self.create { flags |= Self::O_CREAT; }
        if self.truncate { flags |= Self::O_TRUNC; }

        flags
    }
}

/// Seek origin for file positioning
#[derive(Debug, Clone, Copy)]
pub enum SeekFrom {
    /// Seek from start of file (absolute position)
    Start(u64),
    /// Seek from current position (relative)
    Current(i64),
    /// Seek from end of file (relative, usually negative)
    End(i64),
}

// ============================================================================
// Open File
// ============================================================================

/// An open file handle
pub struct OpenFile {
    /// The VFS inode for this file
    pub inode: VfsInode,
    /// Open flags
    pub flags: OpenFlags,
    /// Current file position (protected by mutex for concurrent access)
    pub position: Mutex<u64>,
    /// Mount point ID (identifies which filesystem this belongs to)
    pub mount_id: usize,
}

impl OpenFile {
    /// Create a new open file handle
    pub fn new(inode: VfsInode, flags: OpenFlags, mount_id: usize) -> Self {
        Self {
            inode,
            flags,
            position: Mutex::new(0),
            mount_id,
        }
    }

    /// Seek to a new position in the file
    ///
    /// # Arguments
    /// * `whence` - The origin for the seek operation
    ///
    /// # Returns
    /// The new absolute position from the start of the file
    pub fn seek(&self, whence: SeekFrom) -> Result<u64, VfsError> {
        let mut pos = self.position.lock();
        let file_size = self.inode.size;

        let new_pos = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    (*pos).saturating_add(offset as u64)
                } else {
                    (*pos).saturating_sub((-offset) as u64)
                }
            }
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    file_size.saturating_add(offset as u64)
                } else {
                    file_size.saturating_sub((-offset) as u64)
                }
            }
        };

        *pos = new_pos;
        Ok(new_pos)
    }

    /// Get the current file position
    pub fn tell(&self) -> u64 {
        *self.position.lock()
    }

    /// Check if file is open for reading
    pub fn can_read(&self) -> bool {
        self.flags.read
    }

    /// Check if file is open for writing
    pub fn can_write(&self) -> bool {
        self.flags.write
    }
}

// ============================================================================
// Additional types for VFS consumers
// ============================================================================

/// A directory entry returned from readdir operations
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Entry name
    pub name: alloc::string::String,
    /// Inode number
    pub inode_num: u64,
    /// File type
    pub file_type: FileType,
}

/// File stat information (subset of POSIX struct stat)
#[derive(Debug, Clone)]
pub struct FileStat {
    /// Inode number
    pub inode_num: u64,
    /// File type and permissions mode
    pub mode: u16,
    /// File type
    pub file_type: FileType,
    /// Number of hard links
    pub nlink: u16,
    /// Owner user ID
    pub uid: u32,
    /// Owner group ID
    pub gid: u32,
    /// File size in bytes
    pub size: u64,
    /// Last access time
    pub atime: u64,
    /// Last modification time
    pub mtime: u64,
    /// Creation/change time
    pub ctime: u64,
    /// Device ID (for device files)
    pub rdev: u64,
}
