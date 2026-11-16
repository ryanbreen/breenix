//! Exercise the filesystem interface, cache and BreenFS guest volumes.
mod cache_reread;
mod filesystem_read;
mod filesystem_write;
use cache_reread::{reread_is_cached, REREAD_BYTES};
use breenix_suite::suite::{case, category, check, suite, CaseResult, Suite};
use libbreenix::{fs as sysfs, io as sysio, types::Fd};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirEntryExt, MetadataExt};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

static SUITE: Suite = suite("filesystem", "Copy-on-write filesystem", &[
    category("interface", "Mounts and file operations through the filesystem interface", &[
        case("mounts-listed", "Mounts are listed with their types and options", mounts_listed),
        case("cross-mount-exdev", "Links and renames across mounts fail with EXDEV", cross_mount_exdev),
        case("statfs-per-mount", "Each mount reports its own type and filesystem ID", statfs_per_mount),
        case("ino-consistent", "stat, fstat and getdents agree on an inode number", ino_consistent),
    ]),
    category("buffer-cache", "The shared buffer cache: verified misses, write groups and flush ordering", &[
        case("reread-from-cache", "A second file read issues no device reads", reread_from_cache),
        case("write-through", "Writes reach the device before returning", write_through),
        case("fsync-flush", "fsync flush follows the file's last device write", fsync_flush),
    ]),
    category("read", "A host-built filesystem reads back as its manifest", &[
        case("manifest-equal", "Types, metadata, hashes and link groups equal the host manifest", filesystem_read::manifest_equal),
        case("holes-and-tail", "Holes and the mapped partial tail are zero with only mapped blocks counted", filesystem_read::holes_and_tail),
        case("readdir-resume", "Small directory reads and cookie seeks enumerate 5000 names exactly once", filesystem_read::readdir_resume),
        case("read-only-refusals", "Write, create and unlink are refused with EROFS", filesystem_read::read_only_refusals),
    ]),
    category("write", "File and directory operations commit through transactions", &[
        case("remount-reads-back", "An fsynced tree reads back unchanged after remount", filesystem_write::remount_reads_back),
        case("rename-replace-atomic", "Rename replacement preserves open files and publishes the new name atomically", filesystem_write::rename_replace_atomic),
        case("unlinked-open-file", "An unlinked open file remains readable until close and is gone after remount", filesystem_write::unlinked_open_file),
        case("truncate-sparse", "Sparse writes and truncate preserve data and zero-filled holes after remount", filesystem_write::truncate_sparse),
        case("mmap-msync", "Shared mapped stores survive MS_SYNC and remount", filesystem_write::mmap_msync),
        case("commit-without-fsync", "The periodic commit makes an unfsynced write durable", filesystem_write::commit_without_fsync),
    ]),
]);

fn io<T>(result: std::io::Result<T>) -> Result<T, String> { result.map_err(|e| e.to_string()) }

#[derive(Debug)]
struct CacheCounters { hits: u64, misses: u64, reads: u64, writes: u64, write_seq: u64, pending: u64, flush_seq: u64 }

fn cache_counters() -> Result<CacheCounters, String> {
    let text = io(fs::read_to_string("/proc/block/cache"))?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 { return Err(format!("invalid cache counter row: {line}")); }
        let count = fields[1].parse::<u64>().map_err(|e| e.to_string())?;
        let sequence = fields[2].parse::<u64>().map_err(|e| e.to_string())?;
        rows.push((fields[0], count, sequence));
    }
    let names = ["hits", "misses", "device_reads", "device_writes", "pending_writes", "flushes"];
    if rows.len() != names.len() || rows.iter().zip(names).any(|(row, name)| row.0 != name) {
        return Err(format!("invalid cache counter table: {text:?}"));
    }
    Ok(CacheCounters { hits: rows[0].1, misses: rows[1].1, reads: rows[2].1, writes: rows[3].1, write_seq: rows[3].2,
        pending: rows[4].1, flush_seq: rows[5].2 })
}

