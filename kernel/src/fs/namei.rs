//! Pathname resolution.
//!
//! Every pathname-taking syscall resolves its pathname here, once, and acts on
//! the result. The walk keeps the stack of physical directories it has entered:
//! a symlink component is expanded in place before any later component is
//! applied, so a `..` after it leaves the directory the link names, not the one
//! the text names. The pathname a resolution reports is that physical stack,
//! with no symlink, `.` or `..` component.
//!
//! A walk holds the read guard of the filesystem it is in across every step
//! there, so no directory it has entered and no symlink it reads can be removed
//! and its inode reused under it. It changes filesystem only at a filesystem
//! root, whose inode is never reused. The inode a lookup resolves to, or the
//! directory an absent name would be created in, is held by the result, so a
//! caller acting on it after the guard is released acts on what the walk found.
//!
//! The working directory is a directory, not a string: an ext2 directory is
//! held by a live-inode handle, and its pathname is derived from the directory
//! itself, by walking `..` entries, whenever it is needed. A rename of the
//! directory or of an ancestor is therefore seen at once, and the handle keeps
//! the directory from being removed (rmdir fails with EBUSY) or its inode
//! reused while it is anyone's working directory.
//!
//! The ext2 root filesystem holds `/`. The home filesystem is entered at
//! `/home` when mounted. devfs and procfs are entered at `/dev` and `/proc`;
//! they have no symlinks, and each name below their mount points is checked
//! against the entries they hold.

use crate::fs::ext2::{self, live_inode::FileHandle, DirReader, FileType, EXT2_ROOT_INO};
use crate::syscall::errno::{EIO, ELOOP, ENAMETOOLONG, ENOENT, ENOTDIR};
use alloc::string::String;
use alloc::vec::Vec;

/// Longest pathname a syscall accepts, counting its terminating NUL (Linux).
pub const PATH_MAX: usize = 4096;
/// Longest name one pathname component may have.
pub const NAME_MAX: usize = 255;
/// Symlinks one resolution may expand before it fails with ELOOP (Linux).
pub const MAX_SYMLINKS: u32 = 40;

/// An ext2 filesystem in the namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    /// The root filesystem, at `/`.
    Root,
    /// The home filesystem, at `/home`.
    Home,
}

impl Mount {
    pub fn read(self) -> ext2::Ext2ReadGuard {
        match self {
            Mount::Root => ext2::root_fs_read(),
            Mount::Home => ext2::home_fs_read(),
        }
    }

    pub fn write(self) -> ext2::Ext2WriteGuard {
        match self {
            Mount::Root => ext2::root_fs_write(),
            Mount::Home => ext2::home_fs_write(),
        }
    }
}

/// What a resolved pathname names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// An existing ext2 inode. A final symlink is reported as itself only when
    /// the resolution did not follow it.
    Inode {
        mount: Mount,
        ino: u32,
        file_type: FileType,
    },
    /// The final component names nothing; `parent` is the existing directory
    /// it would be created in.
    Absent { mount: Mount, parent: u32 },
    /// A pathname at or below `/dev` or `/proc`.
    Virtual,
}

/// The kind of the final component as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Last {
    /// An ordinary name.
    Name,
    /// `.`, or a pathname with no component other than `.`.
    Dot,
    /// `..`.
    DotDot,
    /// `/`, with no component at all.
    Root,
}

/// The result of resolving a pathname.
#[derive(Debug)]
pub struct Resolved {
    /// The physical absolute pathname of the target: no symlink, `.` or `..`
    /// component, except a final symlink the resolution did not follow.
    pub path: String,
    pub target: Target,
    pub last: Last,
    /// The pathname ended in `/`, or its final symlink's target did, so its
    /// final component must be a directory.
    pub trailing_slash: bool,
    /// The final virtual component was absent during the namespace walk.
    pub virtual_absent: bool,
    /// For a lookup, the target inode, or for an absent target the directory
    /// it would be created in: held from the walk step that found it, so its
    /// inode is not reclaimed and its number reused while this lives.
    pin: Option<FileHandle>,
    /// For a rename, the directory holding the final entry, held so the
    /// entry is looked up again in the directory the walk found.
    entry_dir: Option<FileHandle>,
}

