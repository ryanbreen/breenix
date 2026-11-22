//! Publish verified stdin bytes. The same-directory rename is the commit point.

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

#[cfg(not(test))]
fn main() -> std::process::ExitCode {
    guest::main()
}

#[cfg(test)]
#[path = "bput/transfer_host_tests.rs"]
mod transfer_host_tests;
