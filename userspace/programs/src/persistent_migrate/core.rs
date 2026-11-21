//! Migration and read-back verification, independent of the mount syscalls.
#[path = "../../../../libs/libbreenix/src/migration_manifest.rs"]
mod manifest;

use crate::{
    layout,
    sha256::{sha256, Sha256},
};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::path::{Path, PathBuf};

const STORE: &str = ".breenix-shared";
const GENERATIONS: &str = ".breenix-generations";
const SYSTEM_ENTRIES: [&str; 2] = [STORE, "root"];
const BLOCK: usize = 4096;

pub struct Paths<'a> {
    pub source_system: &'a Path,
    pub source_home: &'a Path,
    pub system: &'a Path,
    pub home: &'a Path,
    pub evidence: &'a Path,
}

pub trait Mounts {
    fn snapshot(&mut self, name: &str) -> io::Result<()>;
    fn switch_system(&mut self, root: Option<&str>, read_only: bool) -> io::Result<PathBuf>;
    fn home_read_only(&mut self) -> io::Result<()>;
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn directory(path: &Path) -> io::Result<Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(invalid(format!("{} is not a directory", path.display())));
    }
    Ok(meta)
}

fn record(path: &Path, expected: &[u8]) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() != expected.len() as u64 || fs::read(path)? != expected {
        return Err(invalid(format!("invalid layout record {}", path.display())));
    }
    Ok(())
}

fn generation(source: &Path, id: &str) -> io::Result<PathBuf> {
    let store = source.join(STORE);
    directory(source)?;
    directory(&store)?;
    record(&store.join(".layout-v2"), b"BREENIX-SHARED 2\n")?;
    for path in layout::SHARED {
        directory(&store.join(path))?;
    }
    directory(&source.join("root"))?;
    let generations = source.join(GENERATIONS);
    directory(&generations)?;
    let generation = generations.join(id);
    directory(&generation)?;
    record(&generation.join(".complete"), layout::record(id).as_bytes())?;
    for path in [
        "bin",
        "sbin",
        "root",
        "home",
        "dev",
        "proc",
        "sys",
        "usr",
        "usr/share",
        "usr/share/factory",
        "usr/share/factory/etc",
    ]
    .into_iter()
    .chain(layout::SHARED)
    {
        directory(&generation.join(path))?;
    }
    Ok(generation)
}

fn empty_directory(path: &Path) -> io::Result<()> {
    directory(path)?;
    if fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(invalid(format!(
            "destination {} is not empty",
            path.display()
        )));
    }
    Ok(())
}

#[repr(C)]
struct Timespec {
    sec: i64,
    #[cfg(not(target_vendor = "apple"))]
    nsec: i64,
    #[cfg(target_vendor = "apple")]
    nsec: std::ffi::c_long,
}

// Both Breenix architectures use the same POSIX time and node interfaces.
#[cfg(target_vendor = "apple")]
type Mode = u16;
#[cfg(not(target_vendor = "apple"))]
type Mode = u32;
#[cfg(target_vendor = "apple")]
type Dev = i32;
#[cfg(not(target_vendor = "apple"))]
type Dev = u64;

extern "C" {
    fn utimensat(fd: i32, path: *const u8, times: *const Timespec, flags: i32) -> i32;
    fn mknod(path: *const u8, mode: Mode, device: Dev) -> i32;
    fn mkfifo(path: *const u8, mode: Mode) -> i32;
}

fn status(result: i32) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid("NUL in pathname"))
}

