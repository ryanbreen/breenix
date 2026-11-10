//! User-filesystem bridge acceptance, using ordinary file operations.
use breenix_suite::suite::{case, category, check, suite, CaseResult, Suite};
use libbreenix::{fs as sysfs, io as sysio, types::Fd};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const DAEMON: &str = "/usr/libexec/breenix/memfsd_test";
static SUITE: Suite = suite("network-shares", "Network shares", &[
    category("ufs", "User filesystem bridge", &[
        case("mount-list", "Mount and list", mount_list),
        case("create-write-read", "Create, write and read", create_write_read),
        case("rename-replace", "Rename replaces a file", rename_replace),
        case("dirs-unlink", "Directories and unlink", dirs_unlink),
        case("statfs", "Filesystem statistics", statfs),
        case("atomic-save", "Atomic file replacement", atomic_save),
        case("daemon-death", "Daemon death disconnects and unmount recovers", daemon_death),
        case("timeout-eio", "Request timeout returns EIO", timeout_eio),
        case("non-root-refused", "Non-root mount is refused", non_root_refused),
    ]),
]).case_limit_ms(30_000);

fn main() {
    SUITE.with_evidence_upload().run()
}
fn io<T>(r: std::io::Result<T>) -> Result<T, String> { r.map_err(|e| e.to_string()) }

