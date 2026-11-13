//! Filesystem errors use the existing syscall errno values.

/// A positive POSIX errno, before the syscall ABI encodes an error return.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Errno(pub i32);
