//! Writable scratch-volume cases through POSIX file operations and mount(2).
use breenix_suite::suite::{check, value, wait_for, CaseResult};
use libbreenix::{fs as sysfs, memory, syscall::raw};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Barrier,
};
use std::time::{Duration, Instant};

#[cfg(target_arch = "x86_64")]
const MSYNC: u64 = 26;
#[cfg(target_arch = "aarch64")]
const MSYNC: u64 = 227;

const MOUNT: &str = "/mnt/breenfs-write";

fn io<T>(result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

struct ReadbackMount(bool);
impl Drop for ReadbackMount {
    fn drop(&mut self) {
        if self.0 {
            let _ = sysfs::umount2("/mnt/breenfs-write-durable", 0);
        }
    }
}

struct Scratch {
    source: String,
    mounted: bool,
    work: PathBuf,
}
impl Scratch {
    fn mount() -> Result<Self, String> {
        io(fs::create_dir_all(MOUNT))?;
        let mut scratch = Self {
            source: String::new(),
            mounted: false,
            work: PathBuf::from(MOUNT).join(format!("write-{}", std::process::id())),
        };
        let mut disks = Vec::new();
        for entry in io(fs::read_dir("/dev"))? {
            let name = io(entry)?.file_name();
            if let Some(number) = name
                .to_str()
                .and_then(|s| s.strip_prefix("disk"))
                .and_then(|s| s.parse::<u8>().ok())
            {
                disks.push(number);
            }
        }
        disks.sort_unstable();
        for disk in disks {
            scratch.source = format!("/dev/disk{disk}");
            // Disk nodes cannot be opened. mount(2) checks BREENFSS magic before
            // recovery; probe read-only so root and fixture disks stay untouched.
            if scratch.attach(1).is_err() {
                continue;
            }
            let empty = io(fs::read_dir(MOUNT))?
                .next()
                .transpose()
                .map_err(|e| e.to_string())?
                .is_none();
            scratch.detach()?;
            // The launcher supplies one empty scratch volume. The root and read
            // fixture contain files, so this probe excludes them from writable selection.
            if !empty {
                continue;
            }
            scratch.attach(0)?;
            io(fs::create_dir(&scratch.work))?;
            return Ok(scratch);
        }
        Err("no empty BreenFS scratch disk found by mount magic probe".into())
    }
    fn attach(&mut self, flags: u64) -> Result<(), String> {
        unsafe { sysfs::mount(&self.source, MOUNT, "breenfs", flags, std::ptr::null()) }
            .map_err(|error| error.to_string())?;
        self.mounted = true;
        Ok(())
    }
    fn detach(&mut self) -> Result<(), String> {
        sysfs::umount2(MOUNT, 0).map_err(|error| error.to_string())?;
        self.mounted = false;
        Ok(())
    }
    fn remount(&mut self) -> Result<(), String> {
        self.detach()?;
        self.attach(0)
    }
    fn committed_bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        let target = "/mnt/breenfs-write-durable";
        io(fs::create_dir_all(target))?;
        unsafe { sysfs::mount(&self.source, target, "breenfs", 1, std::ptr::null()) }
            .map_err(|error| error.to_string())?;
        let mut mount = ReadbackMount(true);
        let read = io(fs::read(
            Path::new(target)
                .join(self.work.file_name().ok_or("scratch work name")?)
                .join(name),
        ));
        // A separate read-only mount has no dirty VNodes from the writer.
        let unmount = sysfs::umount2(target, 0).map_err(|error| error.to_string());
        unmount?;
        mount.0 = false;
        read
    }
    fn finish(&mut self) -> Result<(), String> {
        if self.mounted {
            let cleanup = io(fs::remove_dir_all(&self.work));
            // Unmount even if cleanup failed, so a failed case releases its mount.
            let unmount = self.detach();
            unmount?;
            cleanup?;
        }
        Ok(())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if self.mounted {
            let _ = self.finish();
        }
    }
}
fn on_scratch(run: impl FnOnce(&mut Scratch) -> CaseResult) -> CaseResult {
    let mut scratch = Scratch::mount()?;
    let result = run(&mut scratch);
    let cleanup = scratch.finish();
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error:?}; unmount/cleanup: {cleanup}").into()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), cleanup) => {
            cleanup?;
            Ok(())
        }
    }
}

