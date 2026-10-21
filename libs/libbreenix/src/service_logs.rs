//! Bounded persistent logs. One writer owns each file; descriptors are reopened
//! after rotation; services write pipes rather than archive descriptors.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const LIMIT: u64 = 262_144;
pub const ARCHIVES: usize = 4;
pub const LINE_LIMIT: usize = 4096;

pub struct Log {
    path: PathBuf,
    header: Vec<u8>,
    lost: u64,
}
impl Log {
    pub fn new(directory: &Path, name: &str, boot: &str) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let mut log = Self { path: directory.join(format!("{}.log", name)),
            header: format!("BREENIX-LOG 1 boot={} source={} limit={} archives={}\n", boot, name, LIMIT, ARCHIVES).into_bytes(), lost: 0 };
        let previous = fs::read(&log.path);
        match previous {
            Ok(bytes) if !bytes.starts_with(&log.header) => log.rotate()?,
            Ok(_) => {},
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }
        log.append(b"")?;
        Ok(log)
    }
    fn rotate(&mut self) -> io::Result<()> {
        for index in (1..=ARCHIVES).rev() {
            let from = if index == 1 { self.path.clone() } else { self.path.with_extension(format!("log.{}", index - 1)) };
            let to = self.path.with_extension(format!("log.{}", index));
            match fs::rename(from, to) {
                Ok(()) => {
                    let archive = OpenOptions::new().append(true).open(self.path.with_extension(format!("log.{}", index)))?;
                    protect(&archive)?;
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => {},
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn append(&mut self, data: &[u8]) -> io::Result<()> {
        let size = match fs::metadata(&self.path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        let gap = if self.lost > 0 { format!("\n[log] lost lines={} reason=write-error\n", self.lost) } else { String::new() };
        if size.max(self.header.len() as u64) + data.len() as u64 + gap.len() as u64 > LIMIT { self.rotate()?; }
        let mut file = OpenOptions::new().create(true).append(true).mode(0o640).open(&self.path)?;
        protect(&file)?;
        if file.metadata()?.len() == 0 { file.write_all(&self.header)?; }
        file.write_all(gap.as_bytes())?;
        file.write_all(data)?;
        file.sync_all()?;
        self.lost = 0;
        Ok(())
    }
    pub fn record(&mut self, data: &[u8]) {
        if self.append(data).is_err() { self.lost = self.lost.saturating_add(data.iter().filter(|&&b| b == b'\n').count().max(1) as u64); }
    }
}
use std::os::unix::io::AsRawFd;
extern "C" { fn fchown(fd: i32, uid: u32, gid: u32) -> i32; }
fn protect(file: &std::fs::File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o640))?;
    // adm is the system group from the users design, not a site identity.
    if unsafe { fchown(file.as_raw_fd(), 0, 4) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

/// A stream's bounded partial line. Oversized lines are discarded through the
/// newline and reported, rather than silently truncated or allowed to grow RAM.
pub struct Lines { pending: Vec<u8>, discarded: bool }
impl Lines {
    pub fn new() -> Self { Self { pending: Vec::new(), discarded: false } }
    pub fn feed(&mut self, bytes: &[u8], stream: &str, log: &mut Log) {
        let mut output = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if self.discarded {
                    output.extend_from_slice(format!("[log] lost lines=1 stream={} reason=line-too-long\n", stream).as_bytes());
                } else {
                    output.extend_from_slice(format!("[{}] ", stream).as_bytes());
                    output.extend_from_slice(&self.pending);
                    output.push(b'\n');
                }
                self.pending.clear();
                self.discarded = false;
            } else if !self.discarded {
                if self.pending.len() == LINE_LIMIT { self.pending.clear(); self.discarded = true; }
                else { self.pending.push(byte); }
            }
        }
        // A batch is bounded by the reader's 4096-byte read plus one partial line.
        if !output.is_empty() { log.record(&output); }
    }
    pub fn finish(&mut self, stream: &str, log: &mut Log) {
        if !self.pending.is_empty() || self.discarded { self.feed(b"\n", stream, log); }
    }
}
