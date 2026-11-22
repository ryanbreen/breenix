//! File freshness for POSIX.1-2024 make, under "EXTENDED DESCRIPTION".

use std::{fs, io::ErrorKind, time::SystemTime};

pub(crate) fn modified(name: &str) -> Result<Option<SystemTime>, String> {
    let metadata = match fs::metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("'{name}': {error}")),
    };
    metadata
        .modified()
        .map(Some)
        .map_err(|error| format!("'{name}': modification time: {error}"))
}

pub(crate) fn newer(target: Option<SystemTime>, prerequisite: Option<SystemTime>) -> bool {
    // A prerequisite successfully made without a file is newer than its dependents.
    target.is_none_or(|target| prerequisite.is_none_or(|modified| modified > target))
}
