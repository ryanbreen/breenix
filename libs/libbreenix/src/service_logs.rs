//! Bounded persistent logs. One writer owns each file; descriptors are reopened
//! after rotation; services write pipes rather than archive descriptors.
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const LIMIT: u64 = 262_144;
pub const ARCHIVES: usize = 4;
pub const LINE_LIMIT: usize = 4096;

pub struct Log {
    path: PathBuf,
    header: Vec<u8>,
    lost: u64,
    initialized: bool,
}
impl Log {
    pub fn new(directory: &Path, name: &str, boot: &str) -> io::Result<Self> {
        let mut log = Self::deferred(directory, name, boot);
        log.append(b"")?;
        Ok(log)
    }
    /// Keep consuming pipes when the filesystem is unavailable; later writes retry.
    pub fn deferred(directory: &Path, name: &str, boot: &str) -> Self {
        Self { path: directory.join(format!("{}.log", name)),
            header: format!("BREENIX-LOG 1 boot={} source={} limit={} archives={}\n", boot, name, LIMIT, ARCHIVES).into_bytes(),
            lost: 0, initialized: false }
    }
    pub fn retry(&mut self) {
        if !self.initialized || self.lost > 0 { let _ = self.append(b""); }
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
        if !self.initialized {
            fs::create_dir_all(self.path.parent().unwrap())?;
            match std::fs::File::open(&self.path) {
                Ok(file) => {
                    let mut header = Vec::new();
                    file.take(self.header.len() as u64).read_to_end(&mut header)?;
                    if header != self.header { self.rotate()?; }
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => {},
                Err(e) => return Err(e),
            }
            self.initialized = true;
        }
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
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut bytes = vec![u8::from(self.discarded)];
        bytes.extend_from_slice(&self.pending);
        bytes
    }
    pub fn restore(bytes: &[u8]) -> Option<Self> {
        let (&discarded, pending) = bytes.split_first()?;
        if discarded > 1 || pending.len() > LINE_LIMIT || (discarded == 1 && !pending.is_empty()) { return None; }
        Some(Self { pending: pending.to_vec(), discarded: discarded == 1 })
    }
    pub fn feed(&mut self, bytes: &[u8], stream: &str, log: &mut Log) {
        self.feed_observed(bytes, stream, log, |_| {});
    }
    /// Observe complete, bounded lines before recording them. Partial or lost lines produce no receipt.
    pub fn feed_observed(&mut self, bytes: &[u8], stream: &str, log: &mut Log, mut observe: impl FnMut(&[u8])) {
        let mut output = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if self.discarded {
                    output.extend_from_slice(format!("[log] lost lines=1 stream={} reason=line-too-long\n", stream).as_bytes());
                } else {
                    observe(&self.pending);
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

/// One snapshot consumer. The cursor commits the input offset, partial line and
/// output identity/length together. Restore before retrying any failed apply:
/// an append may have reached disk without its cursor.
pub struct KmsgFollower {
    log: Log,
    state: PathBuf,
    boot: String,
    next: u64,
    lines: Lines,
    failed: bool,
}

use std::os::unix::fs::MetadataExt;

#[derive(Clone, Copy)]
struct LogFile { device: u64, inode: u64, length: u64 }
impl LogFile {
    fn name(self) -> String { format!("{}-{}", self.device, self.inode) }
}

fn remove_log_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn log_file(path: &Path) -> io::Result<Option<LogFile>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(LogFile { device: m.dev(), inode: m.ino(), length: m.len() })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn invalid_cursor() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid kernel log cursor or snapshot")
}

fn log_path(directory: &Path, index: usize) -> PathBuf {
    directory.join(if index == 0 { "kernel.log".into() } else { format!("kernel.log.{}", index) })
}

fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(if path.as_os_str().is_empty() { Path::new(".") } else { path })?.sync_all()
}

impl KmsgFollower {
    pub fn restore(directory: &Path, state: &Path, boot: &str) -> io::Result<Self> {
        let mut next = 0;
        let mut lines = Lines::new();
        match fs::read(state) {
            Ok(bytes) => {
                let mut parts = bytes.splitn(ARCHIVES + 3, |&b| b == b'\n');
                let header = std::str::from_utf8(parts.next().ok_or_else(invalid_cursor)?)
                    .map_err(|_| invalid_cursor())?;
                let fields: Vec<_> = header.split_whitespace().collect();
                if fields.len() != 4 || fields[..2] != ["BREENIX-KMSG-CURSOR", "1"] {
                    return Err(invalid_cursor());
                }
                let saved_boot = fields[2].strip_prefix("boot=").ok_or_else(invalid_cursor)?;
                let saved_next = fields[3].strip_prefix("next=").and_then(|s| s.parse().ok())
                    .ok_or_else(invalid_cursor)?;
                let mut files = Vec::new();
                for _ in 0..=ARCHIVES {
                    let line = std::str::from_utf8(parts.next().ok_or_else(invalid_cursor)?)
                        .map_err(|_| invalid_cursor())?;
                    if line == "-" { files.push(None); }
                    else {
                        let numbers: Vec<u64> = line.split_whitespace().map(str::parse)
                            .collect::<Result<_, _>>().map_err(|_| invalid_cursor())?;
                        if numbers.len() != 3 { return Err(invalid_cursor()); }
                        files.push(Some(LogFile { device: numbers[0], inode: numbers[1], length: numbers[2] }));
                    }
                }
                let saved_lines = Lines::restore(parts.next().ok_or_else(invalid_cursor)?)
                    .ok_or_else(invalid_cursor)?;
                if files[0].is_none() { return Err(invalid_cursor()); }
                // The cursor's hard links survive both rename-rotation and
                // replacement of an archive. Re-link before replaying input.
                for (index, saved) in files.into_iter().enumerate() {
                    let target = log_path(directory, index);
                    if let Some(saved) = saved {
                        let anchor = directory.join(".kernel-log-cursor").join(saved.name());
                        let actual = log_file(&anchor)?.ok_or_else(invalid_cursor)?;
                        if actual.device != saved.device || actual.inode != saved.inode
                            || actual.length < saved.length { return Err(invalid_cursor()); }
                        let file = OpenOptions::new().write(true).open(&anchor)?;
                        file.set_len(saved.length)?;
                        file.sync_all()?;
                        let temporary = directory.join(".kernel.log.restore");
                        remove_log_file(&temporary)?;
                        fs::hard_link(&anchor, &temporary)?;
                        fs::rename(&temporary, &target)?;
                        // rename is a no-op when both names already link the inode.
                        remove_log_file(&temporary)?;
                    } else { remove_log_file(&target)?; }
                }
                sync_directory(directory)?;
                if saved_boot == boot { next = saved_next; lines = saved_lines; }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }
        let mut follower = Self { log: Log::new(directory, "kernel", boot)?,
            state: state.to_owned(), boot: boot.to_owned(), next, lines, failed: false };
        follower.publish()?;
        Ok(follower)
    }

    fn publish(&mut self) -> io::Result<()> {
        if self.log.lost != 0 {
            return Err(io::Error::other("kernel log append failed; restore before retrying"));
        }
        let directory = self.log.path.parent().unwrap();
        let anchors = directory.join(".kernel-log-cursor");
        fs::create_dir_all(&anchors)?;
        fs::set_permissions(&anchors, fs::Permissions::from_mode(0o700))?;
        let mut retained = Vec::new();
        let mut saved = format!("BREENIX-KMSG-CURSOR 1 boot={} next={}\n", self.boot, self.next).into_bytes();
        for index in 0..=ARCHIVES {
            let path = log_path(directory, index);
            match log_file(&path)? {
                Some(m) => {
                    let name = m.name();
                    match fs::hard_link(&path, anchors.join(&name)) {
                        Ok(()) => {},
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {},
                        Err(e) => return Err(e),
                    }
                    retained.push(name);
                    saved.extend_from_slice(format!("{} {} {}\n", m.device, m.inode, m.length).as_bytes());
                },
                None => saved.extend_from_slice(b"-\n"),
            }
        }
        saved.extend_from_slice(&self.lines.checkpoint());
        // Persist the append and rotation names before committing their cursor.
        sync_directory(&anchors)?;
        sync_directory(directory)?;
        let mut temporary = self.state.as_os_str().to_os_string();
        temporary.push(".new");
        let mut file = OpenOptions::new().write(true).create(true).truncate(true)
            .mode(0o600).open(&temporary)?;
        file.write_all(&saved)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.state)?;
        sync_directory(self.state.parent().unwrap_or(Path::new(".")))?;
        // Only discard pins after the replacement cursor is durable.
        for entry in fs::read_dir(&anchors)? {
            let entry = entry?;
            if !retained.iter().any(|name| entry.file_name() == name.as_str()) {
                fs::remove_file(entry.path())?;
            }
        }
        sync_directory(&anchors)
    }

    pub fn apply(&mut self, snapshot: &str) -> io::Result<()> {
        if self.failed { return Err(io::Error::other("restore kernel log follower after an error")); }
        let result = self.apply_snapshot(snapshot);
        self.failed = result.is_err();
        result
    }

    fn apply_snapshot(&mut self, snapshot: &str) -> io::Result<()> {
        let (header, hex) = snapshot.split_once('\n').ok_or_else(invalid_cursor)?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 4 || fields[..2] != ["BREENIX-KMSG", "1"] { return Err(invalid_cursor()); }
        let start = fields[2].strip_prefix("start=").and_then(|s| s.parse::<u64>().ok()).ok_or_else(invalid_cursor)?;
        let end = fields[3].strip_prefix("end=").and_then(|s| s.parse::<u64>().ok()).ok_or_else(invalid_cursor)?;
        let hex = hex.trim().as_bytes();
        if hex.len() % 2 != 0 || end < start || (hex.len() / 2) as u64 != end - start {
            return Err(invalid_cursor());
        }
        let data: Vec<u8> = hex.chunks_exact(2).map(|pair| {
            std::str::from_utf8(pair).ok().and_then(|s| u8::from_str_radix(s, 16).ok()).ok_or_else(invalid_cursor)
        }).collect::<io::Result<_>>()?;
        if self.next < start {
            self.lines.finish("kernel", &mut self.log);
            if self.log.lost != 0 { return Err(io::Error::other("kernel log append failed")); }
            self.log.append(format!("[log] lost lines=unknown bytes={} reason=ring-overrun from={} to={}\n",
                start - self.next, self.next, start).as_bytes())?;
            self.next = start;
            self.publish()?;
        }
        if end > self.next && self.next >= start {
            for chunk in data[(self.next - start) as usize..].chunks(4096) {
                self.lines.feed(chunk, "kernel", &mut self.log);
                self.next += chunk.len() as u64;
                self.publish()?;
            }
        }
        Ok(())
    }
}