// This small model describes the tree we create, without learning expected
// bytes, modes or link identity from the filesystem under test.
#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    mode: u32,
    bytes: Vec<u8>,
}
fn tree(root: &Path) -> Result<BTreeMap<PathBuf, TreeEntry>, String> {
    let mut pending = vec![PathBuf::new()];
    let mut entries = BTreeMap::new();
    while let Some(relative) = pending.pop() {
        if entries.len() >= 16 {
            return Err("write tree walk did not terminate".into());
        }
        let path = root.join(&relative);
        let metadata = io(fs::symlink_metadata(&path))?;
        let bytes = if metadata.is_dir() {
            for entry in io(fs::read_dir(&path))? {
                pending.push(relative.join(io(entry)?.file_name()));
            }
            Vec::new()
        } else if metadata.file_type().is_symlink() {
            use std::os::unix::ffi::OsStrExt;
            io(fs::read_link(&path))?.as_os_str().as_bytes().to_vec()
        } else if metadata.is_file() {
            io(fs::read(&path))?
        } else {
            return Err("unexpected write tree type".into());
        };
        entries.insert(
            relative,
            TreeEntry {
                mode: metadata.mode(),
                bytes,
            },
        );
    }
    Ok(entries)
}
fn absent(path: &Path) -> CaseResult {
    let result = fs::symlink_metadata(path);
    check(
        result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(2),
        &format!("expected ENOENT at {path:?}, got {result:?}"),
    )
}

pub fn remount_reads_back() -> CaseResult {
    on_scratch(|scratch| {
        let root = &scratch.work;
        io(fs::create_dir(root.join("nested")))?;
        let data: Vec<u8> = (0..12345).map(|i| (i % 251) as u8).collect();
        let mut file = io(File::create(root.join("nested/data")))?;
        io(file.write_all(&data))?;
        io(fs::hard_link(root.join("nested/data"), root.join("alias")))?;
        io(symlink("nested/data", root.join("symbol")))?;
        io(fs::write(root.join("empty"), []))?;
        for (path, mode) in [
            ("", 0o750),
            ("nested", 0o710),
            ("nested/data", 0o640),
            ("empty", 0o600),
        ] {
            io(fs::set_permissions(
                root.join(path),
                fs::Permissions::from_mode(mode),
            ))?;
        }
        let expected: BTreeMap<_, _> = [
            ("", 0o040750, Vec::new()),
            ("nested", 0o040710, Vec::new()),
            ("nested/data", 0o100640, data.clone()),
            ("alias", 0o100640, data),
            ("symbol", 0o120777, b"nested/data".to_vec()),
            ("empty", 0o100600, Vec::new()),
        ]
        .into_iter()
        .map(|(p, mode, bytes)| (PathBuf::from(p), TreeEntry { mode, bytes }))
        .collect();
        io(file.sync_all())?;
        drop(file);
        scratch.remount()?;
        check(
            tree(&scratch.work)? == expected,
            "remounted tree differs from the expected model",
        )?;
        let original = io(fs::metadata(scratch.work.join("nested/data")))?;
        let alias = io(fs::metadata(scratch.work.join("alias")))?;
        check(
            original.ino() == alias.ino() && original.nlink() == 2 && alias.nlink() == 2,
            "hard-link identity or link count lost across remount",
        )
    })
}