impl Resolved {
    /// The pathname within the ext2 filesystem that holds the target.
    pub fn fs_path(&self) -> &str {
        match self.target {
            Target::Inode {
                mount: Mount::Home, ..
            }
            | Target::Absent {
                mount: Mount::Home, ..
            } => ext2::strip_home_prefix(&self.path),
            _ => &self.path,
        }
    }

    /// The ext2 filesystem that holds the target, if any.
    pub fn mount(&self) -> Option<Mount> {
        match self.target {
            Target::Inode { mount, .. } | Target::Absent { mount, .. } => Some(mount),
            Target::Virtual => None,
        }
    }

    /// Whether the target is the root of a filesystem: `/`, `/home`, `/dev`
    /// or `/proc`.
    pub fn is_mount_point(&self) -> bool {
        match self.target {
            Target::Inode { ino, .. } => ino == EXT2_ROOT_INO,
            Target::Virtual => self.path == "/dev" || self.path == "/proc",
            Target::Absent { .. } => false,
        }
    }

    /// The handle a lookup holds on an existing ext2 target.
    pub fn handle(&self) -> Option<&FileHandle> {
        match self.target {
            Target::Inode { .. } => self.pin.as_ref(),
            _ => None,
        }
    }

    /// The directory a rename resolution holds for its final name.
    pub fn entry_dir(&self) -> Option<&FileHandle> {
        self.entry_dir.as_ref()
    }

    /// The final component of the physical pathname.
    pub fn final_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Node {
    Ext2(Mount, u32),
    Virtual,
}

struct Frame {
    name: String,
    node: Node,
}

const ROOT: Node = Node::Ext2(Mount::Root, EXT2_ROOT_INO);

fn top(stack: &[Frame]) -> Node {
    stack.last().map_or(ROOT, |frame| frame.node)
}

fn join(stack: &[Frame]) -> String {
    if stack.is_empty() {
        return String::from("/");
    }
    let mut path = String::new();
    for frame in stack {
        path.push('/');
        path.push_str(&frame.name);
    }
    path
}

/// Push the components of `path` onto `pending` so they pop in order.
fn push_components(pending: &mut Vec<String>, path: &str) -> Result<(), u64> {
    let start = pending.len();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        if name.len() > NAME_MAX {
            return Err(ENAMETOOLONG as u64);
        }
        pending.push(String::from(name));
    }
    pending[start..].reverse();
    Ok(())
}

/// The read guard of the ext2 filesystem a walk is in.
struct Held(Option<(Mount, ext2::Ext2ReadGuard)>);

impl Held {
    /// The filesystem `mount`, taking its guard after releasing any other.
    fn fs(&mut self, mount: Mount) -> Result<&ext2::Ext2Fs, u64> {
        if self.0.as_ref().map_or(true, |(held, _)| *held != mount) {
            self.0 = None;
            self.0 = Some((mount, mount.read()));
        }
        self.0
            .as_ref()
            .and_then(|(_, guard)| guard.as_ref())
            .ok_or(ENOENT as u64)
    }

    fn release(&mut self) {
        self.0 = None;
    }
}

fn read_dir_inode(fs: &ext2::Ext2Fs, ino: u32) -> Result<ext2::Ext2Inode, u64> {
    let inode = fs.read_inode(ino).map_err(|_| EIO as u64)?;
    if !inode.is_dir() {
        return Err(ENOTDIR as u64);
    }
    // A removed directory holds no names, and nothing may be created in it.
    if unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) } == 0 {
        return Err(ENOENT as u64);
    }
    Ok(inode)
}