struct CacheFile(PathBuf);
impl CacheFile {
    fn new(name: &str) -> Self { Self(PathBuf::from(format!("/tmp/filesystem-cache-{name}-{}", std::process::id()))) }
}
impl Drop for CacheFile { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }

fn reread_from_cache() -> CaseResult {
    let path = CacheFile::new("reread");
    let data = vec![0x5a; REREAD_BYTES];
    { let mut file = io(File::create(&path.0))?; io(file.write_all(&data))?; }
    let mut first = Vec::new();
    io(io(File::open(&path.0))?.read_to_end(&mut first))?;
    check(first == data, "first read differs from written data")?;
    let before = cache_counters()?;
    let mut second = Vec::new();
    io(io(File::open(&path.0))?.read_to_end(&mut second))?;
    let after = cache_counters()?;
    check(second == data, "second read differs from written data")?;
    check(reread_is_cached((before.hits, before.misses, before.reads),
        (after.hits, after.misses, after.reads)),
        &format!("reread cache evidence failed: {before:?} -> {after:?}"))
}

fn write_through() -> CaseResult {
    let path = CacheFile::new("write-through");
    let mut file = io(File::create(&path.0))?;
    for byte in 0..4u8 {
        let before = cache_counters()?;
        let data = [byte; 4096];
        let written = io(file.write(&data))?;
        let after = cache_counters()?;
        check(written == data.len(), &format!("short write: {written}"))?;
        check(after.writes > before.writes && after.pending == 0,
            &format!("write did not complete: {before:?} -> {after:?}"))?;
    }
    Ok(())
}

fn fsync_flush() -> CaseResult {
    let path = CacheFile::new("fsync");
    let mut file = io(File::create(&path.0))?;
    io(file.write_all(&[0x37; 4096]))?;
    let after_write = cache_counters()?;
    io(file.sync_all())?;
    let after_sync = cache_counters()?;
    check(after_write.write_seq != 0 && after_sync.flush_seq > after_write.write_seq,
        &format!("flush did not follow write: {after_write:?} -> {after_sync:?}"))
}

struct Mount { dir: PathBuf, child: Option<Child>, output: Option<ChildStdout>, mounted: bool }
impl Mount {
    fn start() -> Result<Self, String> {
        let dir = PathBuf::from(format!("/tmp/filesystem-interface-{}", std::process::id()));
        io(fs::create_dir(&dir))?;
        let mut mount = Self { dir, child: None, output: None, mounted: false };
        let mut child = io(Command::new("/usr/libexec/breenix/memfsd_test").arg(&mount.dir)
            .stdout(Stdio::piped()).spawn())?;
        mount.output = child.stdout.take();
        mount.child = Some(child);
        mount.mounted = true;
        let output = mount.output.as_mut().ok_or("missing daemon output")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("daemon acknowledgement watchdog expired".into()); }
            let mut poll = [sysio::PollFd::new(Fd::from_raw(output.as_raw_fd() as u64), sysio::poll_events::POLLIN)];
            sysio::poll(&mut poll, left.as_millis().min(10_000) as i32).map_err(|e| e.to_string())?;
            if poll[0].revents == 0 { continue; }
            let mut byte = [0];
            if io(output.read(&mut byte))? == 0 { return Err("daemon exited before acknowledgement".into()); }
            if byte[0] == b'\n' { break; }
            line.push(byte[0]);
            if line.len() >= 80 { return Err("oversized daemon acknowledgement".into()); }
        }
        if line != b"READY" { return Err("daemon did not acknowledge mount".into()); }
        Ok(mount)
    }
    fn finish(mut self) -> CaseResult {
        sysfs::umount2(self.dir.to_str().ok_or("mount path encoding")?, 0)?;
        self.mounted = false;
        if let Some(mut child) = self.child.take() {
            let status = io(child.wait())?;
            check(status.success(), &format!("daemon failed: {status}"))?;
        }
        io(fs::remove_dir_all(&self.dir))?;
        Ok(())
    }
}
impl Drop for Mount {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() { let _ = child.kill(); let _ = child.wait(); }
        if self.mounted && sysfs::umount2(self.dir.to_str().unwrap_or(""), 0).is_ok() { self.mounted = false; }
        if !self.mounted { let _ = fs::remove_dir_all(&self.dir); }
    }
}

