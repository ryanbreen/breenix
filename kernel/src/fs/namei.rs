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
//! A process's root directory is `/` unless it has called chroot: then an
//! absolute pathname, and an absolute symlink target, start from that
//! directory, and `..` there stays there. The root is held as the working
//! directory is, and travels with it: `CLONE_FS` threads share both, fork
//! copies both. Until some process changes its root, every root is `/` and a
//! walk does not look it up.
//!
//! The ext2 root filesystem holds `/`. The home filesystem is entered at
//! `/home` when mounted. devfs and procfs are entered at `/dev` and `/proc`;
//! they have no symlinks, and each name below their mount points is checked
//! against the entries they hold.

use crate::fs::ext2::{self, live_inode::FileHandle, FileType, EXT2_ROOT_INO};
use crate::syscall::errno::{EIO, ELOOP, ENAMETOOLONG, ENOENT, ENOTDIR};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Set by the first chroot and never cleared: until then every process's
/// root directory is `/`, and a walk does not look it up.
static ROOT_CHANGED: AtomicBool = AtomicBool::new(false);

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
    pub fn read(self) -> Result<ext2::Ext2ReadGuard, u64> {
        match self {
            Mount::Root => ext2::root_fs_read(),
            Mount::Home => ext2::home_fs_read(),
        }.map_err(|_| crate::syscall::errno::EINTR as u64)
    }

    pub fn write(self) -> Result<ext2::Ext2WriteGuard, u64> {
        match self {
            Mount::Root => ext2::root_fs_write(),
            Mount::Home => ext2::home_fs_write(),
        }.map_err(|_| crate::syscall::errno::EINTR as u64)
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
pub struct Resolved<P = String> {
    /// The physical absolute pathname of the target: no symlink, `.` or `..`
    /// component, except a final symlink the resolution did not follow.
    pub path: P,
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

    /// The final component of the physical pathname.
    pub fn final_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or("")
    }
}

impl<P> Resolved<P> {
    /// The handle a lookup holds on an existing ext2 target.
    pub fn handle(&self) -> Option<&FileHandle> {
        match self.target {
            Target::Inode { .. } => self.pin.as_ref(),
            _ => None,
        }
    }

    /// The parent held by a lookup of an absent final name. This is distinct
    /// from handle(), whose callers require an existing target inode.
    pub fn lookup_parent(&self) -> Option<&FileHandle> {
        match self.target {
            Target::Absent { .. } => self.pin.as_ref(),
            _ => None,
        }
    }

    /// The directory a rename resolution holds for its final name.
    pub fn entry_dir(&self) -> Option<&FileHandle> {
        self.entry_dir.as_ref()
    }

}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Node {
    Ext2(Mount, u32),
    Virtual,
}

struct Frame {
    name: Vec<u8>,
    node: Node,
}

const ROOT: Node = Node::Ext2(Mount::Root, EXT2_ROOT_INO);

fn top(stack: &[Frame]) -> Node {
    stack.last().map_or(ROOT, |frame| frame.node)
}

/// Whether `stack` is the directory `root` names.
fn is_root(stack: &[Frame], root: &[Frame]) -> bool {
    stack.len() == root.len()
        && stack.iter().zip(root).all(|(a, b)| a.node == b.node && a.name == b.name)
}

fn copy_frames(frames: &[Frame]) -> Result<Vec<Frame>, u64> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(frames.len()).map_err(|_| 12u64)?;
    for frame in frames {
        copy.push(Frame { name: copy_bytes(&frame.name)?, node: frame.node });
    }
    Ok(copy)
}