/// Whether a devfs or procfs pathname names a directory (`Some(true)`),
/// another entry (`Some(false)`) or nothing.
fn virtual_entry(path: &str) -> Option<bool> {
    if path == "/dev" || path == "/proc" || path == "/dev/pts" {
        return Some(true);
    }
    if let Some(pty) = path.strip_prefix("/dev/pts/") {
        return crate::fs::devptsfs::lookup(pty).map(|_| false);
    }
    if let Some(device) = path.strip_prefix("/dev/") {
        return crate::fs::devfs::lookup(device).map(|_| false);
    }
    crate::fs::procfs::lookup_by_path(path).map(|entry| entry.entry_type.is_directory())
}

/// How a resolution treats its final component.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Look the target up: a final symlink is followed when `follow` is set
    /// or the pathname ends in `/`. The result holds the target.
    Lookup { follow: bool },
    /// Act on the directory entry itself (unlink, rmdir): a final symlink
    /// is never followed, and a trailing `/` requires the entry itself to be
    /// a directory.
    Entry,
    /// As `Entry`, for rename, and the result holds the directory the final
    /// name is in.
    Rename,
    /// Create the final name (mkdir, symlink, link, mknod): a final symlink,
    /// dangling or not, is an existing name, whatever follows it.
    Create,
}

/// Resolve `path` from the calling process's working directory (or `/` when
/// the caller has no process). `follow` says whether a final symlink is
/// followed; symlinks in every earlier component always are, and so is a
/// final one when the pathname ends in `/`.
pub fn resolve(path: &str, follow: bool) -> Result<Resolved, u64> {
    walk(path, Mode::Lookup { follow })
}

/// Resolve `path` to the directory entry an unlink, rmdir or rename acts on.
/// A final symlink is not followed, and a pathname ending in `/` fails with
/// ENOTDIR unless that entry is a directory.
pub fn resolve_entry(path: &str) -> Result<Resolved, u64> {
    walk(path, Mode::Entry)
}

/// Resolve `path` to the directory entry a rename acts on, as
/// `resolve_entry` does, holding the directory the final name is in.
pub fn resolve_rename(path: &str) -> Result<Resolved, u64> {
    walk(path, Mode::Rename)
}

/// Resolve `path` to the name a mkdir, symlink, link or mknod creates. A
/// final symlink is not followed, even when the pathname ends in `/`.
pub fn resolve_create(path: &str) -> Result<Resolved, u64> {
    walk(path, Mode::Create)
}

/// Resolve with the caller's chosen credentials (access uses real IDs) and
/// optionally a directory descriptor, using the same component walk.
pub(crate) fn resolve_from(
    path: &str, follow: bool, start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    walk_with(path, Mode::Lookup { follow }, start, cred)
}

fn walk(path: &str, mode: Mode) -> Result<Resolved, u64> {
    let cred = super::permissions::Credentials::current(false);
    walk_with(path, mode, None, &cred)
}

