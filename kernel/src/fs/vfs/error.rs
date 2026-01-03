//! VFS Error Types
//!
//! Defines error conditions that can occur during VFS operations.

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
