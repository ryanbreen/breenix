//! Report host filesystem format evidence staged on the disk.
use breenix_suite::suite::{case, category, check, fail, running_case, suite, CaseResult, Suite};
use libbreenix::{fs as sysfs, io as sysio, types::Fd};
use std::fs::{self, File};
use std::io::Read;
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
    category("format", "Formatter images pass the independent checker and violating images are refused", &[
        case("mkfs-empty-clean", "Empty formatted images pass the full checker", host_case),
        case("mkfs-populate-clean", "Populated formatted images pass the full checker", host_case),
        case("reproducible-image", "Fixed inputs produce byte-identical images", host_case),
        case("violations-refused", "Images violating format invariants are refused", host_case),
    ]),
]);

fn io<T>(result: std::io::Result<T>) -> Result<T, String> { result.map_err(|e| e.to_string()) }

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

fn host_case() -> CaseResult {
    let id = running_case().ok_or("the case could not tell which case it is")?;
    let path = format!("/usr/local/filesystem-suite/host/{id}");
    let text = std::fs::read_to_string(&path).map_err(|_| format!("the host harness left no record {path}"))?;
    match text.lines().next().unwrap_or("") {
        "PASS" => Ok(()),
        line if line.starts_with("FAIL ") => fail(format!("host: {}", &line[5..])),
        line => fail(format!("{path} reads {line:?}, not PASS or FAIL")),
    }
}

fn main() { SUITE.run() }
