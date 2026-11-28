//! A persistent writer's staging-tree stream (docs/metal/paired-generations.md
//! 3.1, checks T-A1 and T-A2). One writer serves `gen-A` (the booted tree),
//! root 1 (the default tree) and the writable clone `B.new` (the staging
//! tree). A fixed sequence edits the three roots across several commits, then
//! detaches the staging tree and renames `B.new` to `gen-B`. A model of each
//! root, kept beside the writer, gives the expected state at each commit.
use crate::{
    block::{BlockDevice, BlockError},
    commit::{Committer, DeviceStore},
    format::{self, Root, Superblock, Time},
    ops::{self, Error, Files, SetAttr, DEFAULT_TREE, STAGING_TREE},
    recording::{Event, RecordingDevice, SECTOR_SIZE},
    recover,
};
use alloc::{collections::BTreeMap, vec, vec::Vec};
use spin::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    Booted,
    Default,
    Staging,
}

/// One path of a root, as the walk and the model both describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Dir(u32),
    File { mode: u32, links: u32, bytes: Vec<u8> },
    Symlink(Vec<u8>),
}
pub type Manifest = BTreeMap<Vec<u8>, Entry>;
/// The volume's roots by name, each with its read-only flag and manifest.
pub type State = BTreeMap<Vec<u8>, (bool, Manifest)>;

