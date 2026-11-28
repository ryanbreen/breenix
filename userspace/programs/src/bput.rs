//! Publish verified bytes. The rename into the destination directory is
//! the commit point. The bytes come from stdin into a temporary file, or
//! with --adopt from a file an upload already staged on the same file
//! system, which is verified and renamed without being written again.

use std::io;

#[cfg(not(test))]
#[path = "bput/guest.rs"]
mod guest;
#[cfg(not(test))]
use libbreenix::crypto::sha256::Sha256;
#[cfg(test)]
#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;
#[cfg(test)]
use sha256::Sha256;

trait PublishIo {
    type File;
    /// An open staging directory (--adopt).
    type Dir;

    fn open_dir(&mut self) -> io::Result<()>;
    fn create_temp(&mut self) -> io::Result<Self::File>;
    fn read_stdin(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_temp(&mut self, file: &mut Self::File, buf: &[u8]) -> io::Result<usize>;
    fn progress(&mut self, received: u64) -> io::Result<()>;
    fn chmod(&mut self, file: &Self::File, mode: u32) -> io::Result<()>;
    fn sync_file(&mut self, file: &Self::File) -> io::Result<()>;
    fn rename(&mut self) -> io::Result<()>;
    fn sync_dir(&mut self) -> io::Result<()>;
    fn unlink_temp(&mut self) -> io::Result<()>;
    /// Open the staging directory, refusing a symbolic link. It stays open
    /// so the payload is opened and renamed from the directory it was
    /// verified in, even if its path is given to another one meanwhile.
    fn open_staging_dir(&mut self) -> io::Result<Self::Dir>;
    /// Whether the staging directory is the caller's alone: owned by the
    /// caller, and nobody else may add, remove or rename entries in it.
    fn staging_is_private(&mut self, dir: &Self::Dir) -> io::Result<bool>;
    /// Open the staged upload in `dir` for reading, refusing a symbolic link.
    fn open_staged(&mut self, dir: &Self::Dir) -> io::Result<Self::File>;
    fn read_staged(&mut self, file: &mut Self::File, buf: &mut [u8]) -> io::Result<usize>;
    /// Rename the staged upload in `dir` over the destination.
    fn rename_staged(&mut self, dir: &Self::Dir) -> io::Result<()>;
}

#[derive(Debug)]
struct Failure {
    status: u8,
    reason: String,
}

fn at(operation: &str) -> impl FnOnce(io::Error) -> Failure + '_ {
    move |error| Failure {
        status: 1,
        reason: format!("{operation}: {error}"),
    }
}

fn prepare(
    sys: &mut impl PublishIo,
    length: u64,
    digest: [u8; 32],
    mode: u32,
) -> Result<(), Failure> {
    sys.open_dir().map_err(at("open directory"))?;
    let mut file = sys.create_temp().map_err(at("open temporary file"))?;
    let result = (|| {
        let mut hash = Sha256::new();
        let mut received = 0;
        let mut buf = [0u8; 32 * 1024];
        loop {
            let n = sys.read_stdin(&mut buf).map_err(at("stdin"))?;
            if n == 0 {
                break;
            }
            if n as u64 > length - received {
                return Err(at("long stdin")(io::ErrorKind::InvalidData.into()));
            }
            let mut pending = &buf[..n];
            hash.update(pending);
            while !pending.is_empty() {
                let written = sys.write_temp(&mut file, pending).map_err(at("write"))?;
                if written == 0 {
                    return Err(at("write")(io::ErrorKind::WriteZero.into()));
                }
                pending = &pending[written..];
            }
            if received == 0 {
                sys.progress(n as u64).map_err(at("progress"))?;
            }
            received += n as u64;
        }
        if received != length {
            return Err(Failure {
                status: 1,
                reason: format!("short stdin: received {received} of {length} bytes"),
            });
        }
        if hash.finalize() != digest {
            return Err(Failure {
                status: 1,
                reason: "SHA-256 mismatch".into(),
            });
        }
        sys.chmod(&file, mode).map_err(at("chmod"))?;
        sys.sync_file(&file).map_err(at("file fsync"))?;
        drop(file);
        sys.rename().map_err(at("rename"))
    })();
    result.map_err(|mut failure| {
        if let Err(error) = sys.unlink_temp() {
            failure
                .reason
                .push_str(&format!("; unlink temporary file: {error}"));
        }
        failure
    })
}

fn publish(
    sys: &mut impl PublishIo,
    length: u64,
    digest: [u8; 32],
    mode: u32,
) -> Result<(), Failure> {
    prepare(sys, length, digest, mode)?;
    sys.sync_dir().map_err(|error| Failure {
        status: 3,
        reason: format!("directory sync failed: {error}"),
    })
}

/// Verify a staged upload by reading it, then publish it in place. A file
/// that does not verify stays staged; its staging directory's owner
/// removes it.
///
/// The staging directory sits in the destination directory, which others
/// may be able to write. So it must be the caller's alone, which keeps
/// anyone else from changing the payload's name inside it, and it is held
/// open from verification to rename, so replacing the directory at its
/// path cannot change which file is published.
fn adopt(
    sys: &mut impl PublishIo,
    length: u64,
    digest: [u8; 32],
    mode: u32,
) -> Result<(), Failure> {
    sys.open_dir().map_err(at("open directory"))?;
    let stage = sys.open_staging_dir().map_err(at("open staging directory"))?;
    if !sys.staging_is_private(&stage).map_err(at("inspect staging directory"))? {
        return Err(Failure {
            status: 1,
            reason: "staging directory is not private to its owner".into(),
        });
    }
    let mut file = sys.open_staged(&stage).map_err(at("open staged file"))?;
    let mut hash = Sha256::new();
    let mut received = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = sys.read_staged(&mut file, &mut buf).map_err(at("read staged file"))?;
        if n == 0 {
            break;
        }
        if n as u64 > length - received {
            return Err(at("long staged file")(io::ErrorKind::InvalidData.into()));
        }
        hash.update(&buf[..n]);
        received += n as u64;
    }
    if received != length {
        return Err(Failure {
            status: 1,
            reason: format!("short staged file: {received} of {length} bytes"),
        });
    }
    if hash.finalize() != digest {
        return Err(Failure {
            status: 1,
            reason: "SHA-256 mismatch".into(),
        });
    }
    sys.chmod(&file, mode).map_err(at("chmod"))?;
    sys.sync_file(&file).map_err(at("file fsync"))?;
    drop(file);
    sys.rename_staged(&stage).map_err(at("rename staged file"))?;
    sys.sync_dir().map_err(|error| Failure {
        status: 3,
        reason: format!("directory sync failed: {error}"),
    })
}

#[cfg(not(test))]
fn main() -> std::process::ExitCode {
    guest::main()
}

#[cfg(test)]
#[path = "bput/transfer_host_tests.rs"]
mod transfer_host_tests;
