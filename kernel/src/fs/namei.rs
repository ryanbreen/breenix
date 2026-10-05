//! Pathname resolution.
//!
//! Every pathname-taking syscall resolves its pathname here, once, and acts on
//! the result. The walk keeps the stack of physical directories it has entered:
//! a symlink component is expanded in place before any later component is
//! applied, so a `..` after it leaves the directory the link names, not the one
//! the text names. The pathname a resolution reports is that physical stack,
//! with no symlink, `.` or `..` component.
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
//! they have no symlinks, so below their mount points names are taken as
//! written.

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
    /// The pathname ended in `/`, so its final component must be a directory.
    pub trailing_slash: bool,
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

/// One step of a walk: look `name` up in the ext2 directory `dir`.
fn lookup(mount: Mount, dir: u32, name: &str) -> Result<Option<(u32, ext2::Ext2Inode)>, u64> {
    let guard = mount.read();
    let fs = guard.as_ref().ok_or(ENOENT as u64)?;
    let dir_inode = read_dir_inode(fs, dir)?;
    match fs.lookup_in_dir(&dir_inode, name).map_err(|_| EIO as u64)? {
        Some(ino) => Ok(Some((ino, fs.read_inode(ino).map_err(|_| EIO as u64)?))),
        None => Ok(None),
    }
}

fn read_link(mount: Mount, ino: u32) -> Result<String, u64> {
    let guard = mount.read();
    let fs = guard.as_ref().ok_or(ENOENT as u64)?;
    fs.read_symlink(ino).map_err(|_| EIO as u64)
}

/// Resolve `path` from the calling process's working directory (or `/` when
/// the caller has no process). `follow` says whether a final symlink is
/// followed; symlinks in every earlier component always are, and so is a
/// final one when the pathname ends in `/`.
pub fn resolve(path: &str, follow: bool) -> Result<Resolved, u64> {
    if path.is_empty() {
        return Err(ENOENT as u64);
    }
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    let mut stack = if path.starts_with('/') {
        Vec::new()
    } else {
        frames(&current_working_dir())?
    };
    let trailing_slash = path.ends_with('/');
    let follow_final = follow || trailing_slash;
    let mut pending = Vec::new();
    push_components(&mut pending, path)?;
    let mut last = if path.starts_with('/') {
        Last::Root
    } else {
        Last::Dot
    };
    let mut links = 0u32;
    let mut final_type = FileType::Directory;

    while let Some(name) = pending.pop() {
        let is_final = pending.is_empty();
        match name.as_str() {
            "." => {
                last = Last::Dot;
                final_type = FileType::Directory;
                // `.` names the directory itself, which must still be one.
                if let Node::Ext2(mount, dir) = top(&stack) {
                    let guard = mount.read();
                    read_dir_inode(guard.as_ref().ok_or(ENOENT as u64)?, dir)?;
                }
                continue;
            }
            ".." => {
                last = Last::DotDot;
                final_type = FileType::Directory;
                stack.pop();
                continue;
            }
            _ => last = Last::Name,
        }
        match top(&stack) {
            Node::Virtual => {
                stack.push(Frame {
                    name,
                    node: Node::Virtual,
                });
                final_type = FileType::Unknown;
            }
            Node::Ext2(mount, dir) => {
                if mount == Mount::Root && dir == EXT2_ROOT_INO {
                    let entered = match name.as_str() {
                        "dev" | "proc" => Some(Node::Virtual),
                        "home" if ext2::is_home_mounted() => {
                            Some(Node::Ext2(Mount::Home, EXT2_ROOT_INO))
                        }
                        _ => None,
                    };
                    if let Some(node) = entered {
                        final_type = match node {
                            Node::Virtual => FileType::Unknown,
                            Node::Ext2(..) => FileType::Directory,
                        };
                        stack.push(Frame { name, node });
                        continue;
                    }
                }
                let Some((ino, inode)) = lookup(mount, dir, &name)? else {
                    if !is_final {
                        return Err(ENOENT as u64);
                    }
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
                    });
                };
                if inode.is_symlink() && (!is_final || follow_final) {
                    links += 1;
                    if links > MAX_SYMLINKS {
                        return Err(ELOOP as u64);
                    }
                    let target = read_link(mount, ino)?;
                    if target.is_empty() {
                        return Err(ENOENT as u64);
                    }
                    if target.starts_with('/') {
                        stack.clear();
                    }
                    push_components(&mut pending, &target)?;
                    if pending.is_empty() {
                        // The link names `/` itself.
                        last = Last::Root;
                        final_type = FileType::Directory;
                    }
                    continue;
                }
                final_type = inode.file_type();
                stack.push(Frame {
                    name,
                    node: Node::Ext2(mount, ino),
                });
            }
        }
    }

    let target = match top(&stack) {
        Node::Ext2(mount, ino) => {
            if trailing_slash && final_type != FileType::Directory {
                return Err(ENOTDIR as u64);
            }
            Target::Inode {
                mount,
                ino,
                file_type: final_type,
            }
        }
        Node::Virtual => Target::Virtual,
    };
    Ok(Resolved {
        path: join(&stack),
        target,
        last,
        trailing_slash,
    })
}

