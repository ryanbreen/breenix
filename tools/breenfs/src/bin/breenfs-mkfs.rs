//! Initial-image writer; on-disk layouts and tree edits come from the breenfs core.
use breenfs::block::{BlockDevice, BlockError};
use breenfs::commit::{Committer, DeviceStore};
use breenfs::format::*;
use breenfs::ops::{Files, SetAttr};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "../sparse.rs"]
mod sparse;

type Result<T> = std::result::Result<T, String>;
const USAGE: &str = "breenfs-mkfs [--populate DIR] [--uuid HEX] [--time SECS] [--hash-key HEX] [--label L] [--extents] --size BYTES IMAGE";

struct Args {
    image: PathBuf,
    size: u64,
    populate: Option<PathBuf>,
    uuid: [u8; 16],
    hash_key: [u8; 16],
    time: Option<Time>,
    label: [u8; 64],
    extents: bool,
}
fn hex(s: &str) -> Result<[u8; 16]> {
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("HEX must contain exactly 32 hexadecimal digits".into());
    }
    let mut out = [0; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
fn random_bytes() -> Result<[u8; 16]> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
fn args() -> Result<Args> {
    let mut it = std::env::args_os().skip(1);
    let (mut image, mut size, mut populate, mut uuid, mut hash_key, mut time) =
        (None, None, None, None, None, None);
    let mut label = [0; 64];
    let mut extents = false;
    let mut seen = BTreeSet::new();
    while let Some(arg) = it.next() {
        if arg == "--help" {
            println!("{USAGE}");
            std::process::exit(0);
        }
        if arg.as_bytes().starts_with(b"--") {
            let flag = arg.to_str().ok_or("invalid option")?;
            if !seen.insert(flag.to_owned()) {
                return Err(format!("repeated option {flag}"));
            }
            if flag == "--extents" {
                extents = true;
                continue;
            }
            if !matches!(
                flag,
                "--populate" | "--size" | "--uuid" | "--hash-key" | "--time" | "--label"
            ) {
                return Err(format!("unknown option {flag}"));
            }
            let value = it
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            if flag == "--populate" {
                populate = Some(PathBuf::from(value));
                continue;
            }
            let value = value
                .to_str()
                .ok_or_else(|| format!("invalid value for {flag}"))?;
            match flag {
                "--size" => size = Some(value.parse::<u64>().map_err(|_| "invalid size in bytes")?),
                "--uuid" => uuid = Some(hex(value)?),
                "--hash-key" => hash_key = Some(hex(value)?),
                "--time" => {
                    time = Some(Time {
                        seconds: value.parse().map_err(|_| "invalid time in seconds")?,
                        nanos: 0,
                    })
                }
                "--label" => {
                    if value.len() > 64 || value.contains('\0') {
                        return Err("label must be at most 64 UTF-8 bytes without NUL".into());
                    }
                    label[..value.len()].copy_from_slice(value.as_bytes());
                }
                _ => unreachable!(),
            }
        } else if image.replace(PathBuf::from(arg)).is_some() {
            return Err("expected one image path".into());
        }
    }
    let size = size.ok_or("--size is required")?;
    if !(16384..=1 << 48).contains(&(size / BLOCK_SIZE as u64)) {
        return Err("size must be 16384 to 2^48 blocks".into());
    }
    Ok(Args {
        image: image.ok_or("IMAGE is required")?,
        size,
        populate,
        uuid: match uuid {
            Some(v) => v,
            None => random_bytes()?,
        },
        hash_key: match hash_key {
            Some(v) => v,
            None => random_bytes()?,
        },
        time,
        label,
        extents,
    })
}
fn operation<T>(r: std::result::Result<T, breenfs::ops::Error>) -> Result<T> {
    r.map_err(|e| format!("writer: {e:?}"))
}
fn io<T>(r: std::io::Result<T>, path: &Path) -> Result<T> {
    r.map_err(|e| format!("{}: {e}", path.display()))
}
fn system_time(t: SystemTime) -> Time {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Time {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos(),
        },
        Err(e) => {
            let d = e.duration();
            let nanos = d.subsec_nanos();
            Time {
                seconds: -(d.as_secs() as i64) - i64::from(nanos != 0),
                nanos: if nanos == 0 { 0 } else { 1_000_000_000 - nanos },
            }
        }
    }
}

