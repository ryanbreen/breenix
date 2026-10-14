//! Clean-room ISO C17 / POSIX stdio.
pub(crate) mod file;
pub(crate) use file::flush_all;

mod args;
mod float;
pub(crate) mod format;
pub(crate) use format::{vformat, Format, Sink};

mod misc;
pub(crate) mod scan;

pub(crate) mod pipe;