pub fn rename_replace_atomic() -> CaseResult {
    on_scratch(|scratch| {
        let target = scratch.work.join("target");
        let source = scratch.work.join("source");
        let old = vec![b'A'; 8192];
        let new = vec![b'B'; 8192];
        io(fs::write(&target, &old))?;
        let mut held = io(File::open(&target))?;
        let stop = AtomicBool::new(false);
        let ready = Barrier::new(2);
        // POSIX rename replacement keeps the destination present. Check complete
        // old/new contents; the runner owns the hang watchdog.
        std::thread::scope(|scope| -> CaseResult {
            let reader = scope.spawn(|| -> Result<usize, String> {
                let first = io(fs::read(&target));
                ready.wait();
                let first = first?;
                if first != old {
                    return Err("initial rename contents differ".into());
                }
                let mut reads = 1;
                while !stop.load(Ordering::Acquire) {
                    let bytes = io(fs::read(&target))?;
                    if bytes != old && bytes != new {
                        return Err("rename observer read partial or mixed contents".into());
                    }
                    reads += 1;
                }
                Ok(reads)
            });
            ready.wait();
            let result = (|| -> CaseResult {
                for round in 0..16 {
                    io(fs::write(&source, if round % 2 == 0 { &new } else { &old }))?;
                    io(fs::rename(&source, &target))?;
                    absent(&source)?;
                    check(
                        io(fs::read(&target))? == if round % 2 == 0 { &new[..] } else { &old[..] },
                        "rename did not publish the replacement bytes",
                    )?;
                }
                Ok(())
            })();
            stop.store(true, Ordering::Release);
            let reads = reader.join().map_err(|_| "rename observer panicked")??;
            result?;
            check(reads > 0, "rename observer never ran")
        })?;
        let mut held_bytes = Vec::new();
        io(held.read_to_end(&mut held_bytes))?;
        check(held_bytes == old, "rename changed the replaced open file")?;
        drop(held);
        scratch.remount()?;
        check(
            io(fs::read(scratch.work.join("target")))? == old,
            "replacement was not recovered after remount",
        )?;
        absent(&scratch.work.join("source"))
    })
}

pub fn unlinked_open_file() -> CaseResult {
    on_scratch(|scratch| {
        let path = scratch.work.join("orphan");
        let data = b"open file remains readable after unlink\n";
        io(fs::write(&path, data))?;
        let mut held = io(OpenOptions::new().read(true).write(true).open(&path))?;
        io(fs::remove_file(&path))?;
        absent(&path)?;
        // Commit while the open descriptor pins the zero-link inode.
        io(held.sync_all())?;
        check(
            io(held.metadata())?.nlink() == 0,
            "unlinked open file has nonzero nlink",
        )?;
        let mut bytes = Vec::new();
        io(held.read_to_end(&mut bytes))?;
        check(bytes == data, "unlinked open file became unreadable")?;
        drop(held);
        scratch.remount()?;
        absent(&scratch.work.join("orphan"))
    })
}

pub fn truncate_sparse() -> CaseResult {
    on_scratch(|scratch| {
        let path = scratch.work.join("sparse");
        let mut file = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path))?;
        io(file.write_all(&[0x41; 4096]))?;
        io(file.seek(SeekFrom::Start(1024 * 1024 + 17)))?;
        io(file.write_all(b"tail"))?;
        io(file.set_len(1000))?;
        io(file.set_len(16384))?;
        io(file.seek(SeekFrom::Start(8192 + 9)))?;
        io(file.write_all(b"sparse"))?;
        let mut expected = vec![0; 16384];
        expected[..1000].fill(0x41);
        expected[8201..8207].copy_from_slice(b"sparse");
        io(file.sync_all())?;
        drop(file);
        scratch.remount()?;
        check(
            io(fs::read(scratch.work.join("sparse")))? == expected,
            "truncate regrowth exposed discarded data or sparse holes differ",
        )?;
        let metadata = io(fs::metadata(scratch.work.join("sparse")))?;
        check(
            metadata.size() == 16384 && metadata.blocks() == 16,
            &format!(
                "sparse size/blocks differ: size={} blocks={}",
                metadata.size(),
                metadata.blocks()
            ),
        )
    })
}

