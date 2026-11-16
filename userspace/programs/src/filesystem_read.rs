//! Read a host-built breenfs fixture through the public filesystem interfaces.
use breenix_suite::suite::{check, value, CaseResult};
use libbreenix::crypto::sha256::{sha256, Sha256};
use libbreenix::{fs as sysfs, memory, types::Fd};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};

const MANIFEST: &str = "/usr/local/filesystem-suite/read/manifest";
const TAIL_OFFSET: usize = 4 * 1024 * 1024;
const FILE_SIZE: usize = TAIL_OFFSET + 23;
const MESSAGE: &[u8] = b"breenfs read fixture\n";

fn io<T>(result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

struct Fixture(PathBuf, bool, u8);
impl Fixture {
    fn mount() -> Result<Self, String> {
        let dir = PathBuf::from(format!("/tmp/filesystem-read-{}", std::process::id()));
        io(fs::create_dir(&dir))?;
        let result = Self::find(&dir);
        match result {
            Ok(disk) => Ok(Self(dir, true, disk)),
            Err(error) => {
                // find unmounts an unrelated breenfs volume before continuing.
                let _ = fs::remove_dir(&dir);
                Err(error)
            }
        }
    }

    fn find(dir: &Path) -> Result<u8, String> {
        let target = dir.to_str().ok_or("mount path encoding")?;
        let mut disks = Vec::new();
        for entry in io(fs::read_dir("/dev"))? {
            let entry = io(entry)?;
            let name = entry.file_name();
            if let Some(number) = name
                .to_str()
                .and_then(|s| s.strip_prefix("disk"))
                .and_then(|s| s.parse::<u8>().ok())
            {
                disks.push(number);
            }
        }
        disks.sort_unstable();
        for number in disks {
            // No mount data; the kernel resolves the disk node without opening it.
            if unsafe {
                sysfs::mount(
                    &format!("/dev/disk{number}"),
                    target,
                    "breenfs",
                    1,
                    std::ptr::null(),
                )
            }
            .is_err()
            {
                continue;
            }
            if fs::read(dir.join("nested/deeper/message")).is_ok_and(|bytes| bytes == MESSAGE) {
                return Ok(number);
            }
            sysfs::umount2(target, 0).map_err(|error| error.to_string())?;
        }
        Err("no read-only breenfs read fixture found on /dev/disk<N>".into())
    }

    fn finish(mut self) -> CaseResult {
        // Drop retries cleanup on failure; a failed unmount still fails the case.
        sysfs::umount2(self.0.to_str().ok_or("mount path encoding")?, 0)?;
        self.1 = false;
        io(fs::remove_dir(&self.0))?;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.1 && sysfs::umount2(self.0.to_str().unwrap_or(""), 0).is_err() {
            return;
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

struct Row {
    path: Vec<u8>,
    metadata: Metadata,
    digest: String,
}

fn guest_manifest(root: &Path) -> Result<String, String> {
    let mut todo = vec![(root.to_path_buf(), b"/".to_vec())];
    let mut rows = Vec::new();
    let mut directories = BTreeSet::new();
    while let Some((path, relative)) = todo.pop() {
        if rows.len() > 5020 {
            return Err("fixture walk did not terminate".into());
        }
        let metadata = io(fs::symlink_metadata(&path))?;
        let digest = if metadata.is_dir() {
            if !directories.insert((metadata.dev(), metadata.ino())) {
                return Err("directory cycle or alias".into());
            }
            for child in io(fs::read_dir(&path))? {
                let child = io(child)?;
                let mut name = relative.clone();
                if name != b"/" {
                    name.push(b'/');
                }
                name.extend_from_slice(child.file_name().as_bytes());
                todo.push((child.path(), name));
            }
            "-".into()
        } else if metadata.is_file() {
            let mut file = io(File::open(&path))?;
            let mut hash = Sha256::new();
            let mut bytes = [0; 16384];
            let mut size = 0;
            loop {
                let n = io(file.read(&mut bytes))?;
                if n == 0 {
                    break;
                }
                hash.update(&bytes[..n]);
                size += n as u64;
            }
            if size != metadata.size() {
                return Err("file read size differs from stat".into());
            }
            hex(&hash.finalize())
        } else if metadata.file_type().is_symlink() {
            hex(&sha256(io(fs::read_link(&path))?.as_os_str().as_bytes()))
        } else {
            return Err("unexpected fixture file type".into());
        };
        rows.push(Row {
            path: relative,
            metadata,
            digest,
        });
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups = BTreeMap::new();
    for row in &rows {
        if !row.metadata.is_dir() {
            let group = groups
                .entry((row.metadata.dev(), row.metadata.ino()))
                .or_insert((0, hex(&row.path)));
            group.0 += 1;
        }
    }
    use std::fmt::Write;
    let mut out = String::new();
    for row in rows {
        let m = row.metadata;
        let kind = if m.is_dir() {
            "dir"
        } else if m.is_file() {
            "reg"
        } else {
            "symlink"
        };
        let group = groups
            .get(&(m.dev(), m.ino()))
            .filter(|g| g.0 > 1)
            .map(|g| g.1.as_str())
            .unwrap_or("-");
        writeln!(
            out,
            "{}\t{}\t{:06o}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            hex(&row.path),
            kind,
            m.mode(),
            m.uid(),
            m.gid(),
            m.size(),
            m.blocks(),
            m.mtime(),
            m.mtime_nsec(),
            row.digest,
            group
        )
        .unwrap();
    }
    Ok(out)
}

pub fn manifest_equal() -> CaseResult {
    let fixture = Fixture::mount()?;
    let expected = io(fs::read_to_string(MANIFEST))?;
    let actual = guest_manifest(&fixture.0)?;
    if actual != expected {
        let different = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b);
        return Err(format!(
            "fixture manifest differs at row {different:?}; guest={} host={}",
            actual.lines().count(),
            expected.lines().count()
        )
        .into());
    }
    fixture.finish()
}

pub fn holes_and_tail() -> CaseResult {
    let fixture = Fixture::mount()?;
    let mut file = io(File::open(fixture.0.join("sparse")))?;
    let metadata = io(file.metadata())?;
    check(
        metadata.size() == FILE_SIZE as u64,
        "sparse file size differs",
    )?;
    check(
        metadata.blocks() == 16,
        "st_blocks must count two mapped 4 KiB blocks in 512-byte units",
    )?;
    let mut bytes = Vec::new();
    io(file.read_to_end(&mut bytes))?;
    check(
        bytes.len() == FILE_SIZE
            && bytes[..4096].iter().all(|&b| b == b'A')
            && bytes[4096..TAIL_OFFSET].iter().all(|&b| b == 0)
            && bytes[TAIL_OFFSET..].iter().all(|&b| b == b'Z'),
        "sparse data or hole differs",
    )?;
    io(file.seek(SeekFrom::Start(FILE_SIZE as u64)))?;
    check(
        io(file.read(&mut [0xa5; 1]))? == 0,
        "read past EOF returned data",
    )?;
    let page = memory::mmap(
        std::ptr::null_mut(),
        4096,
        memory::PROT_READ,
        memory::MAP_PRIVATE,
        file.as_raw_fd(),
        TAIL_OFFSET as i64,
    )?;
    // POSIX: the remainder of a mapped last partial page is zero-filled.
    let tail = unsafe { std::slice::from_raw_parts(page, 4096) };
    let correct = tail[..23].iter().all(|&b| b == b'Z') && tail[23..].iter().all(|&b| b == 0);
    memory::munmap(page, 4096)?;
    check(
        correct,
        "mapped partial page has incorrect data or nonzero EOF tail",
    )?;
    drop(file);
    fixture.finish()
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: Vec<u8>,
    ino: u64,
    cookie: u64,
}

fn batch(file: &File, size: usize) -> Result<Vec<Entry>, String> {
    let mut buffer = vec![0; size];
    let n = sysfs::getdents64(Fd::from_raw(file.as_raw_fd() as u64), &mut buffer)
        .map_err(|error| error.to_string())?;
    if n > size {
        return Err("oversized getdents result".into());
    }
    let mut at = 0;
    let mut entries = Vec::new();
    while at < n {
        if n - at < 24 {
            return Err("truncated dirent".into());
        }
        let record = &buffer[at..n];
        let len = u16::from_ne_bytes(record[16..18].try_into().unwrap()) as usize;
        if len < 24 || len % 8 != 0 || len > record.len() {
            return Err("invalid dirent length".into());
        }
        let end = record[19..len]
            .iter()
            .position(|&b| b == 0)
            .ok_or("dirent lacks NUL")?
            + 19;
        let cookie = i64::from_ne_bytes(record[8..16].try_into().unwrap());
        if cookie < 0 {
            return Err("negative directory cookie".into());
        }
        entries.push(Entry {
            name: record[19..end].to_vec(),
            ino: u64::from_ne_bytes(record[..8].try_into().unwrap()),
            cookie: cookie as u64,
        });
        at += len;
    }
    Ok(entries)
}

fn remainder(file: &File, size: usize) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    loop {
        let batch = batch(file, size)?;
        if batch.is_empty() {
            return Ok(entries);
        }
        entries.extend(batch);
        if entries.len() > 5002 {
            return Err("directory stream did not terminate".into());
        }
    }
}

pub fn readdir_resume() -> CaseResult {
    let fixture = Fixture::mount()?;
    let file = io(File::open(fixture.0.join("many")))?;
    let entries = remainder(&file, 128)?;
    let names: BTreeSet<_> = entries
        .iter()
        .filter(|e| e.name != b"." && e.name != b"..")
        .map(|e| e.name.clone())
        .collect();
    let expected: BTreeSet<_> = (0..5000).map(|i| format!("n{i:04}").into_bytes()).collect();
    check(
        names == expected
            && entries.len() == 5002
            && entries.iter().filter(|e| e.name == b".").count() == 1
            && entries.iter().filter(|e| e.name == b"..").count() == 1,
        "directory names missing or duplicated",
    )?;
    check(
        entries.iter().all(|e| e.ino != 0),
        "directory returned zero inode",
    )?;
    let fd = Fd::from_raw(file.as_raw_fd() as u64);
    for boundary in [1, 2501, 5001] {
        let cookie = entries[boundary - 1].cookie;
        check(
            sysfs::lseek(fd, cookie as i64, sysfs::SEEK_SET)? == cookie,
            "directory seek changed cookie",
        )?;
        let replay = remainder(&file, 64)?;
        check(
            replay == entries[boundary..],
            "cookie seek did not replay the exact suffix",
        )?;
    }
    drop(file);
    fixture.finish()
}

fn erofs<T: std::fmt::Debug>(result: std::io::Result<T>, operation: &str) -> CaseResult {
    check(
        result.as_ref().err().and_then(|error| error.raw_os_error()) == Some(30),
        &format!("{operation} expected EROFS: {result:?}"),
    )
}

pub fn read_only_refusals() -> CaseResult {
    let root = crate::filesystem_root::declared()?;
    let fixture = Fixture::mount()?;
    let mounts = io(fs::read_to_string("/proc/mounts"))?;
    let rows: Vec<Vec<&str>> = mounts
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    let roots: Vec<_> = rows.iter().filter(|row| row.len() == 6 && row[1] == "/").collect();
    let root_declared = roots.len() == 1 && roots[0][2] == root.name();
    let root_distinct = io(fs::metadata("/"))?.dev() != io(fs::metadata(&fixture.0))?.dev();
    let fixture_ro = rows.iter().any(|row| {
        row.len() == 6
            && row[1] == fixture.0.to_str().unwrap_or("")
            && row[2] == "breenfs"
            && row[3].split(',').any(|option| option == "ro")
    });
    check(root_declared, "root fstype differs from the host declaration")?;
    check(root_distinct, "fixture and root have the same st_dev")?;
    check(fixture_ro, "fixture is not breenfs ro in /proc/mounts")?;
    value("fixture-disk", i64::from(fixture.2), "", None);
    value("root-declared", i64::from(root_declared), "", Some((1, 1)));
    value("root-distinct", i64::from(root_distinct), "", Some((1, 1)));
    value("fixture-breenfs-ro", i64::from(fixture_ro), "", Some((1, 1)));
    let path = fixture.0.join("linked-a");
    let before = io(fs::read(&path))?;
    check(
        before == b"one inode, two names\n",
        "refusal source is not readable",
    )?;
    // A read-only mount refuses write access at open; no writable descriptor can exist.
    erofs(OpenOptions::new().write(true).open(&path), "write open")?;
    erofs(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(fixture.0.join("new")),
        "create",
    )?;
    erofs(fs::remove_file(&path), "unlink")?;
    check(
        io(fs::read(&path))? == before && !fixture.0.join("new").exists(),
        "refusal changed the fixture",
    )?;
    fixture.finish()
}
