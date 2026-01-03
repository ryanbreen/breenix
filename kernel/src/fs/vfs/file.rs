//! VFS Open File Representation
//!
//! Provides an abstract representation of open files with file descriptor operations.

use super::error::VfsError;
use super::inode::VfsInode;
use spin::Mutex;

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
    /// * `offset` - The offset to seek by
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
