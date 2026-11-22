//! Makefile reader, dependency graph and recipe runner for Breenix's serial make subset.

#![forbid(unsafe_code)]

use std::{fmt, path::Path, sync::Arc};

mod cli;
pub mod graph;
mod inference;
pub mod r#macro;
pub mod reader;
mod run;
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