struct Source {
    path: PathBuf,
    meta: Metadata,
    target: Option<Vec<u8>>,
    children: Vec<Source>,
}
impl Source {
    fn read(path: &Path) -> Result<Self> {
        let meta = io(fs::symlink_metadata(path), path)?;
        let mut children = Vec::new();
        let mut target = None;
        if meta.is_dir() {
            let mut entries = io(fs::read_dir(path), path)?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name();
                if name.as_bytes().is_empty() || name.as_bytes().len() > 255 {
                    return Err(format!(
                        "{}: name exceeds breenfs limits",
                        e.path().display()
                    ));
                }
                children.push(Self::read(&e.path())?);
            }
        } else if meta.file_type().is_symlink() {
            let bytes = io(fs::read_link(path), path)?
                .as_os_str()
                .as_bytes()
                .to_vec();
            if bytes.is_empty() || bytes.len() > 4095 {
                return Err(format!(
                    "{}: symlink target exceeds breenfs limits",
                    path.display()
                ));
            }
            target = Some(bytes);
        } else if !meta.is_file() {
            return Err(format!("{}: unsupported file kind (only regular files, directories and symlinks are accepted)", path.display()));
        }
        if meta.is_file() && meta.len() > (1u64 << 63) - BLOCK_SIZE as u64 {
            return Err(format!("{}: file exceeds breenfs limits", path.display()));
        }
        Ok(Self {
            path: path.into(),
            meta,
            target,
            children,
        })
    }
    fn aliases(&self, meta: &Metadata) -> bool {
        (self.meta.dev(), self.meta.ino()) == (meta.dev(), meta.ino())
            || self.children.iter().any(|c| c.aliases(meta))
    }
}

// The adapter knows file I/O and records submitted ranges, not tree layouts.
struct ImageDevice {
    file: Mutex<File>,
    blocks: u64,
    written: Mutex<BTreeSet<u64>>,
}
impl BlockDevice for ImageDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> std::result::Result<(), BlockError> {
        if block >= self.blocks || buf.len() != BLOCK_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let mut file = self.file.lock().map_err(|_| BlockError::IoError)?;
        file.seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .and_then(|_| file.read_exact(buf))
            .map_err(|_| BlockError::IoError)
    }
    fn write_block(&self, block: u64, buf: &[u8]) -> std::result::Result<(), BlockError> {
        if block >= self.blocks || buf.len() != BLOCK_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let mut file = self.file.lock().map_err(|_| BlockError::IoError)?;
        file.seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .and_then(|_| file.write_all(buf))
            .map_err(|_| BlockError::IoError)?;
        self.written
            .lock()
            .map_err(|_| BlockError::IoError)?
            .insert(block);
        Ok(())
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.blocks
    }
    fn flush(&self) -> std::result::Result<(), BlockError> {
        self.file
            .lock()
            .map_err(|_| BlockError::IoError)?
            .sync_all()
            .map_err(|_| BlockError::IoError)
    }
}

