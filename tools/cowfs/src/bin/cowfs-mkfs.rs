//! Initial-image writer; on-disk layouts and tree edits come from the cowfs core.
use cowfs::btree::{self, NodeStore};
use cowfs::format::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, String>;
const ROOT_OBJECT: u64 = 0xffff_ffff_ffff_ff10;
const FREE_OBJECT: u64 = 0xffff_ffff_ffff_ff20;
const USAGE: &str = "cowfs-mkfs [--populate DIR] [--uuid HEX] [--time SECS] [--hash-key HEX] [--label L] [--extents] --size BYTES IMAGE";

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
fn key(object: u64, item_type: u8, offset: u64) -> Key {
    Key {
        object,
        item_type,
        offset,
    }
}
fn tree<T>(r: std::result::Result<T, btree::Error>) -> Result<T> {
    r.map_err(|e| format!("B-tree: {e:?}"))
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
                    return Err(format!("{}: name exceeds cowfs limits", e.path().display()));
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
                    "{}: symlink target exceeds cowfs limits",
                    path.display()
                ));
            }
            target = Some(bytes);
        } else if !meta.is_file() {
            return Err(format!("{}: unsupported file kind (only regular files, directories and symlinks are accepted)", path.display()));
        }
        if meta.is_file() && meta.len() > (1u64 << 63) - BLOCK_SIZE as u64 {
            return Err(format!("{}: file exceeds cowfs limits", path.display()));
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

struct Store {
    nodes: BTreeMap<u64, Vec<u8>>,
    next: u64,
    end: u64,
}
impl Store {
    fn new(next: u64, end: u64) -> Self {
        Self {
            nodes: BTreeMap::new(),
            next,
            end,
        }
    }
    fn reserve(&mut self, count: u64) -> Result<u64> {
        let next = self
            .next
            .checked_add(count)
            .filter(|n| *n <= self.end)
            .ok_or("image is too small for populated data and metadata")?;
        let addr = self.next;
        self.next = next;
        Ok(addr)
    }
    fn reachable(&self, root: BlockPtr) -> Result<Vec<u64>> {
        let mut pending = vec![root.addr];
        let mut seen = BTreeSet::new();
        while let Some(addr) = pending.pop() {
            if !seen.insert(addr) {
                return Err("cycle or shared child in initial tree".into());
            }
            let b = self.nodes.get(&addr).ok_or("missing initial node")?;
            let h = NodeHeader::decode(b).map_err(|e| format!("node: {e:?}"))?;
            if h.level > 0 {
                for i in 0..usize::from(h.nitems) {
                    let e = InternalEntry::decode(&b[48 + i * 37..48 + (i + 1) * 37])
                        .map_err(|e| format!("entry: {e:?}"))?;
                    pending.push(e.child_addr);
                }
            }
        }
        Ok(seen.into_iter().collect())
    }
}
impl NodeStore for Store {
    fn get(&self, addr: u64) -> std::result::Result<Vec<u8>, btree::Error> {
        self.nodes.get(&addr).cloned().ok_or(btree::Error::Store)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> std::result::Result<(), btree::Error> {
        self.nodes.insert(addr, bytes);
        Ok(())
    }
    fn alloc(&mut self) -> std::result::Result<u64, btree::Error> {
        self.reserve(4).map_err(|_| btree::Error::Store)
    }
}
fn write_at(file: &mut File, addr: u64, bytes: &[u8]) -> Result<()> {
    file.seek(SeekFrom::Start(addr * BLOCK_SIZE as u64))
        .and_then(|_| file.write_all(bytes))
        .map_err(|e| format!("write block {addr}: {e}"))
}
fn node_ref(kind: u8) -> Ref {
    Ref {
        refcount: 1,
        kind,
        alloc_generation: 1,
        crcs: Vec::new(),
    }
}

struct Builder {
    store: Store,
    root: BlockPtr,
    refs: BTreeMap<Key, Vec<u8>>,
    next_inode: u64,
    time: Option<Time>,
    now: Time,
    hash_key: [u8; 16],
}
impl Builder {
    fn insert(&mut self, k: Key, v: &[u8]) -> Result<()> {
        tree(btree::insert(&mut self.store, &mut self.root, k, v)).map(|_| ())
    }
    fn inode(&self, source: Option<&Source>, parent: u64) -> Inode {
        let mut inode = Inode {
            mode: 0o40755,
            nlink: 2,
            uid: 0,
            gid: 0,
            size: 0,
            nblocks: 0,
            parent,
            rdev: 0,
            next_index: 2,
            created_generation: 1,
            atime: self.now,
            mtime: self.now,
            ctime: self.now,
            btime: self.now,
        };
        if let Some(s) = source {
            inode.mode = s.meta.mode() & 0o177777;
            inode.atime = Time {
                seconds: s.meta.atime(),
                nanos: s.meta.atime_nsec() as u32,
            };
            inode.mtime = Time {
                seconds: s.meta.mtime(),
                nanos: s.meta.mtime_nsec() as u32,
            };
            inode.ctime = Time {
                seconds: s.meta.ctime(),
                nanos: s.meta.ctime_nsec() as u32,
            };
            inode.btime = s.meta.created().map(system_time).unwrap_or(inode.ctime);
            if !s.meta.is_dir() {
                inode.nlink = 1;
                inode.parent = 0;
                inode.next_index = 0;
            }
        }
        if let Some(t) = self.time {
            inode.atime = t;
            inode.mtime = t;
            inode.ctime = t;
            inode.btime = t;
        }
        inode
    }
    fn populate(
        &mut self,
        file: &mut File,
        source: Option<&Source>,
        ino: u64,
        parent: u64,
    ) -> Result<()> {
        let mut inode = self.inode(source, parent);
        if let Some(s) = source {
            if s.meta.is_dir() {
                for child in &s.children {
                    let child_ino = self.next_inode;
                    self.next_inode = self
                        .next_inode
                        .checked_add(1)
                        .ok_or("inode numbers exhausted")?;
                    let kind = if child.meta.is_dir() {
                        inode.nlink = inode
                            .nlink
                            .checked_add(1)
                            .ok_or("too many subdirectories")?;
                        2
                    } else if child.meta.is_file() {
                        1
                    } else {
                        7
                    };
                    let name = child
                        .path
                        .file_name()
                        .ok_or("missing source name")?
                        .as_bytes()
                        .to_vec();
                    let record = DirRecord {
                        ino: child_ino,
                        kind,
                        name,
                    }
                    .encode();
                    let k = key(
                        ino,
                        DIR_ENTRY,
                        cowfs::siphash::hash(
                            &self.hash_key,
                            child.path.file_name().unwrap().as_bytes(),
                        ),
                    );
                    let mut bucket =
                        tree(btree::search(&self.store, self.root, k))?.unwrap_or_default();
                    bucket.extend_from_slice(&record);
                    self.insert(k, &bucket)?;
                    self.insert(key(ino, DIR_INDEX, inode.next_index), &record)?;
                    inode.next_index += 1;
                    inode.size += 1;
                    self.populate(file, Some(child), child_ino, ino)?;
                }
            } else if let Some(target) = &s.target {
                inode.size = target.len() as u64;
                self.insert(key(ino, INLINE, 0), target)?;
            } else {
                inode.size = s.meta.len();
                let mut input = io(File::open(&s.path), &s.path)?;
                let mut remaining = inode.size;
                let mut logical = 0;
                while remaining > 0 {
                    let len = remaining.min(256 * BLOCK_SIZE as u64) as usize;
                    let blocks = len.div_ceil(BLOCK_SIZE);
                    let mut bytes = vec![0; blocks * BLOCK_SIZE];
                    io(input.read_exact(&mut bytes[..len]), &s.path)?;
                    let addr = self.store.reserve(blocks as u64)?;
                    write_at(file, addr, &bytes)?;
                    let crcs = bytes
                        .chunks_exact(BLOCK_SIZE)
                        .map(cowfs::crc32c::checksum)
                        .collect();
                    self.refs.insert(
                        key(addr, REF, blocks as u64),
                        Ref {
                            crcs,
                            ..node_ref(3)
                        }
                        .encode(),
                    );
                    self.insert(
                        key(ino, EXTENT, logical),
                        &Extent {
                            phys_start: addr,
                            phys_len: blocks as u32,
                            extent_offset: 0,
                            num_blocks: blocks as u32,
                            flags: 0,
                        }
                        .encode(),
                    )?;
                    inode.nblocks += blocks as u64;
                    logical += bytes.len() as u64;
                    remaining -= len as u64;
                }
                if io(input.read(&mut [0]), &s.path)? != 0 {
                    return Err(format!(
                        "{}: source grew while formatting",
                        s.path.display()
                    ));
                }
            }
        }
        self.insert(key(ino, INODE, 0), &inode.encode())
    }
}

// Compute the complement of the referenced allocations, including space nodes.
fn free_ranges(
    refs: &BTreeMap<Key, Vec<u8>>,
    space_nodes: &[u64],
    end: u64,
) -> Result<Vec<(u64, u64)>> {
    let mut used: Vec<_> = refs
        .keys()
        .map(|k| (k.object, k.offset))
        .chain(space_nodes.iter().map(|a| (*a, 4)))
        .collect();
    used.sort_unstable();
    let mut free = Vec::new();
    let mut cursor = 20;
    for (start, len) in used {
        if start < cursor || start + len > end {
            return Err("overlapping or out-of-range initial allocations".into());
        }
        if start > cursor {
            free.push((cursor, start - cursor));
        }
        cursor = start + len;
    }
    if cursor < end {
        free.push((cursor, end - cursor));
    }
    Ok(free)
}
fn space_tree(builder: &Builder, uuid: [u8; 16]) -> Result<(Store, BlockPtr, u64)> {
    let start = builder.store.next;
    let end = builder.store.end;
    let mut own_nodes = Vec::new();
    let mut attempts = BTreeSet::new();
    loop {
        if !attempts.insert(own_nodes.clone()) {
            return Err("space-tree allocation fixed point did not converge".into());
        }
        let mut items = builder.refs.clone();
        for addr in &own_nodes {
            items.insert(key(*addr, REF, 4), node_ref(2).encode());
        }
        items.insert(
            key(ROOT_OBJECT, ROOT, 1),
            Root {
                root: builder.root,
                next_inode: builder.next_inode,
                source_root: 0,
                created_generation: 1,
                ctime: builder.now,
                flags: 0,
                name: b"default".to_vec(),
            }
            .encode(),
        );
        let free = free_ranges(&builder.refs, &own_nodes, end)?;
        let free_blocks = free.iter().map(|(_, len)| len).sum();
        for (addr, len) in free {
            items.insert(
                key(FREE_OBJECT, FREE, addr),
                Free { length: len }.encode().to_vec(),
            );
        }
        let mut store = Store::new(start, end);
        let mut root = tree(btree::create(&mut store, 2, uuid, 1))?;
        for (k, v) in items {
            tree(btree::insert(&mut store, &mut root, k, &v))?;
        }
        let actual = store.reachable(root)?;
        if actual == own_nodes {
            return Ok((store, root, free_blocks));
        }
        own_nodes = actual;
    }
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
    let mut file = io(
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
    let mut store = Store::new(20, total - 4);
    let root = tree(btree::create(&mut store, 1, a.uuid, 1))?;
    let mut builder = Builder {
        store,
        root,
        refs: BTreeMap::new(),
        next_inode: 16,
        time: a.time,
        now,
        hash_key: a.hash_key,
    };
    builder.populate(&mut file, source.as_ref(), 2, 2)?;
    let fs_nodes = builder.store.reachable(builder.root)?;
    for addr in &fs_nodes {
        builder
            .refs
            .insert(key(*addr, REF, 4), node_ref(1).encode());
    }
    let (space, space_root, free_blocks) = space_tree(&builder, a.uuid)?;
    let global_reserve = (total / 200).clamp(4096, 262144);
    if free_blocks < global_reserve {
        return Err("image is too small to retain the global reserve".into());
    }
    let mut written: Vec<_> = builder.refs.keys().map(|k| (k.object, k.offset)).collect();
    for addr in fs_nodes {
        write_at(&mut file, addr, &builder.store.nodes[&addr])?;
    }
    for addr in space.reachable(space_root)? {
        write_at(&mut file, addr, &space.nodes[&addr])?;
        written.push((addr, 4));
    }
    io(file.sync_all(), &a.image)?;
    let mut writer = [0; 32];
    writer[..13].copy_from_slice(b"cowfs-mkfs v1");
    let mut sb = Superblock {
        fs_uuid: a.uuid,
        generation: 1,
        slot: 1,
        total_blocks: total,
        space_root,
        free_blocks,
        global_reserve,
        next_root_id: 2,
        dirhash_key: a.hash_key,
        commit_time: now,
        mkfs_time: now,
        label: a.label,
        writer,
        ..Superblock::default()
    };
    for (ring, start) in [(0, 16), (1, total - 4)] {
        sb.ring = ring;
        for slot in 0..4 {
            write_at(
                &mut file,
                start + slot,
                &if slot == 1 {
                    sb.encode()
                } else {
                    [0; BLOCK_SIZE]
                },
            )?;
        }
        written.push((start, 4));
    }
    io(file.sync_all(), &a.image)?;
    if a.extents {
        written.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (start, len) in written {
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
        eprintln!("cowfs-mkfs: {e}\nUsage: {USAGE}");
        std::process::exit(1);
    }
}
