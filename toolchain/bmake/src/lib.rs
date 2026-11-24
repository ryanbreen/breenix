//! Makefile reader, dependency graph and recipe runner for Breenix make.

// E25 ruling 31 permits signal handling FFI in the signals module only.
#![deny(unsafe_code)]

use std::{fmt, path::Path, sync::Arc};

#[forbid(unsafe_code)]
mod cli;
#[forbid(unsafe_code)]
pub mod graph;
#[forbid(unsafe_code)]
mod inference;
#[forbid(unsafe_code)]
mod inventory;
#[forbid(unsafe_code)]
mod job_pool;
#[forbid(unsafe_code)]
pub mod r#macro;
#[forbid(unsafe_code)]
mod make_functions;
#[forbid(unsafe_code)]
pub mod reader;
#[forbid(unsafe_code)]
mod run;
mod signals;
#[forbid(unsafe_code)]
mod special;
#[forbid(unsafe_code)]
mod time;

pub use cli::execute;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    pub path: Arc<Path>,
    pub line: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

#[derive(Debug)]
pub struct Error {
    pub location: Location,
    message: String,
}

impl Error {
    fn new(location: &Location, message: impl Into<String>) -> Self {
        Self {
            location: location.clone(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.location, self.message)
    }
}

impl std::error::Error for Error {}