fn walk_with(
    path: &str, mode: Mode, start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    if path.is_empty() {
        return Err(ENOENT as u64);
    }
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    // Both read state no filesystem guard is held across.
    let home_mounted = ext2::is_home_mounted();
    let start = if path.starts_with('/') {
        WorkingDir::Root
    } else {
        start.unwrap_or_else(current_working_dir)
    };
    let mut held = Held(None);
    let mut stack = frames(&start, &mut held)?;
    let mut trailing_slash = path.ends_with('/');
    let follow_final = match mode {
        Mode::Lookup { follow } => follow || trailing_slash,
        Mode::Entry | Mode::Rename | Mode::Create => false,
    };
    let mut pending = Vec::new();
    push_components(&mut pending, path)?;
    let mut last = if path.starts_with('/') {
        Last::Root
    } else {
        Last::Dot
    };
    let mut links = 0u32;
    // Whether the top of the stack is a directory: every component is looked
    // up in, or steps out of, one.
    let mut top_is_dir = true;
    // The final component names nothing below `/dev` or `/proc`.
    let mut virtual_absent = false;

    while let Some(name) = pending.pop() {
        let is_final = pending.is_empty();
        if !top_is_dir {
            return Err(ENOTDIR as u64);
        }
        // Searching any component, including . and .., requires permission
        // on the directory it is looked up in. Keep the inode under its guard.
        let searched_inode = if let Node::Ext2(mount, dir) = top(&stack) {
            let inode = read_dir_inode(held.fs(mount)?, dir)?;
            if !cred.permits(&inode, 1) {
                return Err(crate::syscall::errno::EACCES as u64);
            }
            Some(inode)
        } else { None };
        match name.as_str() {
            "." => {
                last = Last::Dot;
                // The search check already verified this directory.
                continue;
            }
            ".." => {
                last = Last::DotDot;
                stack.pop();
                continue;
            }
            _ => last = Last::Name,
        }
        match top(&stack) {
            Node::Virtual => {
                // devfs and procfs take locks no filesystem guard is held
                // across.
                held.release();
                stack.push(Frame {
                    name,
                    node: Node::Virtual,
                });
                match virtual_entry(&join(&stack)) {
                    Some(is_dir) => top_is_dir = is_dir,
                    None if is_final => {
                        virtual_absent = true;
                        top_is_dir = false;
                    }
                    None => return Err(ENOENT as u64),
                }
            }
            Node::Ext2(mount, dir) => {
                if mount == Mount::Root && dir == EXT2_ROOT_INO {
                    let entered = match name.as_str() {
                        "dev" | "proc" => Some(Node::Virtual),
                        "home" if home_mounted => Some(Node::Ext2(Mount::Home, EXT2_ROOT_INO)),
                        _ => None,
                    };
                    if let Some(node) = entered {
                        stack.push(Frame { name, node });
                        continue;
                    }
                }
                let fs = held.fs(mount)?;
                let dir_inode = searched_inode.ok_or(ENOTDIR as u64)?;
                let found = fs
                    .lookup_in_dir(&dir_inode, &name)
                    .map_err(|_| EIO as u64)?;
                let Some(ino) = found else {
                    if !is_final {
                        return Err(ENOENT as u64);
                    }
                    let dir_pin = || {
                        fs.pin_loaded_inode(dir, dir_inode.size())
                            .map_err(|_| EIO as u64)
                    };
                    let (pin, entry_dir) = match mode {
                        Mode::Lookup { .. } => (Some(dir_pin()?), None),
                        Mode::Rename => (None, Some(dir_pin()?)),
                        Mode::Entry | Mode::Create => (None, None),
                    };
                    let mut absent = join(&stack);
                    if !stack.is_empty() {
                        absent.push('/');
                    }
                    absent.push_str(&name);
                    return Ok(Resolved {
                        path: absent,
                        target: Target::Absent {
                            mount,
                            parent: dir,
                        },
                        last,
                        trailing_slash,
                        virtual_absent,
                        pin,
                        entry_dir,
                    });
                };
                let inode = fs.read_inode(ino).map_err(|_| EIO as u64)?;
                if inode.is_symlink() && (!is_final || follow_final) {
                    links += 1;
                    if links > MAX_SYMLINKS {
                        return Err(ELOOP as u64);
                    }
                    let target = fs.read_symlink(ino).map_err(|_| EIO as u64)?;
                    if target.is_empty() {
                        return Err(ENOENT as u64);
                    }
                    // A final link to `dir/` names a directory, as the
                    // pathname `dir/` would.
                    if is_final && target.ends_with('/') {
                        trailing_slash = true;
                    }
                    if target.starts_with('/') {
                        stack.clear();
                    }
                    push_components(&mut pending, &target)?;
                    // What is left to resolve is held to the pathname limit.
                    if pending.iter().map(|name| name.len() + 1).sum::<usize>() >= PATH_MAX {
                        return Err(ENAMETOOLONG as u64);
                    }
                    if pending.is_empty() {
                        // The link names `/` itself.
                        last = Last::Root;
                    }
                    continue;
                }
                top_is_dir = inode.is_dir();
                stack.push(Frame {
                    name,
                    node: Node::Ext2(mount, ino),
                });
            }
        }
    }

    if trailing_slash && !top_is_dir && !virtual_absent {
        return Err(ENOTDIR as u64);
    }
    let (target, pin, entry_dir) = match top(&stack) {
        Node::Ext2(mount, ino) => {
            let fs = held.fs(mount)?;
            let inode = fs.read_inode(ino).map_err(|_| EIO as u64)?;
            let pin = match mode {
                Mode::Lookup { .. } => Some(
                    fs.pin_loaded_inode(ino, inode.size())
                        .map_err(|_| EIO as u64)?,
                ),
                Mode::Entry | Mode::Rename | Mode::Create => None,
            };
            // The final name's directory is the frame below it, on the same
            // filesystem unless the name is a filesystem root.
            let below = stack.len().checked_sub(2).map_or(ROOT, |i| stack[i].node);
            let entry_dir = match (mode, last, below) {
                (Mode::Rename, Last::Name, Node::Ext2(dir_mount, dir)) if dir_mount == mount => {
                    let dir_inode = fs.read_inode(dir).map_err(|_| EIO as u64)?;
                    Some(
                        fs.pin_loaded_inode(dir, dir_inode.size())
                            .map_err(|_| EIO as u64)?,
                    )
                }
                _ => None,
            };
            let target = Target::Inode {
                mount,
                ino,
                file_type: inode.file_type(),
            };
            (target, pin, entry_dir)
        }
        Node::Virtual => (Target::Virtual, None, None),
    };
    drop(held);
    Ok(Resolved {
        path: join(&stack),
        target,
        last,
        trailing_slash,
        virtual_absent,
        pin,
        entry_dir,
    })
}

