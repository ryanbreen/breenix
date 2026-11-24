//! File freshness for POSIX.1-2024 make, under "EXTENDED DESCRIPTION".

use bobj::ar::Archive;
use std::{
    fs,
    io::ErrorKind,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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

pub(crate) fn member_modified(archive: &str, member: &str) -> Result<Option<SystemTime>, String> {
    let bytes = match fs::read(archive) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let archive = Archive::parse(&bytes).map_err(|error| error.to_string())?;
    let Some(member) = archive
        .members
        .iter()
        .find(|entry| entry.name == member.as_bytes())
    else {
        return Ok(None);
    };
    let seconds = member.timestamp().map_err(|error| error.to_string())?;
    let elapsed = Duration::from_secs(seconds.unsigned_abs());
    let modified = if seconds < 0 {
        UNIX_EPOCH.checked_sub(elapsed)
    } else {
        UNIX_EPOCH.checked_add(elapsed)
    };
    modified
        .map(Some)
        .ok_or_else(|| "archive member timestamp is out of range".into())
}

pub(crate) fn newer(target: Option<SystemTime>, prerequisite: Option<SystemTime>) -> bool {
    // A prerequisite successfully made without a file is newer than its dependents.
    target.is_none_or(|target| prerequisite.is_none_or(|modified| modified > target))
}
