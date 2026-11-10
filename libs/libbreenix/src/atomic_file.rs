//! Atomic replacement of a file in its own directory.
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum SaveResult {
    Saved,
    Unconfirmed(io::Error),
    Failed(io::Error),
}

/// Operations exposed for deterministic fault and interruption testing.
pub trait Operations {
    type File;
    fn create_temp(&mut self, dest: &Path) -> io::Result<(PathBuf, Self::File)>;
    fn write(&mut self, file: &mut Self::File, bytes: &[u8]) -> io::Result<usize>;
    fn sync_file(&mut self, file: &Self::File) -> io::Result<()>;
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()>;
    fn remove_temp(&mut self, path: &Path) -> io::Result<()>;
}

pub struct StdOperations;

fn directory(dest: &Path) -> &Path {
    dest.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn prefix(dest: &Path) -> io::Result<OsString> {
    let name = dest.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no filename")
    })?;
    let mut prefix = OsString::from(".");
    prefix.push(name);
    prefix.push(".atomic-");
    Ok(prefix)
}

impl Operations for StdOperations {
    type File = File;
    fn create_temp(&mut self, dest: &Path) -> io::Result<(PathBuf, File)> {
        let prefix = prefix(dest)?;
        loop {
            let mut name = prefix.clone();
            name.push(format!(
                "{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let path = directory(dest).join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
        file.write(bytes)
    }
    fn sync_file(&mut self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        File::open(dir)?.sync_all()
    }
    fn remove_temp(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

pub fn save(dest: &Path, bytes: &[u8]) -> SaveResult {
    save_with(&mut StdOperations, dest, bytes)
}

pub fn save_with<O: Operations>(ops: &mut O, dest: &Path, mut bytes: &[u8]) -> SaveResult {
    let (temp, mut file) = match ops.create_temp(dest) {
        Ok(pair) => pair,
        Err(e) => return SaveResult::Failed(e),
    };
    let result = (|| {
        while !bytes.is_empty() {
            match ops.write(&mut file, bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write temporary file",
                    ))
                }
                Ok(n) if n <= bytes.len() => bytes = &bytes[n..],
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid write length",
                    ))
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        ops.sync_file(&file)?;
        drop(file);
        ops.rename(&temp, dest)
    })();
    if let Err(e) = result {
        let _ = ops.remove_temp(&temp);
        return SaveResult::Failed(e);
    }
    match ops.sync_dir(directory(dest)) {
        Ok(()) => SaveResult::Saved,
        Err(e) => SaveResult::Unconfirmed(e),
    }
}

/// Remove abandoned temporaries for this destination at application startup.
/// Call only when no save for the same destination is in progress.
pub fn remove_stale(dest: &Path) -> io::Result<()> {
    let prefix = prefix(dest)?;
    for entry in fs::read_dir(directory(dest))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name
            .as_encoded_bytes()
            .strip_prefix(prefix.as_encoded_bytes())
        else {
            continue;
        };
        let mut parts = suffix.split(|byte| *byte == b'-');
        let valid_number = |part: Option<&[u8]>| {
            part.is_some_and(|value| !value.is_empty() && value.iter().all(u8::is_ascii_digit))
        };
        if valid_number(parts.next()) && valid_number(parts.next()) && parts.next().is_none() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
