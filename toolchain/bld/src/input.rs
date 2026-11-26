use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Result;

const CHUNK_SIZE: usize = 64 * 1024;

pub(crate) trait ReadSource {
    fn read(&mut self, file: &mut File, bytes: &mut [u8]) -> io::Result<usize>;
}

pub(crate) struct FileSource;

impl ReadSource for FileSource {
    fn read(&mut self, file: &mut File, bytes: &mut [u8]) -> io::Result<usize> {
        file.read(bytes)
    }
}

pub(crate) fn find_library(
    name: &OsStr,
    directories: &[PathBuf],
    static_only: bool,
) -> Result<PathBuf> {
    let exact = name
        .as_encoded_bytes()
        .strip_prefix(b":")
        .map(OsStr::from_bytes);
    let basename = Path::new(exact.unwrap_or(name));
    if !matches!(basename.components().next(), Some(Component::Normal(_)))
        || basename.file_name() != Some(basename.as_os_str())
    {
        return Err(format!(
            "bld: -l{}: invalid library name",
            name.to_string_lossy()
        ));
    }
    let candidate = |suffix| {
        let mut filename = OsString::from("lib");
        filename.push(name);
        filename.push(suffix);
        filename
    };
    let archive = exact.map_or_else(|| candidate(".a"), OsStr::to_os_string);
    let shared = exact.is_none().then(|| candidate(".so"));
    let mut unavailable = None;
    for directory in directories {
        if let Some(shared) = &shared {
            let path = directory.join(shared);
            if exists(&path)? {
                if !static_only {
                    return Ok(path);
                }
                unavailable.get_or_insert(path);
            }
        }
        let path = directory.join(&archive);
        if exists(&path)? {
            return Ok(path);
        }
    }
    if let Some(path) = unavailable {
        return Err(format!(
            "{}: shared library cannot be used with -Bstatic",
            path.display()
        ));
    }
    Err(format!(
        "bld: -l{}: library not found in -L paths",
        name.to_string_lossy()
    ))
}

fn exists(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub(crate) fn find_script(path: PathBuf, directories: &[PathBuf]) -> Result<PathBuf> {
    // GNU ld §2.1: only -L options preceding -T extend script lookup.
    if !directories.is_empty() && path.is_relative() && !exists(&path)? {
        for directory in directories {
            let candidate = directory.join(&path);
            if exists(&candidate)? {
                return Ok(candidate);
            }
        }
    }
    Ok(path)
}

pub(crate) fn read_file(path: &Path, source: &mut dyn ReadSource) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let read_error = || format!("{}: cannot read link input", path.display());
    let metadata = file.metadata().map_err(|_| read_error())?;
    if !metadata.is_file() {
        return Err(read_error());
    }
    let length = usize::try_from(metadata.len()).map_err(|_| read_error())?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| read_error())?;
    bytes.resize(length, 0);
    // POSIX read permits EINTR and short transfers. The 64 KiB bound also
    // lets the Breenix ext2 reader batch contiguous blocks from an archive.
    let mut received = 0;
    while received < length {
        let count = CHUNK_SIZE.min(length - received);
        match source.read(&mut file, &mut bytes[received..received + count]) {
            Ok(0) => return Err(read_error()),
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(read_error()),
        }
    }
    Ok(bytes)
}

#[cfg(feature = "read-fault-probe")]
#[derive(Default)]
pub(crate) struct FaultSource {
    calls: usize,
}

#[cfg(feature = "read-fault-probe")]
impl ReadSource for FaultSource {
    fn read(&mut self, file: &mut File, bytes: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        match self.calls {
            1 => {
                eprintln!("read EINTR");
                Err(io::ErrorKind::Interrupted.into())
            }
            2 => {
                eprintln!("read short");
                let half = (bytes.len() / 2).max(1);
                file.read(&mut bytes[..half])
            }
            _ => {
                eprintln!("read");
                file.read(bytes)
            }
        }
    }
}

pub(crate) fn write_output(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    // Build inside a private directory on the destination filesystem. Creating
    // the file with the requested mode leaves the caller's umask in effect.
    let directory = loop {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".bld-{}-{sequence}", std::process::id()));
        match DirBuilder::new().mode(0o700).create(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    let temporary = directory.join("output");
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    let _ = fs::remove_dir(directory);
    result
}
