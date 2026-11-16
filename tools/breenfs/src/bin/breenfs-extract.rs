//! Read images through recovery and the shared filesystem reader.
use breenfs::block::{BlockDevice, BlockError};
use breenfs::commit::DeviceStore;
use breenfs::format::{self, Inode, Root, Superblock, BLOCK_SIZE};
use breenfs::ops::{self, Files};
use breenfs::sha256::{sha256, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::Path;
use std::sync::Mutex;

#[path = "../sparse.rs"]
mod sparse;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const USAGE: &str = "breenfs-extract IMAGE ls PATH|cat PATH|dump|manifest\n       breenfs-extract manifest --dir DIR";

fn core<T, E: std::fmt::Debug>(r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|e| format!("breenfs: {e:?}").into())
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

struct Image {
    file: Mutex<File>,
    blocks: u64,
}
impl Image {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err("IMAGE must be a regular image file".into());
        }
        Ok(Self {
            file: Mutex::new(file),
            blocks: meta.len() / BLOCK_SIZE as u64,
        })
    }
}
impl BlockDevice for Image {
    fn read_block(&self, block: u64, out: &mut [u8]) -> std::result::Result<(), BlockError> {
        if block >= self.blocks || out.len() != BLOCK_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let mut file = self.file.lock().map_err(|_| BlockError::IoError)?;
        file.seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .and_then(|_| file.read_exact(out))
            .map_err(|_| BlockError::IoError)
    }
    fn write_block(&self, _: u64, _: &[u8]) -> std::result::Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.blocks
    }
    fn flush(&self) -> std::result::Result<(), BlockError> {
        Err(BlockError::IoError)
    }
}