/// Resolve `path`, following every symlink, to an existing ext2 inode, as a
/// program loader needs.
pub fn resolve_file(path: &str) -> Result<(Mount, u32), u64> {
    match resolve(path, true)?.target {
        Target::Inode { mount, ino, .. } => Ok((mount, ino)),
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
        Target::Virtual => {
            let path = resolved.path.as_str();
            if path == "/dev" || path == "/proc" {
                Ok(WorkingDir::Virtual(String::from(path)))
            } else if path.strip_prefix("/dev/").is_some_and(|name| crate::fs::devfs::lookup(name).is_some())
                || crate::fs::procfs::lookup_by_path(path).is_some()
            {
                Err(ENOTDIR as u64)
            } else {
                Err(ENOENT as u64)
            }
        }
        Target::Inode { mount, ino, .. } => {
            if mount == Mount::Root && ino == EXT2_ROOT_INO {
                return Ok(WorkingDir::Root);
            }
            let guard = mount.read();
            let fs = guard.as_ref().ok_or(ENOENT as u64)?;
            let inode = read_dir_inode(fs, ino)?;
            let dir = fs
                .pin_loaded_inode(ino, inode.size())
                .map_err(|_| EIO as u64)?;
            Ok(WorkingDir::Ext2 { mount, dir })
        }
    }
}

/// The physical pathname of a working directory, derived from the directory
/// itself. ENOENT when it no longer has a name.
pub fn working_dir_path(dir: &WorkingDir) -> Result<String, u64> {
    let path = join(&frames(dir)?);
    if path.len() >= PATH_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    Ok(path)
}

/// The physical directory stack of a working directory.
fn frames(dir: &WorkingDir) -> Result<Vec<Frame>, u64> {
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
        WorkingDir::Ext2 { mount, dir } => ancestry(*mount, dir),
    }
}

/// Walk `..` entries from a held directory up to its filesystem's root,
/// naming each directory by the entry its parent holds for it.
fn ancestry(mount: Mount, dir: &FileHandle) -> Result<Vec<Frame>, u64> {
    let mut frames = Vec::new();
    {
        let guard = mount.read();
        let fs = guard.as_ref().ok_or(ENOENT as u64)?;
        let mut ino = dir.verify(fs).map_err(|_| ENOENT as u64)?;
        let mut length = 0usize;
        while ino != EXT2_ROOT_INO {
            let inode = read_dir_inode(fs, ino)?;
            let data = fs.read_directory(&inode).map_err(|_| EIO as u64)?;
            let parent = ext2::find_entry(&data, "..").ok_or(ENOENT as u64)?.inode;
            let parent_inode = read_dir_inode(fs, parent)?;
            let parent_data = fs
                .read_directory(&parent_inode)
                .map_err(|_| EIO as u64)?;
            let entry = DirReader::new(&parent_data)
                .find(|entry| entry.inode == ino && !entry.is_dot() && !entry.is_dotdot())
                .ok_or(ENOENT as u64)?;
            length += entry.name.len() + 1;
            if length >= PATH_MAX {
                return Err(ENAMETOOLONG as u64);
            }
            frames.push(Frame {
                name: entry.name,
                node: Node::Ext2(mount, ino),
            });
            ino = parent;
        }
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
