//! Snapshot properties on the launcher's empty writable scratch volume.
use breenix_suite::suite::{case_ms_left, check, value, CaseResult};
use libbreenix::{fs as sysfs, types::Fd};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn io<T>(result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn command(action: &str, name: Option<&str>, path: &Path) -> Result<String, String> {
    let mut command = Command::new("/usr/bin/breenfs");
    command.arg("snapshot").arg(action);
    if let Some(name) = name {
        command.arg(name);
    }
    let mut child = io(command
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn())?;
    let started = Instant::now();
    loop {
        if io(child.try_wait())?.is_some() {
            let output = io(child.wait_with_output())?;
            if !output.status.success() {
                return Err(format!(
                    "snapshot {action}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            return String::from_utf8(output.stdout).map_err(|error| error.to_string());
        }
        if started.elapsed() >= Duration::from_secs(8) || case_ms_left() < 1200 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("snapshot {action} watchdog expired"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn dead_remaining(path: &Path) -> Result<u64, String> {
    let text = command("status", None, path)?;
    let fields: Vec<_> = text
        .split_whitespace()
        .filter_map(|field| field.strip_prefix("dead_remaining="))
        .collect();
    if fields.len() != 1 {
        return Err(format!("invalid snapshot status: {text:?}"));
    }
    fields[0].parse::<u64>().map_err(|error| error.to_string())
}

struct Scratch {
    base: PathBuf,
    live: PathBuf,
    selected: PathBuf,
    source: String,
    directory: String,
    name: String,
    live_mounted: bool,
    selected_mounted: bool,
    root_created: bool,
    work_created: bool,
    cleanup_started: bool,
}

impl Scratch {
    fn mount() -> Result<Self, String> {
        let pid = std::process::id();
        let base = PathBuf::from(format!("/tmp/filesystem-snapshots-{pid}"));
        io(fs::create_dir(&base))?;
        let mut scratch = Self {
            live: base.join("live"),
            selected: base.join("selected"),
            base,
            source: String::new(),
            directory: format!("snapshots-{pid}"),
            name: format!("snapshot-{pid}"),
            live_mounted: false,
            selected_mounted: false,
            root_created: false,
            work_created: false,
            cleanup_started: false,
        };
        io(fs::create_dir(&scratch.live))?;
        io(fs::create_dir(&scratch.selected))?;
        let mut disks = Vec::new();
        for entry in io(fs::read_dir("/dev"))? {
            let name = io(entry)?.file_name();
            if let Some(disk) = name
                .to_str()
                .and_then(|s| s.strip_prefix("disk"))
                .and_then(|s| s.parse::<u8>().ok())
            {
                disks.push(disk);
            }
        }
        disks.sort_unstable();
        for disk in disks {
            scratch.source = format!("/dev/disk{disk}");
            // Read-only magic probes exclude the root and populated fixtures.
            if scratch.attach_live(1).is_err() {
                continue;
            }
            let empty = io(fs::read_dir(&scratch.live))?
                .next()
                .transpose()
                .map_err(|error| error.to_string())?
                .is_none();
            let geometry = stats(&scratch.live)?;
            scratch.detach_live()?;
            if !empty || geometry.f_blocks != 16384 || geometry.f_bsize != 4096 {
                continue;
            }
            scratch.attach_live(0)?;
            io(fs::create_dir(scratch.work()))?;
            scratch.work_created = true;
            return Ok(scratch);
        }
        Err("no empty 64 MiB BreenFS scratch disk found".into())
    }

    fn mount_at(&self, target: &Path, root: Option<&str>, flags: u64) -> Result<(), String> {
        let data = root
            .map(|name| CString::new(format!("root={name}")))
            .transpose()
            .map_err(|error| error.to_string())?;
        unsafe {
            sysfs::mount(
                &self.source,
                target.to_str().ok_or("mount path encoding")?,
                "breenfs",
                flags,
                data.as_ref()
                    .map_or(std::ptr::null(), |s| s.as_ptr().cast()),
            )
        }
        .map_err(|error| error.to_string())
    }

    fn attach_live(&mut self, flags: u64) -> Result<(), String> {
        self.mount_at(&self.live, None, flags)?;
        self.live_mounted = true;
        Ok(())
    }

    fn detach_live(&mut self) -> Result<(), String> {
        sysfs::umount2(self.live.to_str().ok_or("mount path encoding")?, 0)
            .map_err(|error| error.to_string())?;
        self.live_mounted = false;
        Ok(())
    }

    fn attach_selected(&mut self, flags: u64) -> Result<(), String> {
        self.mount_at(&self.selected, Some(&self.name), flags)?;
        self.selected_mounted = true;
        Ok(())
    }

    fn detach_selected(&mut self) -> Result<(), String> {
        sysfs::umount2(self.selected.to_str().ok_or("mount path encoding")?, 0)
            .map_err(|error| error.to_string())?;
        self.selected_mounted = false;
        Ok(())
    }

    fn work(&self) -> PathBuf {
        self.live.join(&self.directory)
    }
    fn selected_work(&self) -> PathBuf {
        self.selected.join(&self.directory)
    }

    fn create(&mut self, action: &str) -> Result<(), String> {
        command(action, Some(&self.name), &self.work())?;
        self.root_created = true;
        Ok(())
    }

    fn delete(&mut self) -> Result<(), String> {
        command("delete", Some(&self.name), &self.work())?;
        self.root_created = false;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        self.cleanup_started = true;
        let mut errors = Vec::new();
        if self.selected_mounted {
            if let Err(error) = self.detach_selected() {
                errors.push(error);
            }
        }
        // Namespace cleanup may fail too. Keep unmount outside this fallible
        // phase so an assertion or cleanup error cannot bypass detach.
        let namespace = (|| -> Result<(), String> {
            if self.root_created || self.work_created {
                if case_ms_left() < 1200 {
                    return Err("cleanup budget exhausted before namespace removal".into());
                }
                if !self.live_mounted {
                    self.attach_live(0)?;
                }
                if self.root_created {
                    self.delete()?;
                }
                command("wait", None, &self.live)?;
                if self.work_created {
                    io(fs::remove_dir_all(self.work()))?;
                    self.work_created = false;
                }
            }
            Ok(())
        })();
        if let Err(error) = namespace {
            errors.push(error);
        }
        if self.live_mounted {
            if let Err(error) = self.detach_live() {
                errors.push(error);
            }
        }
        for path in [&self.selected, &self.live, &self.base] {
            if path.exists() {
                if let Err(error) = io(fs::remove_dir(path)) {
                    errors.push(error);
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.cleanup_started {
            let _ = self.finish();
        }
    }
}

fn on_scratch(run: impl FnOnce(&mut Scratch) -> CaseResult) -> CaseResult {
    let mut scratch = Scratch::mount()?;
    let result = run(&mut scratch);
    let cleanup = scratch.finish();
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error:?}; cleanup: {cleanup}").into()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), cleanup) => {
            cleanup?;
            Ok(())
        }
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = io(File::create(path))?;
    io(file.write_all(bytes))?;
    io(file.sync_all())
}

fn stats(path: &Path) -> Result<sysfs::Statfs, String> {
    let file = io(File::open(path))?;
    sysfs::fstatfs(Fd::from_raw(file.as_raw_fd() as u64)).map_err(|error| error.to_string())
}

fn marker_commits(scratch: &Scratch, first_marker: u8) -> Result<u64, String> {
    // Coordinator E24 ledger 445: a changed marker plus fsync advances the
    // filesystem generation; snapshot wait alone does not drain quarantine.
    for byte in first_marker..3u8 {
        write_synced(&scratch.work().join("commit-marker"), &[byte])?;
    }
    Ok(stats(&scratch.work())?.f_bfree)
}

fn regain(scratch: &Scratch, before: u64, blocks: u64, first_marker: u8) -> CaseResult {
    let after = marker_commits(scratch, first_marker)?;
    value("free-before-delete", before as i64, "", None);
    value("free-after-delete", after as i64, "", None);
    value("file-blocks", blocks as i64, "", None);
    check(
        after >= before + blocks,
        &format!(
        "deletion did not return {blocks} blocks after three marker fsyncs: {before} -> {after}"),
    )
}

pub fn readonly_snapshot_frozen() -> CaseResult {
    on_scratch(|scratch| {
        let original = b"original snapshot bytes\n";
        write_synced(&scratch.work().join("data"), original)?;
        scratch.create("create")?;
        write_synced(&scratch.work().join("data"), b"changed live bytes\n")?;
        scratch.attach_selected(1)?;
        check(
            io(fs::read(scratch.selected_work().join("data")))? == original,
            "snapshot changed with live root",
        )?;
        let write = OpenOptions::new()
            .write(true)
            .open(scratch.selected_work().join("data"));
        check(
            write.err().and_then(|error| error.raw_os_error()) == Some(30),
            "snapshot write access did not return EROFS",
        )?;
        scratch.detach_selected()?;
        scratch.detach_live()?;
        // Remove the live writer first so EBUSY cannot mask the EROFS check.
        let data = CString::new(format!("root={}", scratch.name)).map_err(|e| e.to_string())?;
        let result = unsafe {
            sysfs::mount(
                &scratch.source,
                scratch.selected.to_str().ok_or("mount path encoding")?,
                "breenfs",
                0,
                data.as_ptr().cast(),
            )
        };
        if result.is_ok() {
            scratch.selected_mounted = true;
        }
        check(
            matches!(
                result,
                Err(libbreenix::error::Error::Os(
                    libbreenix::errno::Errno::EROFS
                ))
            ),
            "writable snapshot mount did not return EROFS",
        )?;
        scratch.attach_live(0)?;
        check(
            io(fs::read(scratch.work().join("data")))? == b"changed live bytes\n",
            "live change did not survive remount",
        )
    })
}

pub fn clone_diverges() -> CaseResult {
    on_scratch(|scratch| {
        let original = b"live bytes before clone\n";
        write_synced(&scratch.work().join("data"), original)?;
        scratch.create("clone")?;
        scratch.detach_live()?;
        scratch.attach_selected(0)?;
        check(
            io(fs::read(scratch.selected_work().join("data")))? == original,
            "clone did not inherit live bytes",
        )?;
        write_synced(
            &scratch.selected_work().join("data"),
            b"clone's private bytes\n",
        )?;
        scratch.detach_selected()?;
        scratch.attach_selected(1)?;
        check(
            io(fs::read(scratch.selected_work().join("data")))? == b"clone's private bytes\n",
            "clone change did not survive remount",
        )?;
        scratch.detach_selected()?;
        scratch.attach_live(0)?;
        check(
            io(fs::read(scratch.work().join("data")))? == original,
            "clone write changed the live root",
        )
    })
}

pub fn delete_returns_space() -> CaseResult {
    on_scratch(|scratch| {
        const BYTES: usize = 4 * 1024 * 1024;
        let path = scratch.work().join("data");
        write_synced(&path, &vec![0x51; BYTES])?;
        let metadata = io(fs::metadata(&path))?;
        check(
            metadata.blocks() == BYTES as u64 / 512,
            "4 MiB file was not fully allocated",
        )?;
        let blocks = metadata.blocks() / 8;
        scratch.create("create")?;
        write_synced(&path, &vec![0xa7; BYTES])?;
        // Coordinator E24 ledger 450: measure both sides after the same marker
        // routine, including its data allocation and COW quarantine.
        let before = marker_commits(scratch, 0)?;
        scratch.delete()?;
        command("wait", None, &scratch.work())?;
        check(
            dead_remaining(&scratch.work())? == 0,
            "snapshot wait left DEAD work",
        )?;
        regain(scratch, before, blocks, 0)?;
        check(
            io(fs::read(path))? == vec![0xa7; BYTES],
            "snapshot deletion changed live bytes",
        )
    })
}

pub fn delete_resumes_after_remount() -> CaseResult {
    on_scratch(|scratch| {
        let files = scratch.work().join("files");
        io(fs::create_dir(&files))?;
        // Long inline targets fill FS leaves without a data write per entry.
        // The production-VFS host check leaves 19 DEAD nodes after the first
        // resumed commit, beyond txn.rs::REAP_PER_COMMIT's 16-node budget.
        const LINKS: usize = 68;
        const BYTES: usize = 2 * 1024 * 1024;
        let target = "x".repeat(4095);
        for index in 0..LINKS {
            check(case_ms_left() >= 1500, "snapshot setup exhausted cleanup budget")?;
            std::os::unix::fs::symlink(&target, files.join(format!("link-{index:02}")))
                .map_err(|error| error.to_string())?;
            // Publish the metadata grants instead of retaining the whole
            // setup's reservations until its final fsync.
            if (index + 1) % 16 == 0 {
                io(io(File::open(&files))?.sync_all())?;
            }
        }
        write_synced(&files.join("data"), &vec![0x63; BYTES])?;
        scratch.create("create")?;
        io(fs::remove_dir_all(&files))?;
        io(io(File::open(scratch.work()))?.sync_all())?;
        let before = stats(&scratch.work())?.f_bfree;
        scratch.delete()?;
        let started = Instant::now();
        scratch.detach_live()?;
        let elapsed = started.elapsed();
        value(
            "delete-umount",
            elapsed.as_millis() as i64,
            "ms",
            Some((0, 1000)),
        );
        check(
            elapsed <= Duration::from_secs(1),
            "setup error: umount after delete exceeded 1 s",
        )?;
        scratch.attach_live(0)?;
        let remaining = dead_remaining(&scratch.work())?;
        value("dead-after-remount", remaining as i64, "", None);
        check(
            remaining > 0,
            "setup error: no DEAD work remained after remount",
        )?;
        // Start the recovered reaper without waiting for the 5 s timer.
        write_synced(&scratch.work().join("commit-marker"), &[0])?;
        command("wait", None, &scratch.work())?;
        check(
            dead_remaining(&scratch.work())? == 0,
            "resumed snapshot wait left DEAD work",
        )?;
        // One marker commit above leaves two of the ruling's three commits.
        regain(scratch, before, (BYTES / 4096) as u64, 1)?;
        check(
            !scratch.work().join("files").exists(),
            "deleted live directory returned after remount",
        )
    })
}
