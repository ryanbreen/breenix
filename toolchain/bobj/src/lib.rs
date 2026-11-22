#![forbid(unsafe_code)]

pub mod ar;
pub mod elf;

use std::fmt;

/// A format error at a byte offset in the supplied file.
#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: &'static str,
}

impl Error {
    const fn new(offset: usize, message: &'static str) -> Self {
        Self { offset, message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "byte {:#x}: {}", self.offset, self.message)
    }
}

impl std::error::Error for Error {}