fn kind(mode: u32) -> Result<&'static str> {
    Ok(match mode & 0o170000 {
        0o100000 => "reg",
        0o040000 => "dir",
        0o120000 => "symlink",
        0o020000 => "chr",
        0o060000 => "blk",
        0o010000 => "fifo",
        0o140000 => "sock",
        _ => return Err("invalid inode type".into()),
    })
}
fn join(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = parent.to_vec();
    if path != b"/" {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}
fn parts(path: &[u8]) -> VecDeque<Vec<u8>> {
    path.split(|&b| b == b'/')
        .filter(|p| !p.is_empty())
        .map(Vec::from)
        .collect()
}
fn resolve(files: &Files<'_, DeviceStore<'_>>, path: &[u8], follow: bool) -> Result<u64> {
    let mut todo = parts(path);
    let mut ino = 2;
    let mut links = 0;
    while let Some(name) = todo.pop_front() {
        if kind(core(files.inode(ino))?.mode)? != "dir" {
            return Err("not a directory".into());
        }
        if name == b"." {
            continue;
        }
        if name == b".." {
            ino = core(files.inode(ino))?.parent;
            continue;
        }
        let child = core(files.lookup(ino, &name))?;
        if kind(core(files.inode(child))?.mode)? == "symlink" && (follow || !todo.is_empty()) {
            links += 1;
            if links > 40 {
                return Err("too many symbolic links".into());
            }
            let target = core(files.readlink(child))?;
            if target.starts_with(b"/") {
                ino = 2;
            }
            let mut next = parts(&target);
            next.append(&mut todo);
            todo = next;
        } else {
            ino = child;
        }
    }
    Ok(ino)
}
fn entries(files: &Files<'_, DeviceStore<'_>>, dir: u64) -> Result<Vec<format::DirRecord>> {
    let mut result = Vec::new();
    let mut cookie = 0;
    loop {
        let batch = core(files.directory_entries(dir, cookie, 128))?;
        if batch.is_empty() {
            break;
        }
        for (next, record) in batch {
            if next <= cookie {
                return Err("invalid directory cookie".into());
            }
            cookie = next;
            result.push(record);
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
fn content(
    files: &Files<'_, DeviceStore<'_>>,
    device: &Image,
    ino: u64,
    out: &mut impl Write,
) -> Result<()> {
    let mut bytes = vec![0; 256 * BLOCK_SIZE];
    let inode = core(files.inode(ino))?;
    if kind(inode.mode)? != "reg" {
        return Err("cat requires a regular file".into());
    }
    let size = inode.size;
    let mut offset = 0;
    while offset < size {
        let n = core(files.read(device, ino, offset, &mut bytes))?;
        if n == 0 {
            return Err("short image read".into());
        }
        out.write_all(&bytes[..n])?;
        offset += n as u64;
    }
    Ok(())
}
struct Hash(Sha256);
impl Write for Hash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Row {
    path: Vec<u8>,
    inode: Inode,
    digest: String,
    identity: (u64, u64),
}
fn image_rows(files: &Files<'_, DeviceStore<'_>>, device: &Image) -> Result<Vec<Row>> {
    let mut todo = vec![(b"/".to_vec(), 2)];
    let mut dirs = BTreeSet::new();
    let mut rows = Vec::new();
    while let Some((path, ino)) = todo.pop() {
        let inode = core(files.inode(ino))?;
        let digest = match kind(inode.mode)? {
            "dir" => {
                if !dirs.insert(ino) {
                    return Err("directory cycle or alias".into());
                }
                for entry in entries(files, ino)? {
                    todo.push((join(&path, &entry.name), entry.ino));
                }
                "-".into()
            }
            "reg" => {
                let mut hash = Hash(Sha256::new());
                content(files, device, ino, &mut hash)?;
                hex(&hash.0.finalize())
            }
            "symlink" => hex(&sha256(&core(files.readlink(ino))?)),
            _ => "-".into(),
        };
        rows.push(Row {
            path,
            inode,
            digest,
            identity: (1, ino),
        });
    }
    Ok(rows)
}
fn source_rows(dir: &Path) -> Result<Vec<Row>> {
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err("--dir must name a directory".into());
    }
    let mut todo = vec![(dir.to_path_buf(), b"/".to_vec())];
    let mut rows = Vec::new();
    while let Some((source, path)) = todo.pop() {
        let meta = fs::symlink_metadata(&source)?;
        let mut inode = Inode {
            mode: meta.mode(),
            uid: 0,
            gid: 0,
            ..Inode::default()
        };
        inode.mtime = format::Time {
            seconds: meta.mtime(),
            nanos: meta.mtime_nsec() as u32,
        };
        let digest = if meta.is_dir() {
            let mut children = fs::read_dir(&source)?.collect::<io::Result<Vec<_>>>()?;
            children.sort_by_key(|e| e.file_name());
            inode.size = children.len() as u64;
            for entry in children {
                let name = entry.file_name();
                if name.as_bytes().len() > 255 {
                    return Err("name exceeds breenfs limits".into());
                }
                todo.push((entry.path(), join(&path, name.as_bytes())));
            }
            "-".into()
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(&source)?;
            let bytes = target.as_os_str().as_bytes();
            if bytes.is_empty() || bytes.len() > 4095 {
                return Err("symlink exceeds breenfs limits".into());
            }
            inode.size = bytes.len() as u64;
            hex(&sha256(bytes))
        } else if meta.is_file() {
            inode.size = meta.len();
            if inode.size > (1u64 << 63) - BLOCK_SIZE as u64 {
                return Err("file exceeds breenfs limits".into());
            }
            let mut file = File::open(&source)?;
            inode.nblocks = sparse::ranges(&file, inode.size)?
                .iter()
                .map(|(a, b)| (b - a) / BLOCK_SIZE as u64)
                .sum();
            file.seek(SeekFrom::Start(0))?;
            let mut hash = Hash(Sha256::new());
            let n = io::copy(&mut file, &mut hash)?;
            if n != inode.size {
                return Err("source size changed while hashing".into());
            }
            hex(&hash.0.finalize())
        } else {
            return Err("unsupported source file kind (only regular files, directories and symlinks are accepted)".into());
        };
        rows.push(Row {
            path,
            inode,
            digest,
            identity: (meta.dev(), meta.ino()),
        });
    }
    Ok(rows)
}
fn manifest(mut rows: Vec<Row>, out: &mut impl Write) -> Result<()> {
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups: BTreeMap<(u64, u64), (usize, Vec<u8>)> = BTreeMap::new();
    for row in &rows {
        if kind(row.inode.mode)? != "dir" {
            let group = groups.entry(row.identity).or_insert((0, row.path.clone()));
            group.0 += 1;
        }
    }
    for row in rows {
        let i = row.inode;
        let group = groups
            .get(&row.identity)
            .filter(|g| g.0 > 1)
            .map(|g| hex(&g.1))
            .unwrap_or_else(|| "-".into());
        writeln!(
            out,
            "{}\t{}\t{:06o}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            hex(&row.path),
            kind(i.mode)?,
            i.mode,
            i.uid,
            i.gid,
            i.size,
            i.nblocks
                .checked_mul(8)
                .ok_or("mapped block count overflow")?,
            i.mtime.seconds,
            i.mtime.nanos,
            row.digest,
            group
        )?;
    }
    Ok(())
}
fn dump(device: &Image, sb: &Superblock, out: &mut impl Write) -> Result<()> {
    writeln!(
        out,
        "superblock generation={} uuid={} total_blocks={} free_blocks={}",
        sb.generation,
        hex(&sb.fs_uuid),
        sb.total_blocks,
        sb.free_blocks
    )?;
    let space = core(ops::items(&DeviceStore(device), sb.space_root))?;
    for (key, data) in &space {
        writeln!(
            out,
            "space object={} type={:02x} offset={} data={}",
            key.object,
            key.item_type,
            key.offset,
            hex(data)
        )?;
    }
    for (key, data) in space
        .iter()
        .filter(|(key, _)| key.item_type == format::ROOT)
    {
        let root = core(Root::decode(data))?;
        for (item, data) in core(ops::items(&DeviceStore(device), root.root))? {
            writeln!(
                out,
                "fs root={} object={} type={:02x} offset={} data={}",
                key.offset,
                item.object,
                item.item_type,
                item.offset,
                hex(&data)
            )?;
        }
    }
    Ok(())
}
fn run(args: Vec<OsString>) -> Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    if args.len() == 3 && args[0] == "manifest" && args[1] == "--dir" {
        manifest(source_rows(Path::new(&args[2]))?, &mut out)?;
    } else {
        if args.len() < 2 {
            return Err(USAGE.into());
        }
        let device = Image::open(Path::new(&args[0]))?;
        let sb = core(breenfs::recover::recover(&device, false))?;
        let command = args[1].to_str().ok_or("invalid command")?;
        match (command, args.len()) {
            ("dump", 2) => dump(&device, &sb, &mut out)?,
            ("manifest", 2) => {
                let files = core(Files::new(DeviceStore(&device), &sb, 1))?;
                manifest(image_rows(&files, &device)?, &mut out)?;
            }
            ("cat" | "ls", 3) => {
                let files = core(Files::new(DeviceStore(&device), &sb, 1))?;
                let path = args[2].as_bytes();
                let ino = resolve(&files, path, command == "cat")?;
                if command == "cat" {
                    content(&files, &device, ino, &mut out)?;
                } else {
                    let mut display = b"/".to_vec();
                    for part in parts(path) {
                        display = join(&display, &part);
                    }
                    if kind(core(files.inode(ino))?.mode)? == "dir" {
                        for entry in entries(&files, ino)? {
                            writeln!(
                                out,
                                "{}\t{}",
                                kind(core(files.inode(entry.ino))?.mode)?,
                                hex(&join(&display, &entry.name))
                            )?;
                        }
                    } else {
                        writeln!(
                            out,
                            "{}\t{}",
                            kind(core(files.inode(ino))?.mode)?,
                            hex(&display)
                        )?;
                    }
                }
            }
            _ => return Err(USAGE.into()),
        }
    }
    out.flush()?;
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("{USAGE}");
        return;
    }
    if let Err(e) = run(args) {
        eprintln!("breenfs-extract: {e}");
        std::process::exit(1);
    }
}