fn restore_metadata(path: &Path, meta: &Metadata) -> io::Result<()> {
    const AT_FDCWD: i32 = if cfg!(target_vendor = "apple") {
        -2
    } else {
        -100
    };
    const AT_SYMLINK_NOFOLLOW: i32 = if cfg!(target_vendor = "apple") {
        0x20
    } else {
        0x100
    };
    std::os::unix::fs::lchown(path, Some(meta.uid()), Some(meta.gid()))?;
    if !meta.is_symlink() {
        // Writing data and changing ownership can clear set-ID bits.
        fs::set_permissions(path, fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    }
    let times = [
        Timespec {
            sec: meta.atime(),
            nsec: meta.atime_nsec() as _,
        },
        Timespec {
            sec: meta.mtime(),
            nsec: meta.mtime_nsec() as _,
        },
    ];
    let path = c_path(path)?;
    status(unsafe {
        utimensat(
            AT_FDCWD,
            path.as_ptr().cast(),
            times.as_ptr(),
            AT_SYMLINK_NOFOLLOW,
        )
    })
}

fn create_special(path: &Path, meta: &Metadata) -> io::Result<()> {
    let path = c_path(path)?;
    let mode = ((meta.mode() & 0o170000) | 0o600) as Mode;
    let result = unsafe {
        if manifest::kind(meta.mode())? == "fifo" {
            mkfifo(path.as_ptr().cast(), 0o600 as Mode)
        } else {
            mknod(path.as_ptr().cast(), mode, meta.rdev() as Dev)
        }
    };
    status(result)
}

#[derive(Default)]
struct Copier {
    links: HashMap<(u64, u64), PathBuf>,
}

impl Copier {
    fn tree(source: &Path, destination: &Path, entries: Option<&[&str]>) -> io::Result<()> {
        let meta = directory(source)?;
        let mut copier = Self::default();
        if let Some(entries) = entries {
            for name in entries {
                copier.entry(&source.join(name), &destination.join(name), meta.dev())?;
            }
        } else {
            copier.children(source, destination, meta.dev())?;
        }
        restore_metadata(destination, &meta)
    }

    fn children(&mut self, source: &Path, destination: &Path, device: u64) -> io::Result<()> {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            self.entry(&entry.path(), &destination.join(entry.file_name()), device)?;
        }
        Ok(())
    }

    fn entry(&mut self, source: &Path, destination: &Path, device: u64) -> io::Result<()> {
        self.copy(source, destination, device).map_err(|error| {
            io::Error::new(error.kind(), format!("copy {}: {error}", source.display()))
        })
    }

    fn copy(&mut self, source: &Path, destination: &Path, device: u64) -> io::Result<()> {
        let meta = fs::symlink_metadata(source)?;
        if meta.dev() != device {
            return Err(invalid("mount inside source tree"));
        }
        if meta.is_dir() {
            fs::DirBuilder::new().mode(0o700).create(destination)?;
            self.children(source, destination, device)?;
            return restore_metadata(destination, &meta);
        }
        let identity = (device, meta.ino());
        if let Some(first) = self.links.get(&identity) {
            return fs::hard_link(first, destination);
        }
        match manifest::kind(meta.mode())? {
            "reg" => copy_file(source, destination, meta.len())?,
            "symlink" => std::os::unix::fs::symlink(fs::read_link(source)?, destination)?,
            _ => create_special(destination, &meta)?,
        }
        restore_metadata(destination, &meta)?;
        if meta.nlink() > 1 {
            self.links.insert(identity, destination.to_path_buf());
        }
        Ok(())
    }
}

fn copy_file(source: &Path, destination: &Path, size: u64) -> io::Result<()> {
    let mut source = File::open(source)?;
    let mut dest = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    let mut start = 0;
    // Set the final length to preserve trailing holes.
    let mut buffer = vec![0; 64 * 1024];
    let mut position = 0;
    while start < size {
        let len = (size - start).min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..len])?;
        for (index, block) in buffer[..len].chunks(BLOCK).enumerate() {
            if block.iter().any(|&byte| byte != 0) {
                let offset = start + (index * BLOCK) as u64;
                if position != offset {
                    dest.seek(SeekFrom::Start(offset))?;
                }
                dest.write_all(block)?;
                position = offset + block.len() as u64;
            }
        }
        start += len as u64;
    }
    dest.set_len(size)
}

fn digest(path: &Path, meta: &Metadata) -> io::Result<String> {
    if meta.is_symlink() {
        return Ok(manifest::hex(&sha256(
            fs::read_link(path)?.as_os_str().as_bytes(),
        )));
    }
    if !meta.is_file() {
        return Ok("-".into());
    }
    let mut file = File::open(path)?;
    let mut buffer = vec![0; 64 * 1024];
    let mut hash = Sha256::new();
    let mut size = 0;
    loop {
        let len = file.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
        size += len as u64;
    }
    if size != meta.len() {
        return Err(invalid(format!(
            "size changed while hashing {}",
            path.display()
        )));
    }
    Ok(manifest::hex(&hash.finalize()))
}