#[derive(Clone, Copy, Debug)]
pub enum Step {
    Mkdir(Tree, &'static str),
    /// Create the file at 0644 if absent, then write at the offset.
    Write(Tree, &'static str, u64, &'static [u8]),
    Truncate(Tree, &'static str, u64),
    Link(Tree, &'static str, &'static str),
    Symlink(Tree, &'static str, &'static [u8]),
    Chmod(Tree, &'static str, u32),
    Unlink(Tree, &'static str),
    Rename(Tree, &'static str, &'static str),
    Commit,
}

use Step::*;
use Tree::*;

/// Staging creates, writes, truncates, links, symlinks, chmods, unlinks and
/// renames, interleaved with booted-root and default-tree
/// writes, across five commits.
pub const STREAM: &[Step] = &[
    Mkdir(Staging, "usr/lib/breenix"),
    Write(Staging, "usr/lib/breenix/rust-current", 0, b"toolchain-1\n"),
    Write(Booted, "etc-local", 0, b"pet change\n"),
    Write(Default, "var/log", 5, b"line two\n"),
    Commit,
    Write(Staging, "bin/tool", 0, b"TOOL v2 replaced in the draft"),
    Write(Staging, "usr/lib/libc.so", 4096, b"second page"),
    Link(Staging, "bin/tool", "sbin/tool"),
    Symlink(Staging, "bin/sh", b"tool"),
    Write(Booted, "bin/tool", 0, b"live"),
    Commit,
    Truncate(Staging, "usr/lib/libc.so", 100),
    Chmod(Staging, "bin/tool", 0o4755),
    Unlink(Staging, "bin/old"),
    Write(Default, "var/log", 0, b"BOOT"),
    Commit,
    Rename(Staging, "usr/lib/breenix/rust-current", "usr/lib/breenix/selected"),
    Mkdir(Staging, "usr/share"),
    Write(Staging, "usr/share/factory-generation", 0, b"B\n"),
    Unlink(Booted, "bin/old"),
    Commit,
];

#[derive(Clone, Debug, Default)]
struct Model {
    names: BTreeMap<Vec<u8>, u64>,
    nodes: BTreeMap<u64, Entry>,
    next: u64,
}

impl Model {
    fn add(&mut self, path: &[u8], entry: Entry) {
        self.next += 1;
        self.names.insert(path.to_vec(), self.next);
        self.nodes.insert(self.next, entry);
    }
    fn file(&mut self, path: &[u8]) -> &mut Vec<u8> {
        if !self.names.contains_key(path) {
            self.add(path, Entry::File { mode: 0o644, links: 1, bytes: Vec::new() });
        }
        let id = self.names[path];
        let Some(Entry::File { bytes, .. }) = self.nodes.get_mut(&id) else { panic!("not a file") };
        bytes
    }
    fn apply(&mut self, step: &Step) {
        match *step {
            Mkdir(_, path) => {
                let mut prefix = Vec::new();
                for part in path.split('/') {
                    if !prefix.is_empty() { prefix.push(b'/'); }
                    prefix.extend_from_slice(part.as_bytes());
                    if !self.names.contains_key(&prefix) { self.add(&prefix, Entry::Dir(0o755)); }
                }
            }
            Write(_, path, offset, data) => {
                let bytes = self.file(path.as_bytes());
                let end = offset as usize + data.len();
                if bytes.len() < end { bytes.resize(end, 0); }
                bytes[offset as usize..end].copy_from_slice(data);
            }
            Truncate(_, path, size) => self.file(path.as_bytes()).resize(size as usize, 0),
            Link(_, from, to) => {
                let id = self.names[from.as_bytes()];
                self.names.insert(to.as_bytes().to_vec(), id);
            }
            Symlink(_, path, target) => self.add(path.as_bytes(), Entry::Symlink(target.to_vec())),
            Chmod(_, path, mode) => {
                let id = self.names[path.as_bytes()];
                if let Some(Entry::File { mode: current, .. } | Entry::Dir(current)) = self.nodes.get_mut(&id) {
                    *current = mode;
                }
            }
            Unlink(_, path) => {
                let id = self.names.remove(path.as_bytes()).unwrap();
                if !self.names.values().any(|&other| other == id) { self.nodes.remove(&id); }
            }
            Rename(_, from, to) => {
                let id = self.names.remove(from.as_bytes()).unwrap();
                self.names.insert(to.as_bytes().to_vec(), id);
            }
            Commit => {}
        }
    }
    fn manifest(&self) -> Manifest {
        self.names.iter().map(|(path, id)| {
            let entry = match &self.nodes[id] {
                Entry::File { mode, bytes, .. } => Entry::File {
                    mode: *mode,
                    links: self.names.values().filter(|&&other| other == *id).count() as u32,
                    bytes: bytes.clone(),
                },
                other => other.clone(),
            };
            (path.clone(), entry)
        }).collect()
    }
}

const AT: Time = Time { seconds: 1_700_000_000, nanos: 0 };

/// The writer, its committer, and one model per root.
pub struct Run<'a> {
    device: &'a RecordingDevice,
    pub files: Files<'a, DeviceStore<'a>>,
    committer: Committer,
    pub sb: Superblock,
    pub staging: u64,
    models: [Model; 3],
    staging_name: &'static [u8],
    /// The expected state after each commit, with the device event count
    /// at that commit's return.
    pub commits: Vec<(usize, State)>,
}

fn root_of(tree: Tree) -> u64 {
    match tree {
        Booted => 2,
        Default => DEFAULT_TREE | 2,
        Staging => STAGING_TREE | 2,
    }
}

fn index(tree: Tree) -> usize {
    match tree {
        Booted => 0,
        Default => 1,
        Staging => 2,
    }
}

impl<'a> Run<'a> {
    /// Build `gen-A` and the default tree, mount them under one persistent
    /// writer, clone `gen-A` to `B.new` and attach it as the staging tree.
    pub fn begin(device: &'a RecordingDevice) -> Result<Self, Error> {
        let sb = recover::recover(device, false)?;
        let mut committer = Committer::new(sb.clone());
        let mut base = Files::new(DeviceStore(device), &sb, 1, true)?;
        let gen_a = base.create_empty_root(b"gen-A", AT)?;
        let sb = base.fsync(device, &mut committer)?;
        let mut files = Files::new(DeviceStore(device), &sb, gen_a, true)?;
        files.include_default_tree()?;
        let mut run = Self {
            device,
            files,
            committer,
            sb,
            staging: 0,
            models: core::default::Default::default(),
            staging_name: b"B.new",
            commits: Vec::new(),
        };
        for step in [
            Mkdir(Default, "var"),
            Write(Default, "var/log", 0, b"boot\n"),
            Mkdir(Booted, "bin"),
            Mkdir(Booted, "sbin"),
            Mkdir(Booted, "usr/lib"),
            Write(Booted, "bin/tool", 0, b"TOOL v1"),
            Write(Booted, "bin/old", 0, b"dropped by the new image"),
            Write(Booted, "usr/lib/libc.so", 0, &[0x7f; 4096]),
            Commit,
        ] {
            run.step(&step)?;
        }
        run.staging = run.files.snapshot(b"B.new", false)?;
        run.models[2] = run.models[0].clone();
        run.commit()?;
        run.files.include_staging_tree(run.staging)?;
        Ok(run)
    }

    fn resolve(&self, tree: Tree, path: &[u8]) -> Result<(u64, u64, Vec<u8>), Error> {
        let mut dir = root_of(tree);
        let mut parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
        let name = parts.pop().unwrap().to_vec();
        for part in parts {
            dir = self.files.lookup(dir, part)?;
        }
        let ino = self.files.lookup(dir, &name).unwrap_or(0);
        Ok((dir, ino, name))
    }

    pub fn step(&mut self, step: &Step) -> Result<(), Error> {
        let device = self.device;
        match *step {
            Mkdir(tree, path) => {
                let mut dir = root_of(tree);
                for part in path.split('/') {
                    dir = match self.files.lookup(dir, part.as_bytes()) {
                        Ok(ino) => ino,
                        Err(Error::ENOENT) => self.files.mkdir(dir, part.as_bytes(), 0o755, AT)?,
                        Err(error) => return Err(error),
                    };
                }
            }
            Write(tree, path, offset, data) => {
                let (dir, mut ino, name) = self.resolve(tree, path.as_bytes())?;
                if ino == 0 { ino = self.files.create(dir, &name, 0o644, AT)?; }
                self.files.write(device, ino, offset, data, AT)?;
            }
            Truncate(tree, path, size) => {
                let (_, ino, _) = self.resolve(tree, path.as_bytes())?;
                self.files.truncate(device, ino, size, AT)?;
            }
            Link(tree, from, to) => {
                let (_, ino, _) = self.resolve(tree, from.as_bytes())?;
                let (dir, _, name) = self.resolve(tree, to.as_bytes())?;
                self.files.link(ino, dir, &name, AT)?;
            }
            Symlink(tree, path, target) => {
                let (dir, _, name) = self.resolve(tree, path.as_bytes())?;
                self.files.symlink(dir, &name, target, AT)?;
            }
            Chmod(tree, path, mode) => {
                let (_, ino, _) = self.resolve(tree, path.as_bytes())?;
                let attrs = SetAttr { mode: Some(mode), uid: None, gid: None, atime: None, mtime: None };
                self.files.setattr(ino, &attrs, AT)?;
            }
            Unlink(tree, path) => {
                let (dir, _, name) = self.resolve(tree, path.as_bytes())?;
                self.files.unlink(dir, &name, AT)?;
            }
            Rename(tree, from, to) => {
                let (from_dir, _, from) = self.resolve(tree, from.as_bytes())?;
                let (to_dir, _, to) = self.resolve(tree, to.as_bytes())?;
                self.files.rename(from_dir, &from, to_dir, &to, AT)?;
            }
            Commit => return self.commit(),
        }
        let tree = match *step {
            Mkdir(tree, _) | Write(tree, ..) | Truncate(tree, ..) | Link(tree, ..) | Symlink(tree, ..)
            | Chmod(tree, ..) | Unlink(tree, _) | Rename(tree, ..) => tree,
            Commit => unreachable!(),
        };
        self.models[index(tree)].apply(step);
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        let sb = self.files.fsync(self.device, &mut self.committer)?;
        self.files.next_generation(DeviceStore(self.device), &sb)?;
        self.sb = sb;
        let state = self.expected();
        self.commits.push((self.device.events().len(), state));
        Ok(())
    }

    /// Detach the committed staging tree, then publish it as `gen-B`.
    pub fn finish(&mut self) -> Result<(), Error> {
        assert_eq!(self.files.exclude_staging_tree()?, self.staging);
        self.files.rename_root(b"B.new", b"gen-B")?;
        self.staging_name = b"gen-B";
        self.commit()
    }

    /// The state the models give for the last step.
    pub fn expected(&self) -> State {
        let mut state = State::new();
        state.insert(b"default".to_vec(), (false, self.models[1].manifest()));
        state.insert(b"gen-A".to_vec(), (false, self.models[0].manifest()));
        if self.staging != 0 {
            state.insert(self.staging_name.to_vec(), (false, self.models[2].manifest()));
        }
        state
    }
}

fn walk<S: crate::btree::NodeStore>(
    files: &Files<'_, S>,
    device: &dyn BlockDevice,
    dir: u64,
    prefix: &[u8],
    out: &mut Manifest,
) -> Result<(), Error> {
    let mut entries = Vec::new();
    files.visit_directory_entries(dir, 0, &mut |_, record| {
        entries.push(record);
        true
    })?;
    for record in entries {
        let mut path = prefix.to_vec();
        if !path.is_empty() { path.push(b'/'); }
        path.extend_from_slice(&record.name);
        let inode = files.inode(record.ino)?;
        let entry = match inode.mode & 0o170000 {
            0o040000 => {
                walk(files, device, record.ino, &path, out)?;
                Entry::Dir(inode.mode & 0o7777)
            }
            0o120000 => Entry::Symlink(files.readlink(record.ino)?),
            _ => {
                let mut bytes = vec![0; inode.size as usize];
                files.read(device, record.ino, 0, &mut bytes)?;
                Entry::File { mode: inode.mode & 0o7777, links: inode.nlink, bytes }
            }
        };
        out.insert(path, entry);
    }
    Ok(())
}

/// The roots of the volume's newest valid generation, read by a fresh mount.
pub fn recovered(device: &dyn BlockDevice) -> Result<State, Error> {
    let sb = recover::recover(device, false)?;
    let store = DeviceStore(device);
    let mut state = State::new();
    for (key, bytes) in ops::items(&store, sb.space_root)? {
        if key.item_type != format::ROOT { continue; }
        let root = Root::decode(&bytes)?;
        let files = Files::new(DeviceStore(device), &sb, key.offset, false)?;
        let mut manifest = Manifest::new();
        walk(&files, device, 2, b"", &mut manifest)?;
        state.insert(root.name, (root.flags & 1 != 0, manifest));
    }
    Ok(state)
}

/// A sparse sector image, as a crash leaves the device.
#[derive(Clone)]
pub struct Image {
    sectors: BTreeMap<u64, [u8; SECTOR_SIZE]>,
    sector_count: u64,
}

impl Image {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut image = Self { sectors: BTreeMap::new(), sector_count: (bytes.len() / SECTOR_SIZE) as u64 };
        for (sector, chunk) in bytes.chunks_exact(SECTOR_SIZE).enumerate() {
            image.write(sector as u64, chunk.try_into().unwrap());
        }
        image
    }
    fn write(&mut self, sector: u64, bytes: [u8; SECTOR_SIZE]) {
        if bytes == [0; SECTOR_SIZE] { self.sectors.remove(&sector); } else { self.sectors.insert(sector, bytes); }
    }
    pub fn sectors(&self) -> impl Iterator<Item = (u64, &[u8; SECTOR_SIZE])> {
        self.sectors.iter().map(|(&sector, bytes)| (sector, bytes))
    }
    pub fn len(&self) -> u64 {
        self.sector_count * SECTOR_SIZE as u64
    }
}

/// The writes issued between two flushes, over the image durable before them.
pub struct Window {
    pub durable: Image,
    pub writes: Vec<(u64, [u8; SECTOR_SIZE])>,
    /// The number of device events up to this window's flush.
    pub end: usize,
}

impl Window {
    pub fn image(&self, mask: &[bool]) -> Image {
        let mut image = self.durable.clone();
        for (&(sector, bytes), &keep) in self.writes.iter().zip(mask) {
            if keep { image.write(sector, bytes); }
        }
        image
    }
}

/// Split the device's recorded events into flush windows.
pub fn windows(initial: &[u8], events: &[Event]) -> Vec<Window> {
    let mut durable = Image::from_bytes(initial);
    let mut pending = Vec::new();
    let mut windows = Vec::new();
    for (at, event) in events.iter().enumerate() {
        match event {
            Event::Write { sector, bytes } => pending.push((*sector, *bytes)),
            Event::Flush => {
                windows.push(Window { durable: durable.clone(), writes: pending.clone(), end: at + 1 });
                for (sector, bytes) in pending.drain(..) { durable.write(sector, bytes); }
            }
        }
    }
    assert!(pending.is_empty(), "unflushed writes");
    windows
}

/// A crash image as an ordinary device, for recovery and the checker.
pub struct MemoryDevice(Mutex<Image>);
impl MemoryDevice {
    pub fn new(image: Image) -> Self { Self(Mutex::new(image)) }
    pub fn image(&self) -> Image { self.0.lock().clone() }
}
impl BlockDevice for MemoryDevice {
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let image = self.0.lock();
        if sector >= image.sector_count { return Err(BlockError::OutOfBounds); }
        let out = bytes.get_mut(..SECTOR_SIZE).ok_or(BlockError::IoError)?;
        out.copy_from_slice(image.sectors.get(&sector).unwrap_or(&[0; SECTOR_SIZE]));
        Ok(())
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let mut image = self.0.lock();
        if sector >= image.sector_count { return Err(BlockError::OutOfBounds); }
        image.write(sector, bytes.get(..SECTOR_SIZE).ok_or(BlockError::IoError)?.try_into().unwrap());
        Ok(())
    }
    fn block_size(&self) -> usize { SECTOR_SIZE }
    fn num_blocks(&self) -> u64 { self.0.lock().sector_count }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