pub(crate) fn fs_errno(error: &'static str) -> u64 {
    if error.contains("memory") {
        12
    } else {
        EIO as u64
    }
}
fn copy_string(text: &str) -> Result<String, u64> {
    let mut copy = String::new();
    copy.try_reserve_exact(text.len()).map_err(|_| 12u64)?;
    copy.push_str(text);
    Ok(copy)
}
fn copy_bytes(bytes: &[u8]) -> Result<Vec<u8>, u64> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len()).map_err(|_| 12u64)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}
fn join(stack: &[Frame]) -> Result<Vec<u8>, u64> {
    let length = stack.iter().fold(0usize, |size, frame| size.saturating_add(frame.name.len() + 1)).max(1);
    if length >= PATH_MAX { return Err(ENAMETOOLONG as u64); }
    let mut path = Vec::new();
    path.try_reserve_exact(length).map_err(|_| 12u64)?;
    if stack.is_empty() { path.push(b'/'); }
    for frame in stack { path.push(b'/'); path.extend_from_slice(&frame.name); }
    Ok(path)
}
fn push_components(pending: &mut Vec<Vec<u8>>, path: &[u8]) -> Result<(), u64> {
    let start = pending.len();
    for name in path.split(|b| *b == b'/').filter(|name| !name.is_empty()) {
        if name.len() > NAME_MAX { return Err(ENAMETOOLONG as u64); }
        pending.try_reserve(1).map_err(|_| 12u64)?;
        pending.push(copy_bytes(name)?);
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
            self.0 = Some((mount, mount.read()?));
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
    let inode = fs.read_inode(ino).map_err(fs_errno)?;
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
    if let Some(entry) = crate::ipc::mqueue::dev::virtual_entry(path) {
        return entry;
    }
    if let Some(entry) = crate::ipc::shmem::virtual_entry(path) {
        return entry;
    }
    if path == "/dev"
        || path == "/proc"
        || path == "/dev/pts"
        || path == "/dev/shm"
        || path == "/dev/mqueue"
    {
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
    path: &str,
    follow: bool,
    start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    walk_with(path, Mode::Lookup { follow }, start, cred)
}

pub(crate) fn resolve_create_from(
    path: &str,
    start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    walk_with(path, Mode::Create, start, cred)
}
pub(crate) fn resolve_entry_from(
    path: &str,
    start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    walk_with(path, Mode::Entry, start, cred)
}

pub(crate) fn resolve_rename_from(
    path: &str,
    start: Option<WorkingDir>,
    cred: &super::permissions::Credentials,
) -> Result<Resolved, u64> {
    walk_with(path, Mode::Rename, start, cred)
}

fn walk(path: &str, mode: Mode) -> Result<Resolved, u64> {
    let cred = super::permissions::Credentials::current(false);
    walk_with(path, mode, None, &cred)
}

fn walk_with(path: &str, mode: Mode, start: Option<WorkingDir>, cred: &super::permissions::Credentials) -> Result<Resolved, u64> {
    let raw = walk_bytes(path.as_bytes(), mode, start, cred)?;
    Ok(Resolved { path: String::from_utf8(raw.path).map_err(|_| EIO as u64)?, target: raw.target, last: raw.last,
        trailing_slash: raw.trailing_slash, virtual_absent: raw.virtual_absent, pin: raw.pin, entry_dir: raw.entry_dir })
}
/// Byte-preserving path resolution for AF_UNIX, including non-UTF-8 components.
/// Creation holds the parent, lookup holds the target, exactly as the text API.
pub(crate) fn resolve_socket_path(path: &[u8], create: bool, cred: &super::permissions::Credentials) -> Result<Resolved<Vec<u8>>, u64> {
    walk_bytes(path, if create { Mode::Create } else { Mode::Lookup { follow: true } }, None, cred)
}
fn walk_bytes(path: &[u8], mode: Mode, start: Option<WorkingDir>, cred: &super::permissions::Credentials) -> Result<Resolved<Vec<u8>>, u64> {
    if path.is_empty() {
        return Err(ENOENT as u64);
    }
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    // Both read state no filesystem guard is held across.
    let home_mounted = ext2::is_home_mounted();
    let absolute = path.starts_with(b"/");
    let (cwd, root) = if ROOT_CHANGED.load(Ordering::Acquire) {
        try_current_dirs()?
    } else if absolute || start.is_some() {
        (WorkingDir::Root, WorkingDir::Root)
    } else {
        (try_current_working_dir()?, WorkingDir::Root)
    };
    let mut held = Held(None);
    // The directory `/` names, which `..` does not leave.
    let root = frames(&root, &mut held)?;
    let mut stack = if absolute {
        copy_frames(&root)?
    } else {
        frames(&start.unwrap_or(cwd), &mut held)?
    };
    let mut trailing_slash = path.ends_with(b"/");
    let follow_final = match mode {
        Mode::Lookup { follow } => follow || trailing_slash,
        Mode::Entry | Mode::Rename | Mode::Create => false,
    };
    let mut pending = Vec::new();
    push_components(&mut pending, path)?;
    let mut last = if path.starts_with(b"/") {
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
        } else {
            None
        };
        match name.as_slice() {
            b"." => {
                last = Last::Dot;
                // The search check already verified this directory.
                continue;
            }
            b".." => {
                last = Last::DotDot;
                if !is_root(&stack, &root) {
                    stack.pop();
                }
                continue;
            }
            _ => last = Last::Name,
        }
        match top(&stack) {
            Node::Virtual => {
                // devfs and procfs take locks no filesystem guard is held
                // across.
                held.release();
                stack.try_reserve(1).map_err(|_| 12u64)?;
                stack.push(Frame {
                    name,
                    node: Node::Virtual,
                });
                match core::str::from_utf8(&join(&stack)?).ok().and_then(virtual_entry) {
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
                    let entered = match name.as_slice() {
                        b"dev" | b"proc" => Some(Node::Virtual),
                        b"home" if home_mounted => Some(Node::Ext2(Mount::Home, EXT2_ROOT_INO)),
                        _ => None,
                    };
                    if let Some(node) = entered {
                        stack.try_reserve(1).map_err(|_| 12u64)?;
                        stack.push(Frame { name, node });
                        continue;
                    }
                }
                let fs = held.fs(mount)?;
                let dir_inode = searched_inode.ok_or(ENOTDIR as u64)?;
                let found = fs.lookup_in_dir_bytes(&dir_inode, &name).map_err(fs_errno)?;
                let Some(ino) = found else {
                    if !is_final {
                        return Err(ENOENT as u64);
                    }
                    let dir_pin = || fs.pin_loaded_inode(dir, dir_inode.size()).map_err(fs_errno);
                    let (pin, entry_dir) = match mode {
                        Mode::Lookup { .. } => (Some(dir_pin()?), None),
                        Mode::Rename => (None, Some(dir_pin()?)),
                        Mode::Create => (None, Some(dir_pin()?)),
                        Mode::Entry => (None, Some(dir_pin()?)),
                    };
                    let mut absent = join(&stack)?;
                    absent.try_reserve(name.len() + 1).map_err(|_| 12u64)?;
                    if !stack.is_empty() {
                        absent.push(b'/');
                    }
                    absent.extend_from_slice(&name);
                    return Ok(Resolved {
                        path: absent,
                        target: Target::Absent { mount, parent: dir },
                        last,
                        trailing_slash,
                        virtual_absent,
                        pin,
                        entry_dir,
                    });
                };
                let inode = fs.read_inode(ino).map_err(fs_errno)?;
                if inode.is_symlink() && (!is_final || follow_final) {
                    links += 1;
                    if links > MAX_SYMLINKS {
                        return Err(ELOOP as u64);
                    }
                    let target = fs.read_symlink_bytes(ino).map_err(fs_errno)?;
                    if target.is_empty() {
                        return Err(ENOENT as u64);
                    }
                    // A final link to `dir/` names a directory, as the
                    // pathname `dir/` would.
                    if is_final && target.ends_with(b"/") {
                        trailing_slash = true;
                    }
                    if target.starts_with(b"/") {
                        stack = copy_frames(&root)?;
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
                stack.try_reserve(1).map_err(|_| 12u64)?;
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
            let inode = fs.read_inode(ino).map_err(fs_errno)?;
            let pin = match mode {
                Mode::Lookup { .. } => {
                    Some(fs.pin_loaded_inode(ino, inode.size()).map_err(fs_errno)?)
                }
                Mode::Entry | Mode::Rename | Mode::Create => None,
            };
            // The final name's directory is the frame below it, on the same
            // filesystem unless the name is a filesystem root.
            let below = stack.len().checked_sub(2).map_or(ROOT, |i| stack[i].node);
            let entry_dir = match (mode, last, below) {
                (Mode::Rename | Mode::Entry, Last::Name, Node::Ext2(dir_mount, dir))
                    if dir_mount == mount =>
                {
                    let dir_inode = fs.read_inode(dir).map_err(fs_errno)?;
                    Some(
                        fs.pin_loaded_inode(dir, dir_inode.size())
                            .map_err(fs_errno)?,
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
        path: join(&stack)?,
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
        Target::Inode { mount, ino, .. } => {
            resolved.pin.map(|pin| (mount, ino, pin)).ok_or(EIO as u64)
        }
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

/// A process's working directory and its root directory.
#[derive(Debug, Default)]
struct Dirs {
    cwd: WorkingDir,
    root: WorkingDir,
}

/// The working and root directories a process holds. Threads created with
/// `CLONE_FS` share them, so a chdir, fchdir or chroot in any of them moves
/// all of them; fork, spawn and a clone without `CLONE_FS` start from a copy.
/// Its lock is only taken under the process manager's, and nothing else is
/// taken under it. An exited row holds none (`release`) and reads as `/`.
#[derive(Debug)]
pub struct SharedWorkingDir(Option<Arc<spin::Mutex<Dirs>>>);

impl Default for SharedWorkingDir {
    fn default() -> Self {
        Self(Some(Arc::new(spin::Mutex::new(Dirs::default()))))
    }
}

impl SharedWorkingDir {
    /// The same working directory, for a thread created with `CLONE_FS`.
    pub fn share(&self) -> Self {
        Self(self.0.clone())
    }

    /// Whether `other` is this same working directory, shared by `CLONE_FS`.
    pub fn same_as(&self, other: &Self) -> bool {
        matches!((&self.0, &other.0), (Some(a), Some(b)) if Arc::ptr_eq(a, b))
    }

    /// Separate working and root directories that start where these are.
    pub fn copy(&self) -> Self {
        let dirs = self.0.as_ref().map_or_else(Dirs::default, |shared| {
            let dirs = shared.lock();
            Dirs { cwd: dirs.cwd.clone(), root: dirs.root.clone() }
        });
        Self(Some(Arc::new(spin::Mutex::new(dirs))))
    }

    pub fn get(&self) -> WorkingDir {
        self.0.as_ref().map_or(WorkingDir::Root, |dirs| dirs.lock().cwd.clone())
    }

    /// Make `dir` the working directory. The one it replaces is returned so
    /// the caller can release it after the process manager; a row that has
    /// released its working directory gets `dir` back.
    pub fn replace(&self, dir: WorkingDir) -> WorkingDir {
        match &self.0 {
            Some(shared) => core::mem::replace(&mut shared.lock().cwd, dir),
            None => dir,
        }
    }

    /// Make `dir` the root directory, returning the one it replaces as
    /// `replace` does.
    pub fn replace_root(&self, dir: WorkingDir) -> WorkingDir {
        match &self.0 {
            Some(shared) => core::mem::replace(&mut shared.lock().root, dir),
            None => dir,
        }
    }

    /// Give up this row's share of its working directory, for an exiting
    /// row. The share is returned so the caller can drop it after the
    /// process manager; a `CLONE_FS` thread still running keeps its own.
    pub fn release(&mut self) -> Self {
        Self(self.0.take())
    }
}

fn copy_dir(dir: &WorkingDir) -> Result<WorkingDir, u64> {
    Ok(match dir {
        WorkingDir::Root => WorkingDir::Root,
        WorkingDir::Ext2 { mount, dir } => WorkingDir::Ext2 {
            mount: *mount,
            dir: dir.clone(),
        },
        WorkingDir::Virtual(path) => WorkingDir::Virtual(copy_string(path)?),
    })
}

/// The calling process's working and root directories; `/` for both when
/// the caller has no process.
fn try_current_dirs() -> Result<(WorkingDir, WorkingDir), u64> {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return Ok((WorkingDir::Root, WorkingDir::Root));
    };
    let guard = crate::process::manager();
    let Some((_, process)) = guard.as_ref().and_then(|m| m.find_process_by_thread(tid)) else {
        return Ok((WorkingDir::Root, WorkingDir::Root));
    };
    let Some(shared) = process.cwd.0.as_ref() else {
        return Ok((WorkingDir::Root, WorkingDir::Root));
    };
    let dirs = shared.lock();
    Ok((copy_dir(&dirs.cwd)?, copy_dir(&dirs.root)?))
}

fn try_current_working_dir() -> Result<WorkingDir, u64> {
    try_current_dirs().map(|(cwd, _)| cwd)
}

/// Make `dir` the calling process's root directory, as chroot does once it
/// holds the directory and has checked the caller may enter it. The threads
/// that share the working directory share the new root.
pub fn set_root(dir: WorkingDir) -> Result<(), u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(3u64)?; // ESRCH
    // Set first, so a walk that can see the new root also looks it up.
    ROOT_CHANGED.store(true, Ordering::Release);
    let previous = {
        let guard = crate::process::manager();
        let (_, process) = guard
            .as_ref()
            .and_then(|m| m.find_process_by_thread(tid))
            .ok_or(3u64)?;
        process.cwd.replace_root(dir)
    };
    // Released after the process manager, as chdir releases its old directory.
    drop(previous);
    Ok(())
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
            .map(|(_, p)| p.cwd.get())
            .unwrap_or_default(),
        None => WorkingDir::Root,
    }
}

/// The working directory a resolved pathname names, for chdir.
pub fn working_dir(resolved: &Resolved) -> Result<WorkingDir, u64> {
    match resolved.target {
        Target::Absent { .. } => Err(ENOENT as u64),
        Target::Virtual => match virtual_entry(&resolved.path) {
            Some(true) => Ok(WorkingDir::Virtual(copy_string(&resolved.path)?)),
            Some(false) => Err(ENOTDIR as u64),
            None => Err(ENOENT as u64),
        },
        Target::Inode { file_type, .. } if file_type != FileType::Directory => Err(ENOTDIR as u64),
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
    let inode = fs.read_inode(ino).map_err(fs_errno)?;
    if !inode.is_dir() {
        return Err(ENOTDIR as u64);
    }
    if !cred.permits(&inode, 1) {
        return Err(crate::syscall::errno::EACCES as u64);
    }
    Ok(())
}

/// The physical pathname of a working directory, derived from the directory
/// itself, as the calling process names it from its root directory. A
/// directory outside that root is reported, as Linux does, with the prefix
/// `(unreachable)`. ENOENT when it no longer has a name.
pub fn working_dir_path(dir: &WorkingDir) -> Result<String, u64> {
    let root = if ROOT_CHANGED.load(Ordering::Acquire) {
        try_current_dirs()?.1
    } else {
        WorkingDir::Root
    };
    let mut held = Held(None);
    let root = frames(&root, &mut held)?;
    let stack = frames(dir, &mut held)?;
    let path = if stack.len() >= root.len() && is_root(&stack[..root.len()], &root) {
        join(&stack[root.len()..])?
    } else {
        const UNREACHABLE: &[u8] = b"(unreachable)";
        let path = join(&stack)?;
        let mut prefixed = Vec::new();
        prefixed.try_reserve_exact(UNREACHABLE.len() + path.len()).map_err(|_| 12u64)?;
        prefixed.extend_from_slice(UNREACHABLE);
        prefixed.extend_from_slice(&path);
        prefixed
    };
    drop(held);
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    String::from_utf8(path).map_err(|_| EIO as u64)
}

/// The physical directory stack of a working directory. An ext2 directory's
/// stack is read under `held`, which stays on its filesystem.
fn frames(dir: &WorkingDir, held: &mut Held) -> Result<Vec<Frame>, u64> {
    match dir {
        WorkingDir::Root => Ok(Vec::new()),
        WorkingDir::Virtual(path) => {
            let mut frames = Vec::new();
            for name in path.split('/').filter(|name| !name.is_empty()) {
                frames.try_reserve(1).map_err(|_| 12u64)?;
                frames.push(Frame {
                    name: copy_bytes(name.as_bytes())?,
                    node: Node::Virtual,
                });
            }
            Ok(frames)
        }
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
        let data = fs.read_directory(&inode).map_err(fs_errno)?;
        let parent = ext2::dir::find_inode(&data, "..").ok_or(ENOENT as u64)?;
        let parent_inode = read_dir_inode(fs, parent)?;
        let parent_data = fs.read_directory(&parent_inode).map_err(fs_errno)?;
        // Only an exact name can have been looked up, so a directory is
        // never entered through one that is not.
        let entry = ext2::dir::raw_entries(&parent_data)
            .find(|entry| {
                entry.inode == ino
                    && entry.name != b"."
                    && entry.name != b".."

            })
            .ok_or(ENOENT as u64)?;
        frames.try_reserve(1).map_err(|_| 12u64)?;
        frames.push(Frame {
            name: copy_bytes(entry.name)?,
            node: Node::Ext2(mount, ino),
        });
        ino = parent;
    }
    if mount == Mount::Home {
        frames.try_reserve(1).map_err(|_| 12u64)?;
        frames.push(Frame {
            name: copy_bytes(b"home")?,
            node: Node::Ext2(Mount::Home, EXT2_ROOT_INO),
        });
    }
    frames.reverse();
    Ok(frames)
}