fn collect(
    rows: &mut Vec<manifest::Row>,
    root: &Path,
    prefix: &[u8],
    entries: Option<&[&str]>,
) -> io::Result<()> {
    let device = directory(root)?.dev();
    let mut pending = vec![(root.to_path_buf(), prefix.to_vec())];
    while let Some((path, logical)) = pending.pop() {
        let meta = fs::symlink_metadata(&path)?;
        if meta.dev() != device {
            return Err(invalid(format!(
                "mount inside manifest tree at {}",
                path.display()
            )));
        }
        if meta.is_dir() {
            let mut child = |name: &std::ffi::OsStr| {
                let mut next = logical.clone();
                if next != b"/" {
                    next.push(b'/');
                }
                next.extend_from_slice(name.as_bytes());
                pending.push((path.join(name), next));
            };
            if let Some(entries) = entries.filter(|_| path == root) {
                entries.iter().for_each(|name| child(name.as_ref()));
            } else {
                for entry in fs::read_dir(&path)? {
                    child(&entry?.file_name());
                }
            }
        }
        let rdev = device_parts(meta.rdev());
        rows.push(manifest::Row {
            path: logical,
            identity: (device, meta.ino()),
            mode: meta.mode(),
            uid: meta.uid(),
            gid: meta.gid(),
            rdev,
            size: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec() as u32),
            digest: digest(&path, &meta)?,
        });
    }
    Ok(())
}

fn device_parts(raw: u64) -> (u64, u64) {
    if cfg!(target_vendor = "apple") {
        ((raw >> 24) & 0xff, raw & 0xff_ffff)
    } else {
        manifest::device_parts(raw)
    }
}

pub fn run(
    paths: &Paths<'_>,
    id: &str,
    mounts: &mut impl Mounts,
    mut out: impl Write,
) -> io::Result<()> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(invalid("invalid userland generation"));
    }
    let source_generation = generation(paths.source_system, id).map_err(|error| {
        invalid(format!(
            "source requires the users layout for {id}: {error}"
        ))
    })?;
    directory(paths.source_home)?;
    empty_directory(paths.system)?;
    empty_directory(paths.home)?;
    // Reserve a private evidence directory before changing either volume.
    fs::DirBuilder::new().mode(0o700).create(paths.evidence)?;

    let name = format!("gen-{id}");
    mounts.snapshot(&name)?;
    Copier::tree(paths.source_system, paths.system, Some(&SYSTEM_ENTRIES))?;
    let gen = mounts.switch_system(Some(&name), false)?;
    empty_directory(&gen)?;
    Copier::tree(&source_generation, &gen, None)?;
    Copier::tree(paths.source_home, paths.home, None)?;

    // Unmount flushes each writer before comparison opens its read-only views.
    let default = mounts.switch_system(None, true)?;
    mounts.home_read_only()?;
    let prefix = format!("/{name}");
    let mut source = Vec::new();
    collect(
        &mut source,
        paths.source_system,
        b"/",
        Some(&SYSTEM_ENTRIES),
    )?;
    collect(&mut source, &source_generation, prefix.as_bytes(), None)?;
    collect(&mut source, paths.source_home, b"/home", None)?;
    let mut destination = Vec::new();
    collect(&mut destination, &default, b"/", None)?;
    collect(&mut destination, paths.home, b"/home", None)?;
    let gen = mounts.switch_system(Some(&name), true)?;
    collect(&mut destination, &gen, prefix.as_bytes(), None)?;

    let normalized_links = source
        .iter()
        .filter(|row| manifest::permissions(row.mode) != row.mode & 0o7777)
        .count();
    let source_path = paths.evidence.join("source.tsv");
    let destination_path = paths.evidence.join("destination.tsv");
    save(&source_path, source)?;
    save(&destination_path, destination)?;
    compare(&source_path, &destination_path)?;
    writeln!(out, "[migrate] normalized symlink modes: {normalized_links}")?;
    writeln!(out, "[migrate] manifests equal")
}

#[cfg(all(target_arch = "x86_64", target_os = "breenix"))]
pub fn describe_receipt(evidence: &Path, mut out: impl Write) -> io::Result<()> {
    let source = evidence.join("source.tsv");
    let destination = evidence.join("destination.tsv");
    writeln!(out, "[migrate] manifest sha256 source={} destination={}",
        digest(&source, &fs::metadata(&source)?)?,
        digest(&destination, &fs::metadata(&destination)?)?)
}

fn save(path: &Path, rows: Vec<manifest::Row>) -> io::Result<()> {
    let mut out = io::BufWriter::new(File::create(path)?);
    manifest::write(rows, &mut out)?;
    out.flush()
}

fn compare(source: &Path, destination: &Path) -> io::Result<()> {
    use io::BufRead;
    let mut source = io::BufReader::new(File::open(source)?);
    let mut destination = io::BufReader::new(File::open(destination)?);
    let mut left = Vec::new();
    let mut right = Vec::new();
    for row in 1.. {
        left.clear();
        right.clear();
        source.read_until(b'\n', &mut left)?;
        destination.read_until(b'\n', &mut right)?;
        if left != right {
            return Err(invalid(format!("migration manifests differ at row {row}")));
        }
        if left.is_empty() {
            return Ok(());
        }
    }
    unreachable!()
}
