use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
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

pub(crate) fn write_executable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    // Build inside a private directory on the destination filesystem. Creating
    // the file with mode 0777 leaves the caller's umask in effect.
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
            .mode(0o777)
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