fn mounts_listed() -> CaseResult {
    let mount = Mount::start()?;
    let text = io(fs::read_to_string("/proc/mounts"))?;
    for (path, kind, options) in [("/", "ext2", "rw"), ("/dev", "devfs", "rw"),
        ("/proc", "procfs", "rw"), (mount.dir.to_str().ok_or("mount path encoding")?, "ufs", "rw")] {
        let matches: Vec<_> = text.lines().filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 6 && fields[1] == path).then_some(fields)
        }).collect();
        check(matches.len() == 1 && matches[0][2] == kind && matches[0][3] == options,
            &format!("mount {path} expected {kind} {options}, got {matches:?}"))?;
    }
    mount.finish()
}
fn cross_mount_exdev() -> CaseResult {
    let mount = Mount::start()?;
    let root_file = mount.dir.with_extension("file");
    let user_file = mount.dir.join("file");
    io(fs::write(&root_file, b"root"))?;
    io(fs::write(&user_file, b"user"))?;
    for (from, to) in [(&root_file, mount.dir.join("link")), (&user_file, mount.dir.with_extension("link"))] {
        let result = fs::hard_link(from, &to);
        check(result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(18), &format!("link expected EXDEV: {result:?}"))?;
        let result = fs::rename(from, &to);
        check(result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(18), &format!("rename expected EXDEV: {result:?}"))?;
    }
    io(fs::remove_file(root_file))?;
    mount.finish()
}
fn statfs_per_mount() -> CaseResult {
    let mount = Mount::start()?;
    let mut ids = Vec::new();
    for (path, expected) in [("/", 0xef53), ("/dev", 0x1373), ("/proc", 0x9fa0),
        (mount.dir.to_str().ok_or("mount path encoding")?, 0x55465331)] {
        let path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let mut stats = sysfs::Statfs::default();
        #[cfg(target_arch = "x86_64")] const STATFS: u64 = 137;
        #[cfg(target_arch = "aarch64")] const STATFS: u64 = 43;
        let result = unsafe { libbreenix::syscall::raw::syscall2(STATFS, path.as_ptr() as u64, &mut stats as *mut _ as u64) } as i64;
        check(result == 0 && stats.f_type == expected, &format!("statfs {path:?}: result={result}, stats={stats:?}"))?;
        check(stats.f_fsid != [0, 0] && !ids.contains(&stats.f_fsid), &format!("statfs {path:?} has duplicate or zero fsid {:?}", stats.f_fsid))?;
        ids.push(stats.f_fsid);
    }
    mount.finish()
}
fn ino_consistent() -> CaseResult {
    let dir = PathBuf::from(format!("/tmp/filesystem-inode-{}", std::process::id()));
    io(fs::create_dir(&dir))?;
    let path = dir.join("file");
    io(fs::write(&path, b"inode"))?;
    let file = io(File::open(&path))?;
    let stat = io(fs::metadata(&path))?.ino();
    let fstat = io(file.metadata())?.ino();
    let mut found = None;
    for entry in io(fs::read_dir(&dir))? {
        let entry = io(entry)?;
        if entry.file_name() == "file" { found = Some(entry.ino()); }
    }
    let result = check(stat != 0 && stat == fstat && found == Some(stat), &format!("inode mismatch: stat={stat}, fstat={fstat}, getdents={found:?}"));
    drop(file);
    io(fs::remove_dir_all(dir))?;
    result
}

fn main() { SUITE.run() }