struct Mount {
    dir: PathBuf,
    child: Option<Child>,
    output: Option<ChildStdout>,
    mounted: bool,
}
impl Mount {
    fn directory() -> Result<Self, String> {
        let dir = PathBuf::from(format!("/tmp/network-shares-{}", std::process::id()));
        io(fs::create_dir(&dir))?;
        let mount = Self { dir, child: None, output: None, mounted: false };
        io(fs::write(mount.dir.join("underlying"), b"ext2 contents"))?;
        Ok(mount)
    }
    fn start(stall: bool) -> Result<Self, String> {
        let mut mount = Self::directory()?;
        let mut cmd = Command::new(DAEMON);
        cmd.arg(&mount.dir).stdout(Stdio::piped());
        if stall { cmd.arg("--stall-reads"); }
        let mut child = io(cmd.spawn())?;
        mount.output = child.stdout.take();
        mount.child = Some(child);
        // The daemon may have mounted before a failed READY write; Drop
        // attempts unmount when this flag is set, including on handshake failure.
        mount.mounted = true;
        if mount.line()? != "READY" { return Err("daemon did not acknowledge mount".into()); }
        Ok(mount)
    }
    fn line(&mut self) -> Result<String, String> {
        let output = self.output.as_mut().ok_or("missing daemon output")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = Vec::new();
        while line.len() < 80 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("daemon acknowledgement watchdog expired".into()); }
            let mut poll = [sysio::PollFd::new(Fd::from_raw(output.as_raw_fd() as u64), sysio::poll_events::POLLIN)];
            sysio::poll(&mut poll, left.as_millis().min(10_000) as i32).map_err(|e| e.to_string())?;
            if poll[0].revents == 0 { continue; }
            let mut byte = [0];
            if io(output.read(&mut byte))? == 0 { return Err("daemon exited before acknowledgement".into()); }
            if byte[0] == b'\n' { return String::from_utf8(line).map_err(|e| e.to_string()); }
            line.push(byte[0]);
        }
        Err("oversized daemon acknowledgement".into())
    }
    fn kill(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            let killed = child.kill();
            let status = io(child.wait())?;
            io(killed)?;
            use std::os::unix::process::ExitStatusExt;
            if status.signal() != Some(9) { return Err(format!("daemon kill status {status}")); }
        }
        Ok(())
    }
    fn unmount(&mut self) -> Result<(), String> {
        sysfs::umount2(self.dir.to_str().ok_or("mount path encoding")?, 0).map_err(|e| e.to_string())?;
        self.mounted = false;
        Ok(())
    }
    fn finish(mut self) -> CaseResult {
        self.unmount()?;
        if let Some(mut child) = self.child.take() {
            let status = io(child.wait())?;
            check(status.success(), &format!("unmounted daemon failed: {status}"))?;
        }
        check(io(fs::read(self.dir.join("underlying")))? == b"ext2 contents", "unmount did not restore ext2 contents")?;
        io(fs::remove_dir_all(&self.dir))?;
        Ok(())
    }
}
impl Drop for Mount {
    fn drop(&mut self) {
        let _ = self.kill();
        if self.mounted { let _ = self.unmount(); }
        // Recursively remove the fixture only after its mount has been detached.
        if !self.mounted { let _ = fs::remove_dir_all(&self.dir); }
    }
}
fn mount_list() -> CaseResult {
    let mount = Mount::start(false)?;
    check(io(fs::read_dir(&mount.dir))?.count() == 0, "new memfs is not empty or underlying ext2 leaked")?;
    io(fs::write(mount.dir.join("listed"), b"entry"))?;
    let names: Result<Vec<_>, _> = io(fs::read_dir(&mount.dir))?.map(|e| e.map(|e| e.file_name())).collect();
    check(io(names)? == [std::ffi::OsString::from("listed")], "mounted directory listing differs")?;
    mount.finish()
}
fn create_write_read() -> CaseResult {
    let mount = Mount::start(false)?;
    let path = mount.dir.join("file");
    // Cross MAX_DATA so the bridge must chunk reads and writes.
    let bytes: Vec<u8> = (0..150_000).map(|n| (n % 251) as u8).collect();
    io(fs::write(&path, &bytes))?;
    check(io(fs::read(&path))? == bytes, "file bytes differ")?;
    let mut file = io(OpenOptions::new().write(true).open(&path))?;
    io(file.seek(SeekFrom::Start(17)))?;
    io(file.write_all(b"patch"))?;
    io(file.set_len(22))?;
    drop(file);
    let mut expected = bytes[..22].to_vec();
    expected[17..22].copy_from_slice(b"patch");
    check(io(fs::read(path))? == expected, "offset write or truncate differs")?;
    mount.finish()
}
fn rename_replace() -> CaseResult {
    let mount = Mount::start(false)?;
    let from = mount.dir.join("from");
    let to = mount.dir.join("to");
    io(fs::write(&from, b"new"))?;
    io(fs::write(&to, b"old"))?;
    io(fs::rename(&from, &to))?;
    check(!io(fs::exists(&from))? && io(fs::read(&to))? == b"new", "rename did not replace destination")?;
    mount.finish()
}
fn dirs_unlink() -> CaseResult {
    let mount = Mount::start(false)?;
    let dir = mount.dir.join("dir");
    io(fs::create_dir(&dir))?;
    let path = dir.join("child");
    io(fs::write(&path, b"open after unlink"))?;
    let mut file = io(File::open(&path))?;
    io(fs::remove_file(&path))?;
    check(!io(fs::exists(&path))?, "unlinked name still exists")?;
    let mut bytes = Vec::new();
    io(file.read_to_end(&mut bytes))?;
    check(bytes == b"open after unlink", "open unlinked handle lost its contents")?;
    drop(file);
    io(fs::remove_dir(&dir))?;
    check(!io(fs::exists(&dir))?, "removed directory still exists")?;
    mount.finish()
}
fn statfs() -> CaseResult {
    let mount = Mount::start(false)?;
    let file = io(File::open(&mount.dir))?;
    let stats = sysfs::fstatfs(Fd::from_raw(file.as_raw_fd() as u64))?;
    check(stats.f_type == ufs_abi::payload::MAGIC && stats.f_bsize == 4096
        && stats.f_blocks == 8192 && stats.f_bfree == 8192 && stats.f_bavail == 8192
        && stats.f_files == 4096 && stats.f_ffree == 4095 && stats.f_namelen == 255,
        &format!("ufs statfs reply differs: {stats:?}"))?;
    drop(file);
    mount.finish()
}
fn atomic_save() -> CaseResult {
    let mount = Mount::start(false)?;
    let path = mount.dir.join("drawing");
    io(fs::write(&path, b"old drawing"))?;
    let bytes = vec![0x97; 150_000];
    match libbreenix::atomic_file::save(&path, &bytes) {
        libbreenix::atomic_file::SaveResult::Saved => {},
        other => return Err(format!("atomic save: {other:?}").into()),
    }
    check(io(fs::read(&path))? == bytes, "atomic save bytes differ")?;
    let names: Result<Vec<_>, _> = io(fs::read_dir(&mount.dir))?.map(|e| e.map(|e| e.file_name())).collect();
    check(io(names)? == [std::ffi::OsString::from("drawing")], "atomic save left a temporary")?;
    mount.finish()
}
fn daemon_death() -> CaseResult {
    let mut mount = Mount::start(true)?;
    let path = mount.dir.join("blocked");
    io(fs::write(&path, b"will not be returned"))?;
    let reader = std::thread::spawn(move || fs::read(path));
    let line = mount.line()?;
    check(line.strip_prefix("READ ").and_then(|s| s.parse::<u64>().ok()).is_some(), "daemon did not receive the stalled read")?;
    mount.kill()?;
    let result = reader.join().map_err(|_| "reader thread panicked")?;
    check(result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(107), &format!("stalled read expected ENOTCONN: {result:?}"))?;
    let later = fs::metadata(mount.dir.join("blocked"));
    check(later.as_ref().err().and_then(|e| e.raw_os_error()) == Some(107), &format!("later lookup expected ENOTCONN: {later:?}"))?;
    mount.finish()
}
fn timeout_eio() -> CaseResult {
    let mount = Mount::start(true)?;
    let path = mount.dir.join("blocked");
    io(fs::write(&path, b"timeout"))?;
    let result = fs::read(path);
    check(result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(5), &format!("stalled read expected EIO: {result:?}"))?;
    // A timed-out request must not mark a live mount disconnected.
    io(fs::write(mount.dir.join("still-alive"), b"ok"))?;
    mount.finish()
}
fn non_root_refused() -> CaseResult {
    // Open the daemon endpoint as root so this judges mount's credential check,
    // rather than merely the device's permissions. Only the child drops uid.
    let mut mount = Mount::directory()?;
    let dev = io(OpenOptions::new().read(true).write(true).open("/dev/ufs"))?;
    let fd = dev.as_raw_fd() as u32;
    mount.mounted = true; // Also clean up an incorrectly accepted mount on failure.
    match libbreenix::process::fork()? {
        libbreenix::process::ForkResult::Child => {
            let ret = unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::SETUID, 1000) } as i64;
            if ret != 0 { libbreenix::process::exit(2); }
            // SAFETY: fd is live and ufs copies its u32 mount data during the call.
            let result = unsafe { sysfs::mount("memfs", mount.dir.to_str().unwrap(), "ufs", 0, (&fd as *const u32).cast()) };
            let refused = matches!(result, Err(libbreenix::error::Error::Os(e)) if e == libbreenix::errno::Errno::EPERM);
            if !refused { eprintln!("non-root mount expected EPERM: {result:?}"); }
            libbreenix::process::exit(if refused { 0 } else { 1 });
        }
        libbreenix::process::ForkResult::Parent(pid) => {
            let mut status = 0;
            libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0)?;
            check(status == 0, &format!("non-root mount child failed: {status:#x}"))?;
        }
    }
    check(io(fs::read(mount.dir.join("underlying")))? == b"ext2 contents", "refused mount hid ext2 contents")?;
    mount.mounted = false;
    Ok(())
}
