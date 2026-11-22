// Host-check interface for the publication contract.
#[cfg(test)]
use std::io;

#[cfg(test)]
pub trait PublishIo {
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

#[cfg(test)]
#[derive(Debug)]
struct Failure {
    status: u8,
    reason: String,
}

#[cfg(test)]
fn publish(
    _sys: &mut impl PublishIo,
    _length: u64,
    _digest: [u8; 32],
    _mode: u32,
) -> Result<(), Failure> {
    Err(Failure {
        status: 1,
        reason: "publication is unavailable".into(),
    })
}

#[cfg(test)]
#[path = "bput/transfer_host_tests.rs"]
pub mod transfer_host_tests;
