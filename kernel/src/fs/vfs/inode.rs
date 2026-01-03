//! VFS Inode Abstraction
//!
//! Provides an abstract representation of filesystem inodes, independent of
//! the underlying filesystem implementation (ext2, tmpfs, etc.).

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
    #[allow(dead_code)] // Part of VFS inode API
    pub fn is_dir(&self) -> bool {
        matches!(self.file_type, FileType::Directory)
    }

    /// Check if this is a regular file
    #[allow(dead_code)] // Part of VFS inode API
    pub fn is_file(&self) -> bool {
        matches!(self.file_type, FileType::Regular)
    }

    /// Check if this is a symbolic link
    #[allow(dead_code)] // Part of VFS inode API
    pub fn is_symlink(&self) -> bool {
        matches!(self.file_type, FileType::SymLink)
    }
}