struct Mapping(*mut u8);
impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = memory::munmap(self.0, 8192);
    }
}
pub fn mmap_msync() -> CaseResult {
    on_scratch(|scratch| {
        let file = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(scratch.work.join("mapped")))?;
        io(file.set_len(8192))?;
        io(file.sync_all())?;
        let before = durable_generation()?;
        let mapping = Mapping(memory::mmap(
            std::ptr::null_mut(),
            8192,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )?);
        let expected: Vec<u8> = (0..8192).map(|i| (i % 239) as u8).collect();
        unsafe {
            std::ptr::copy_nonoverlapping(expected.as_ptr(), mapping.0, expected.len());
        }
        let result = unsafe { raw::syscall3(MSYNC, mapping.0 as u64, 8192, 4) } as i64;
        check(result == 0, &format!("msync(MS_SYNC) returned {result}"))?;
        check(
            durable_generation()? > before,
            "MS_SYNC did not advance the durable generation",
        )?;
        check(
            scratch.committed_bytes("mapped")? == expected,
            "MS_SYNC did not publish the mapped bytes on disk",
        )?;
        drop(mapping);
        drop(file);
        scratch.remount()?;
        check(
            io(fs::read(scratch.work.join("mapped")))? == expected,
            "MS_SYNC stores did not survive remount",
        )
    })
}

fn durable_generation() -> Result<u64, String> {
    // vfs::boot::report labels a non-root storage mount "home" and omits
    // bind entries. Verify our writable mount is present, then require just
    // one home/breenfs counter row, refusing ambiguous storage identities.
    let mounts = io(fs::read_to_string("/proc/mounts"))?;
    let scratch: Vec<Vec<_>> = mounts
        .lines()
        .map(|line| line.split_whitespace().collect())
        .filter(|row: &Vec<&str>| row.len() == 6 && row[1] == MOUNT)
        .collect();
    if scratch.len() != 1
        || scratch[0][2] != "breenfs"
        || !scratch[0][3].split(',').any(|flag| flag == "rw")
    {
        return Err("scratch writable BreenFS mount is missing or duplicated".into());
    }
    let text = io(fs::read_to_string("/proc/breenix/filesystems"))?;
    let rows: Vec<_> = text
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some("home") && fields.next() == Some("breenfs")
        })
        .collect();
    if rows.len() != 1 {
        return Err("scratch filesystem state row missing or duplicated".into());
    }
    let fields: BTreeMap<_, _> = rows[0]
        .split_whitespace()
        .skip(2)
        .filter_map(|field| field.split_once('='))
        .collect();
    if fields.get("read_only") != Some(&"0") {
        return Err("scratch mount latched read-only".into());
    }
    fields
        .get("durable_generation")
        .ok_or("missing durable_generation")?
        .parse()
        .map_err(|e| format!("invalid durable_generation: {e}"))
}

pub fn commit_without_fsync() -> CaseResult {
    on_scratch(|scratch| {
        // Establish a clean transaction before recording the write's predecessor.
        let mut file = io(File::create(scratch.work.join("periodic")))?;
        io(file.sync_all())?;
        let before = durable_generation()?;
        let started = Instant::now();
        io(file.write_all(b"periodic commit without fsync\n"))?;
        // One 5 s commit interval plus 2 s scheduling/I/O margin. This guards
        // a hung commit worker; no assertion judges the measured latency.
        let deadline = started + Duration::from_secs(7);
        wait_for(7000, "commit-wait");
        let durable = loop {
            let durable = durable_generation()?;
            if durable > before {
                break durable;
            }
            if Instant::now() >= deadline {
                return Err("periodic commit watchdog expired".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        value(
            "commit-wait",
            started.elapsed().as_millis() as i64,
            "ms",
            None,
        );
        value("durable-generation", durable as i64, "", None);
        check(
            scratch.committed_bytes("periodic")? == b"periodic commit without fsync\n",
            "periodic receipt did not cover the bytes on disk",
        )?;
        // Only after the receipt is observed may close/unmount force a commit.
        drop(file);
        scratch.remount()?;
        check(
            io(fs::read(scratch.work.join("periodic")))? == b"periodic commit without fsync\n",
            "periodically committed bytes lost across remount",
        )
    })
}
