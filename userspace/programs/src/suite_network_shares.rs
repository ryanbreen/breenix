//! User-filesystem bridge and SMB acceptance, using ordinary file operations.
use breenix_suite::suite::{case, category, check, suite, CaseResult, Suite};
use libbreenix::{fs as sysfs, io as sysio, types::Fd};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const DAEMON: &str = "/usr/libexec/breenix/memfsd_test";
static SUITE: Suite = suite("network-shares", "Network shares", &[
    category("ufs", "User filesystem bridge", &[
        case("mount-list", "Mount and list", mount_list),
        case("create-write-read", "Create, write and read", create_write_read),
        case("rename-replace", "Rename replaces a file", rename_replace),
        case("directory-relative", "Working directories and directory-relative operations", directory_relative),
        case("write-open-progress", "Open proceeds during a chunked write", write_open_progress),
        case("dirs-unlink", "Directories and unlink", dirs_unlink),
        case("statfs", "Filesystem statistics", statfs),
        case("atomic-save", "Atomic file replacement", atomic_save),
        case("daemon-death", "Daemon death disconnects and unmount recovers", daemon_death),
        case("timeout-eio", "Request timeout returns EIO", timeout_eio),
        case("non-root-refused", "Non-root mount is refused", non_root_refused),
    ]),
    category("smb", "SMB shares", &[
        case("shares-mount-list", "Mount and list shares", smb_mount_list),
        case("write-read", "Write and read", smb_write_read),
        case("rename-replace", "Rename replaces a file", smb_rename_replace),
        case("delete", "Delete a file", smb_delete),
        case("dirs", "Create and remove directories", smb_dirs),
        case("atomic-save", "Atomic save of a large file", smb_atomic_save),
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
        Self::start_with(if stall { Some("--stall-reads") } else { None })
    }
    fn start_with(flag: Option<&str>) -> Result<Self, String> {
        let mut mount = Self::directory()?;
        let mut cmd = Command::new(DAEMON);
        cmd.arg(&mount.dir).stdout(Stdio::piped());
        if let Some(flag) = flag { cmd.arg(flag); }
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
    let exclusive = OpenOptions::new().write(true).create_new(true).open(&path);
    check(exclusive.err().and_then(|e| e.raw_os_error()) == Some(17), "exclusive create did not refuse existing name")?;
    let mut file = io(OpenOptions::new().read(true).write(true).open(&path))?;
    check(io(file.metadata())?.len() == bytes.len() as u64, "fstat size differs")?;
    io(file.seek(SeekFrom::Start(17)))?;
    check(io(file.write_at(b"positioned", 70_000))? == 10, "positioned write was short")?;
    let mut positioned = [0; 10];
    check(io(file.read_at(&mut positioned, 70_000))? == 10 && positioned == *b"positioned", "positioned I/O differs")?;
    check(io(file.stream_position())? == 17, "positioned I/O moved shared cursor")?;
    io(file.write_all(b"patch"))?;
    io(file.set_len(22))?;
    drop(file);
    let mut expected = bytes[..22].to_vec();
    expected[17..22].copy_from_slice(b"patch");
    check(io(fs::read(&path))? == expected, "offset write or truncate differs")?;
    let append = io(OpenOptions::new().append(true).open(&path))?;
    let mut duplicate = io(append.try_clone())?;
    drop(append);
    io(duplicate.seek(SeekFrom::Start(0)))?;
    io(duplicate.write_all(b"tail"))?;
    io(duplicate.sync_all())?;
    drop(duplicate);
    expected.extend_from_slice(b"tail");
    check(io(fs::read(path))? == expected, "append or duplicate lifetime differs")?;
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
fn write_open_progress() -> CaseResult {
    let mut mount = Mount::start_with(Some("--hold-write-until-open"))?;
    let other = mount.dir.join("other");
    drop(io(File::create(&other))?);
    let path = mount.dir.join("large");
    let mut writer = io(File::create(&path))?;
    let bytes = vec![0x9d; 150_000];
    let write = std::thread::spawn(move || writer.write_all(&bytes));
    check(mount.line()?.starts_with("WRITE "), "daemon did not hold the write reply")?;
    // The daemon releases the pending write only after receiving this Open.
    let opened = File::open(other);
    let written = write.join().map_err(|_| "writer panicked")?;
    drop(io(opened)?);
    io(written)?;
    check(io(fs::read(path))? == vec![0x9d; 150_000], "interleaved write bytes differ")?;
    mount.finish()
}
fn directory_relative() -> CaseResult {
    struct RestoreCwd(PathBuf);
    impl Drop for RestoreCwd {
        fn drop(&mut self) { let _ = std::env::set_current_dir(&self.0); }
    }
    let mount = Mount::start(false)?;
    // Search permission is enough for chdir, even without directory read access.
    use std::os::unix::fs::DirBuilderExt;
    let search_only = mount.dir.join("search-only");
    io(fs::DirBuilder::new().mode(0o111).create(&search_only))?;
    match libbreenix::process::fork()? {
        libbreenix::process::ForkResult::Child => {
            let uid = unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::SETUID, 1000) } as i64;
            let entered = uid == 0 && std::env::set_current_dir(&search_only).is_ok();
            libbreenix::process::exit(if entered { 0 } else { 1 });
        }
        libbreenix::process::ForkResult::Parent(pid) => {
            let mut status = 0;
            libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0)?;
            check(status == 0, &format!("search-only chdir failed: {status:#x}"))?;
        }
    }
    {
        let restore = RestoreCwd(io(std::env::current_dir())?);
        let old = mount.dir.join("old");
        let renamed = mount.dir.join("renamed");
        io(fs::create_dir(&old))?;
        let directory = io(File::open(&old))?;
        io(std::env::set_current_dir(&old))?;
        io(fs::write("relative", b"from cwd"))?;
        io(fs::rename(&old, &renamed))?;
        check(io(std::env::current_dir())? == renamed, "cwd did not follow directory rename")?;
        let name = b"relative\0";
        let fd = directory.as_raw_fd() as u64;
        let raw = unsafe { libbreenix::syscall::raw::syscall4(
            libbreenix::syscall::nr::OPENAT, fd, name.as_ptr() as u64, 0, 0) } as i64;
        check(raw >= 0, &format!("openat failed: {raw}"))?;
        let mut opened = unsafe { File::from_raw_fd(raw as i32) };
        let mut bytes = Vec::new();
        io(opened.read_to_end(&mut bytes))?;
        check(bytes == b"from cwd", "openat opened the wrong object")?;
        let mut stat = sysfs::Stat::new();
        let result = unsafe { libbreenix::syscall::raw::syscall4(
            libbreenix::syscall::nr::NEWFSTATAT, fd, name.as_ptr() as u64, (&mut stat as *mut sysfs::Stat) as u64, 0) } as i64;
        check(result == 0 && stat.st_size == 8, &format!("fstatat failed: {result}"))?;
        let result = unsafe { libbreenix::syscall::raw::syscall3(
            libbreenix::syscall::nr::UNLINKAT, fd, name.as_ptr() as u64, 0) } as i64;
        check(result == 0, &format!("unlinkat failed: {result}"))?;
        check(!io(fs::exists("relative"))?, "unlinkat left the cwd entry")?;
        io(std::env::set_current_dir(&mount.dir))?;
        let result = unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::FCHDIR, fd) } as i64;
        check(result == 0 && io(std::env::current_dir())? == renamed, "fchdir lost the directory identity")?;
        io(std::env::set_current_dir(".."))?;
        check(io(std::env::current_dir())? == mount.dir, "cwd parent did not reach mount root")?;
        io(std::env::set_current_dir(".."))?;
        check(Some(io(std::env::current_dir())?.as_path()) == mount.dir.parent(), "cwd parent did not leave the mount")?;
        drop(restore);
    }
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