/// Resolve `path`, following every symlink, to an existing ext2 inode, as a
/// program loader needs. The handle holds the inode while it is read.
pub fn resolve_file(path: &str) -> Result<(Mount, u32, FileHandle), u64> {
    let resolved = resolve(path, true)?;
    match resolved.target {
        Target::Inode { mount, ino, .. } => resolved
            .pin
            .map(|pin| (mount, ino, pin))
            .ok_or(EIO as u64),
        Target::Absent { .. } => Err(ENOENT as u64),
        Target::Virtual => Err(crate::syscall::errno::EACCES as u64),
    }
}

/// A process's working directory.
#[derive(Clone, Debug)]
pub enum WorkingDir {
    /// `/`, which can be neither renamed nor removed.
    Root,
    /// An ext2 directory, held so that it stays the same directory.
    Ext2 { mount: Mount, dir: FileHandle },
    /// A devfs or procfs directory, named as written.
    Virtual(String),
}

impl Default for WorkingDir {
    fn default() -> Self {
        WorkingDir::Root
    }
}

/// The working directory of the calling process; `/` for a caller with none.
pub fn current_working_dir() -> WorkingDir {
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return WorkingDir::Root;
    };
    let manager_guard = crate::process::manager();
    match &*manager_guard {
        Some(manager) => manager
            .find_process_by_thread(thread_id)
            .map(|(_, p)| p.cwd.clone())
            .unwrap_or_default(),
        None => WorkingDir::Root,
    }
}

/// The working directory a resolved pathname names, for chdir.
pub fn working_dir(resolved: &Resolved) -> Result<WorkingDir, u64> {
    match resolved.target {
        Target::Absent { .. } => Err(ENOENT as u64),
        Target::Virtual => match virtual_entry(&resolved.path) {
            Some(true) => Ok(WorkingDir::Virtual(resolved.path.clone())),
            Some(false) => Err(ENOTDIR as u64),
            None => Err(ENOENT as u64),
        },
        Target::Inode { file_type, .. } if file_type != FileType::Directory => {
            Err(ENOTDIR as u64)
        }
        Target::Inode {
            mount: Mount::Root,
            ino: EXT2_ROOT_INO,
            ..
        } => Ok(WorkingDir::Root),
        Target::Inode { mount, .. } => {
            let dir = resolved.pin.clone().ok_or(ENOENT as u64)?;
            Ok(WorkingDir::Ext2 { mount, dir })
        }
    }
}