fn import_source(
    files: &mut Files<'_, DeviceStore<'_>>,
    device: &ImageDevice,
    source: &Source,
    ino: u64,
    now: Time,
    fixed_time: Option<Time>,
    links: &mut BTreeMap<(u64, u64), u64>,
) -> Result<()> {
    if source.meta.is_dir() {
        for child in &source.children {
            let name = child
                .path
                .file_name()
                .ok_or("missing source name")?
                .as_bytes();
            let identity = (child.meta.dev(), child.meta.ino());
            if !child.meta.is_dir() {
                if let Some(&existing) = links.get(&identity) {
                    operation(files.link(existing, ino, name, now))?;
                    continue;
                }
            }
            let child_ino = operation(if child.meta.is_dir() {
                files.mkdir(ino, name, child.meta.mode(), now)
            } else if let Some(target) = &child.target {
                files.symlink(ino, name, target, now)
            } else {
                files.create(ino, name, child.meta.mode(), now)
            })?;
            if !child.meta.is_dir() {
                links.insert(identity, child_ino);
            }
            import_source(files, device, child, child_ino, now, fixed_time, links)?;
        }
    } else if source.meta.is_file() {
        let mut input = io(File::open(&source.path), &source.path)?;
        let mut bytes = vec![0; 256 * BLOCK_SIZE];
        for (start, end) in io(sparse::ranges(&input, source.meta.len()), &source.path)? {
            io(input.seek(SeekFrom::Start(start)), &source.path)?;
            let mut offset = start;
            while offset < end.min(source.meta.len()) {
                let len = (end.min(source.meta.len()) - offset).min(bytes.len() as u64) as usize;
                io(input.read_exact(&mut bytes[..len]), &source.path)?;
                let count = operation(files.write(device, ino, offset, &bytes[..len], now))?;
                if count != len {
                    return Err("short writer import".into());
                }
                offset += len as u64;
            }
        }
        operation(files.truncate(device, ino, source.meta.len(), now))?;
        if io(input.metadata(), &source.path)?.len() != source.meta.len() {
            return Err(format!(
                "{}: source size changed while formatting",
                source.path.display()
            ));
        }
    }
    // Preserve the formatter's root ownership and source modes/timestamps.
    // Restore directory times after its children have changed the directory.
    operation(files.setattr(
        ino,
        &SetAttr {
            mode: Some(source.meta.mode()),
            ..SetAttr::default()
        },
        now,
    ))?;
    let ctime = Time {
        seconds: source.meta.ctime(),
        nanos: source.meta.ctime_nsec() as u32,
    };
    let times = fixed_time.map(|t| [t; 4]).unwrap_or([
        Time {
            seconds: source.meta.atime(),
            nanos: source.meta.atime_nsec() as u32,
        },
        Time {
            seconds: source.meta.mtime(),
            nanos: source.meta.mtime_nsec() as u32,
        },
        ctime,
        source.meta.created().map(system_time).unwrap_or(ctime),
    ]);
    operation(files.set_initial_times(ino, times))
}

fn mkfs(a: Args) -> Result<()> {
    let source = a.populate.as_deref().map(Source::read).transpose()?;
    if source.as_ref().is_some_and(|s| !s.meta.is_dir()) {
        return Err("--populate must name a directory".into());
    }
    let old = match fs::symlink_metadata(&a.image) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if let Some(m) = &old {
        if !m.is_file() || source.as_ref().is_some_and(|s| s.aliases(m)) {
            return Err("IMAGE must be a regular file distinct from the source tree".into());
        }
    }
    if let Some(s) = &source {
        let parent = a
            .image
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let output = io(fs::canonicalize(parent), parent)?
            .join(a.image.file_name().ok_or("invalid image path")?);
        if output.starts_with(io(fs::canonicalize(&s.path), &s.path)?) {
            return Err("IMAGE must be outside --populate DIR".into());
        }
    }
    let file = io(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&a.image),
        &a.image,
    )?;
    io(file.set_len(a.size), &a.image)?;
    let total = a.size / BLOCK_SIZE as u64;
    let now = a.time.unwrap_or_else(|| system_time(SystemTime::now()));
    let device = ImageDevice {
        file: Mutex::new(file),
        blocks: total,
        written: Mutex::new(BTreeSet::new()),
    };
    let mut writer = [0; 32];
    let writer_name = b"breenfs-mkfs v1";
    writer[..writer_name.len()].copy_from_slice(writer_name);
    let sb = Superblock {
        fs_uuid: a.uuid,
        generation: 1,
        slot: 1,
        total_blocks: total,
        global_reserve: (total / 200).clamp(4096, 262144),
        next_root_id: 2,
        dirhash_key: a.hash_key,
        commit_time: now,
        mkfs_time: now,
        label: a.label,
        writer,
        ..Superblock::default()
    };
    let mut files = operation(Files::format(DeviceStore(&device), &sb))?;
    if let Some(source) = &source {
        import_source(
            &mut files,
            &device,
            source,
            2,
            now,
            a.time,
            &mut BTreeMap::new(),
        )?;
    }
    let mut committer = Committer::format(sb);
    operation(files.fsync(&device, &mut committer))?;
    if a.extents {
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for start in device
            .written
            .lock()
            .map_err(|_| "image write ledger poisoned")?
            .iter()
            .copied()
        {
            let len = 1;
            if let Some(last) = merged.last_mut().filter(|last| last.0 + last.1 == start) {
                last.1 += len;
            } else {
                merged.push((start, len));
            }
        }
        let mut stdout = std::io::stdout().lock();
        for (start, len) in merged {
            writeln!(stdout, "{start} {len}").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = args().and_then(mkfs) {
        eprintln!("breenfs-mkfs: {e}\nUsage: {USAGE}");
        std::process::exit(1);
    }
}