struct SmbMount {
    dir: PathBuf,
    work: PathBuf,
    work_created: bool,
    mounted: bool,
    ready: String,
}
impl SmbMount {
    fn start(share: &str, smb302: bool) -> Result<Self, String> {
        let server = io(fs::read_to_string("/etc/breenix/smb-test-server"))?;
        let fields: Vec<_> = server.split_whitespace().collect();
        if fields.len() != 2 || fields[1].parse::<u16>().ok().filter(|p| *p != 0).is_none() {
            return Err("invalid staged SMB endpoint".into());
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?.as_nanos();
        let name = format!("breenix-suite-{}-{stamp}", std::process::id());
        let dir = PathBuf::from("/tmp").join(&name);
        io(fs::create_dir(&dir))?;
        let mut mount = Self {
            work: dir.join(&name), dir, work_created: false, mounted: false, ready: String::new(),
        };
        let source = format!("//{}/{share}", fields[0]);
        let mut options = format!("credfile=/etc/breenix/smb-test.cred,port={}", fields[1]);
        if smb302 { options.push_str(",vers=3.0.2"); }
        // A failed handshake may follow a successful mount. Drop must detach it.
        mount.mounted = true;
        let output = io(Command::new("/sbin/mount")
            .args(["-t", "smb", &source])
            .arg(&mount.dir).args(["-o", &options])
            .stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn())?;
        // Keep daemon stderr inherited: capturing it would hold this pipe open
        // until unmount, while we are still waiting for mount to finish.
        let output = io(output.wait_with_output())?;
        if !output.status.success() { return Err(format!("SMB mount failed: {}", output.status)); }
        mount.ready = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
        if !mount.ready.starts_with("READY dialect=") || !mount.ready.ends_with('\n') {
            return Err("SMB mount did not report READY".into());
        }
        if share != "ro" {
            io(fs::create_dir(&mount.work))?;
            mount.work_created = true;
        }
        Ok(mount)
    }
    fn rw() -> Result<Self, String> { Self::start("rw", false) }
    fn path(&self, name: &str) -> PathBuf { self.work.join(name) }
    fn clean_work(&mut self) -> Result<(), String> {
        if self.work_created {
            io(fs::remove_dir_all(&self.work))?;
            self.work_created = false;
        }
        Ok(())
    }
    fn finish(mut self) -> CaseResult {
        self.clean_work()?;
        sysfs::umount2(self.dir.to_str().ok_or("mount path encoding")?, 0)?;
        self.mounted = false;
        io(fs::remove_dir(&self.dir))?;
        Ok(())
    }
}
impl Drop for SmbMount {
    fn drop(&mut self) {
        if let Err(e) = self.clean_work() { eprintln!("SMB fixture cleanup: {e}"); }
        if self.mounted {
            match sysfs::umount2(self.dir.to_str().unwrap_or(""), 0) {
                Ok(()) | Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINVAL)) => self.mounted = false,
                Err(e) => eprintln!("SMB fixture unmount: {e}"),
            }
        }
        // Remove only the local mount point here; recursive cleanup uses work.
        if !self.mounted { let _ = fs::remove_dir(&self.dir); }
    }
}
fn smb_names(dir: &std::path::Path) -> Result<Vec<std::ffi::OsString>, String> {
    io(io(fs::read_dir(dir))?.map(|e| e.map(|e| e.file_name())).collect())
}
fn smb_mount_list() -> CaseResult {
    for (share, smb302, expected) in [
        ("rw", false, "READY dialect=0x0311 encrypted=0\n"),
        ("rw302", true, "READY dialect=0x0302 encrypted=0\n"),
        ("enc", false, "READY dialect=0x0311 encrypted=1\n"),
        ("ro", false, "READY dialect=0x0311 encrypted=0\n"),
    ] {
        let mount = SmbMount::start(share, smb302)?;
        check(mount.ready == expected, &format!("{share} security state differs: {:?}", mount.ready))?;
        let names = smb_names(&mount.dir)?;
        let expected_name = if share == "ro" {
            std::ffi::OsStr::new("README.txt")
        } else {
            mount.work.file_name().ok_or("missing work directory name")?
        };
        check(names.iter().any(|n| n == expected_name), &format!("{share} listing missed its fixture"))?;
        mount.finish()?;
    }
    Ok(())
}
fn smb_write_read() -> CaseResult {
    let mount = SmbMount::rw()?;
    let path = mount.path("chunks");
    // Exceed both the UFS payload and SMB single-credit size with varying bytes.
    let bytes: Vec<u8> = (0..(3 * ufs_abi::MAX_DATA + 317)).map(|n| ((n * 37 + n / 251) % 256) as u8).collect();
    io(fs::write(&path, &bytes))?;
    check(io(fs::read(&path))? == bytes, "SMB chunked read differs from written bytes")?;
    mount.finish()
}
fn smb_rename_replace() -> CaseResult {
    let mount = SmbMount::rw()?;
    let from = mount.path("from");
    let to = mount.path("to");
    io(fs::write(&from, b"replacement contents"))?;
    io(fs::write(&to, b"old destination"))?;
    io(fs::rename(&from, &to))?;
    check(!io(fs::exists(&from))?, "SMB rename left the source name")?;
    check(io(fs::read(&to))? == b"replacement contents", "SMB rename did not replace destination")?;
    mount.finish()
}
fn smb_delete() -> CaseResult {
    let mount = SmbMount::rw()?;
    let path = mount.path("delete-me");
    io(fs::write(&path, b"delete contents"))?;
    check(smb_names(&mount.work)?.contains(&std::ffi::OsString::from("delete-me")), "SMB create missing before delete")?;
    io(fs::remove_file(&path))?;
    check(!io(fs::exists(&path))? && smb_names(&mount.work)?.is_empty(), "SMB delete left the name")?;
    mount.finish()
}
fn smb_dirs() -> CaseResult {
    let mount = SmbMount::rw()?;
    let dir = mount.path("directory");
    io(fs::create_dir(&dir))?;
    let child = dir.join("child");
    io(fs::write(&child, b"keep me"))?;
    let removed = fs::remove_dir(&dir);
    check(removed.is_err(), "SMB rmdir accepted a non-empty directory")?;
    check(io(fs::read(&child))? == b"keep me", "failed SMB rmdir changed the child")?;
    io(fs::remove_file(&child))?;
    io(fs::remove_dir(&dir))?;
    check(!io(fs::exists(&dir))?, "SMB rmdir left the empty directory")?;
    mount.finish()
}
fn smb_atomic_save() -> CaseResult {
    use libbreenix::crypto::sha256::{sha256, Sha256};
    let mount = SmbMount::rw()?;
    let path = mount.path("drawing");
    io(fs::write(&path, b"old drawing"))?;
    let bytes: Vec<u8> = (0..4 * 1024 * 1024).map(|n| ((n * 37 + n / 251) % 256) as u8).collect();
    let expected = sha256(&bytes);
    match libbreenix::atomic_file::save(&path, &bytes) {
        libbreenix::atomic_file::SaveResult::Saved => {},
        other => return Err(format!("SMB atomic save: {other:?}").into()),
    }
    let mut file = io(File::open(&path))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; ufs_abi::MAX_DATA + 17];
    let mut size = 0;
    loop {
        let n = io(file.read(&mut buffer))?;
        if n == 0 { break; }
        size += n;
        digest.update(&buffer[..n]);
    }
    drop(file);
    check(size == bytes.len() && digest.finalize() == expected, "SMB atomic save size or SHA-256 differs")?;
    check(smb_names(&mount.work)? == [std::ffi::OsString::from("drawing")], "SMB atomic save left a temporary")?;
    mount.finish()
}