/// EACCES unless `cred` may search `dir`, as entering it as the working
/// directory requires. A removed directory is checked by its inode.
pub(crate) fn may_search(
    dir: &WorkingDir,
    cred: &super::permissions::Credentials,
) -> Result<(), u64> {
    let (mount, handle) = match dir {
        WorkingDir::Virtual(_) => return Ok(()),
        WorkingDir::Root => (Mount::Root, None),
        WorkingDir::Ext2 { mount, dir } => (*mount, Some(dir)),
    };
    let mut held = Held(None);
    let fs = held.fs(mount)?;
    let ino = match handle {
        Some(handle) => handle.verify(fs).map_err(|_| ENOENT as u64)?,
        None => EXT2_ROOT_INO,
    };
    let inode = fs.read_inode(ino).map_err(|_| EIO as u64)?;
    if !inode.is_dir() {
        return Err(ENOTDIR as u64);
    }
    if !cred.permits(&inode, 1) {
        return Err(crate::syscall::errno::EACCES as u64);
    }
    Ok(())
}

/// The physical pathname of a working directory, derived from the directory
/// itself. ENOENT when it no longer has a name.
pub fn working_dir_path(dir: &WorkingDir) -> Result<String, u64> {
    let mut held = Held(None);
    let path = join(&frames(dir, &mut held)?);
    drop(held);
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    Ok(path)
}

/// The physical directory stack of a working directory. An ext2 directory's
/// stack is read under `held`, which stays on its filesystem.
fn frames(dir: &WorkingDir, held: &mut Held) -> Result<Vec<Frame>, u64> {
    match dir {
        WorkingDir::Root => Ok(Vec::new()),
        WorkingDir::Virtual(path) => Ok(path
            .split('/')
            .filter(|name| !name.is_empty())
            .map(|name| Frame {
                name: String::from(name),
                node: Node::Virtual,
            })
            .collect()),
        WorkingDir::Ext2 { mount, dir } => {
            let fs = held.fs(*mount)?;
            let ino = dir.verify(fs).map_err(|_| ENOENT as u64)?;
            ancestry(fs, *mount, ino)
        }
    }
}

/// Walk `..` entries from directory `ino` up to its filesystem's root,
/// naming each directory by the entry its parent holds for it. The whole
/// walk is one read of the filesystem, so a concurrent rename cannot leave
/// it with a pathname the directory never had.
fn ancestry(fs: &ext2::Ext2Fs, mount: Mount, mut ino: u32) -> Result<Vec<Frame>, u64> {
    let mut frames = Vec::new();
    while ino != EXT2_ROOT_INO {
        let inode = read_dir_inode(fs, ino)?;
        let data = fs.read_directory(&inode).map_err(|_| EIO as u64)?;
        let parent = ext2::find_entry(&data, "..").ok_or(ENOENT as u64)?.inode;
        let parent_inode = read_dir_inode(fs, parent)?;
        let parent_data = fs
            .read_directory(&parent_inode)
            .map_err(|_| EIO as u64)?;
        // Only an exact name can have been looked up, so a directory is
        // never entered through one that is not.
        let entry = DirReader::new(&parent_data)
            .find(|entry| {
                entry.inode == ino && entry.name_is_exact && !entry.is_dot() && !entry.is_dotdot()
            })
            .ok_or(ENOENT as u64)?;
        frames.push(Frame {
            name: entry.name,
            node: Node::Ext2(mount, ino),
        });
        ino = parent;
    }
    if mount == Mount::Home {
        frames.push(Frame {
            name: String::from("home"),
            node: Node::Ext2(Mount::Home, EXT2_ROOT_INO),
        });
    }
    frames.reverse();
    Ok(frames)
}
