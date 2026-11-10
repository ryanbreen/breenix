//! Transport-independent SMB namespace and UFS attribute mapping.
use libsmb::msg::{
    access_mask as access, create_options as options, CreateDisposition, DirectoryEntry,
    FileAllInformation, FileFsFullSizeInformation, FileId, SetInfo,
};
use std::{collections::BTreeMap, time::Instant};
mod connection;
mod files;
pub use connection::Connection;
pub use files::{
    Statfs, OPERATION_TIMEOUT, O_CREAT, O_DIRECTORY, O_EXCL, O_RDWR, O_TRUNC, O_WRONLY,
};
pub use ufs_abi::payload::ROOT;
use ufs_abi::payload::{Attr, S_IFDIR, S_IFREG};

/// Backend calls must honor the deadline installed for the whole operation.
pub trait Backend {
    fn set_deadline(&mut self, deadline: Instant) -> Result<(), u32>;
    fn reconnect(&mut self, deadline: Instant) -> Result<(), u32>;
    fn read_at(&mut self, id: FileId, offset: u64, length: usize) -> Result<Vec<u8>, u32>;
    fn write_at(&mut self, id: FileId, offset: u64, data: &[u8]) -> Result<(), u32>;
    fn flush(&mut self, id: FileId) -> Result<(), u32>;
    fn query_fs_size(&mut self, id: FileId) -> Result<FileFsFullSizeInformation, u32>;
    fn open(
        &mut self,
        path: &str,
        access: u32,
        disposition: CreateDisposition,
        options: u32,
    ) -> Result<FileId, u32>;
    fn close(&mut self, id: FileId) -> Result<(), u32>;
    fn query_file(&mut self, id: FileId) -> Result<FileAllInformation, u32>;
    fn list_handle(&mut self, id: FileId) -> Result<Vec<DirectoryEntry>, u32>;
    fn set_info(&mut self, id: FileId, info: SetInfo<'_>) -> Result<(), u32>;
}
fn client_error(e: libsmb::client::Error) -> u32 {
    match e {
        libsmb::client::Error::Status(s) => s.0,
        libsmb::client::Error::Io(_) => files::DISCONNECTED,
        libsmb::client::Error::Protocol => 0xffff_fffe,
    }
}
pub fn errno(status: u32) -> i32 {
    match status {
        0xc000_0008 => 9,
        0xc000_0034 | 0xc000_003a | 0xc000_000f => 2,
        0xc000_0022 | 0xc000_006d => 13,
        0xc000_00a2 => 30,
        0xc000_0035 => 17,
        0xc000_0101 => 39,
        0xc000_0103 => 20,
        0xc000_00ba => 21,
        0xc000_007f => 28,
        0xc000_0043 => 16,
        0xc000_000d | 0xc000_0033 => 22,
        0xc000_0106 => 36,
        _ => 5,
    }
}
pub fn filetime_ns(t: u64) -> u64 {
    t.saturating_sub(116_444_736_000_000_000)
        .saturating_mul(100)
}
pub fn attr(node: u64, i: &FileAllInformation) -> Attr {
    Attr {
        node,
        size: i.end_of_file,
        mode: if i.directory {
            S_IFDIR | 0o755
        } else {
            S_IFREG | 0o644
        },
        nlink: i.links,
        uid: 0,
        gid: 0,
        atime_ns: filetime_ns(i.last_access_time),
        mtime_ns: filetime_ns(i.last_write_time),
    }
}
fn validate(name: &str) -> Result<(), i32> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > ufs_abi::NAME_MAX
        || name.contains(['/', '\\', ':', '\0'])
    {
        Err(22)
    } else {
        Ok(())
    }
}
fn within(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|s| s.starts_with('\\'))
}
struct Node {
    path: String,
    parent: u64,
}
pub struct Namespace<B> {
    backend: B,
    nodes: BTreeMap<u64, Node>,
    paths: BTreeMap<String, u64>,
    cache: BTreeMap<u64, (u64, Attr)>,
    next: u64,
    handles: BTreeMap<u64, files::Handle>,
    next_handle: u64,
    connected: bool,
}
impl<B: Backend> Namespace<B> {
    pub fn new(backend: B) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT,
            Node {
                path: String::new(),
                parent: ROOT,
            },
        );
        let mut paths = BTreeMap::new();
        paths.insert(String::new(), ROOT);
        Self {
            backend,
            nodes,
            paths,
            cache: BTreeMap::new(),
            next: ROOT + 1,
            handles: BTreeMap::new(),
            next_handle: 1,
            connected: true,
        }
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }
    pub fn path(&self, node: u64) -> Result<&str, i32> {
        self.nodes.get(&node).map(|n| n.path.as_str()).ok_or(2)
    }
    pub fn parent(&self, node: u64) -> Result<(u64, String), i32> {
        if node == ROOT {
            return Err(2);
        }
        let n = self.nodes.get(&node).ok_or(2)?;
        let name = n.path.rsplit('\\').next().ok_or(2)?.to_owned();
        Ok((n.parent, name))
    }
    fn child(&self, parent: u64, name: &str) -> Result<String, i32> {
        validate(name)?;
        let p = self.path(parent)?;
        Ok(if p.is_empty() {
            name.to_owned()
        } else {
            format!("{p}\\{name}")
        })
    }
    fn node(&mut self, path: String, parent: u64) -> u64 {
        if let Some(&n) = self.paths.get(&path) {
            return n;
        }
        let n = self.next;
        self.next += 1;
        self.paths.insert(path.clone(), n);
        self.nodes.insert(n, Node { path, parent });
        n
    }
    fn with_handle<T>(
        &mut self,
        path: &str,
        access: u32,
        disp: CreateDisposition,
        opts: u32,
        mut f: impl FnMut(&mut B, FileId) -> Result<T, u32>,
    ) -> Result<T, i32> {
        self.operation(|s| {
            let id = s.backend.open(path, access, disp, opts)?;
            let result = f(&mut s.backend, id);
            if result.as_ref().is_err_and(|e| files::disconnected(*e)) {
                return result;
            }
            let close = s.backend.close(id);
            if let Err(status) = &close {
                if files::disconnected(*status) {
                    return Err(*status);
                }
            }
            match result {
                Ok(v) => close.map(|()| v),
                Err(e) => Err(e),
            }
        })
    }
    pub fn lookup(&mut self, parent: u64, name: &str, now: u64) -> Result<Attr, i32> {
        let path = self.child(parent, name)?;
        let info = self.with_handle(
            &path,
            access::READ_ATTRIBUTES,
            CreateDisposition::Open,
            0,
            |b, h| b.query_file(h),
        )?;
        let n = self.node(path, parent);
        let a = attr(n, &info);
        self.cache.insert(n, (now, a));
        Ok(a)
    }
    pub fn getattr(&mut self, node: u64, now: u64) -> Result<Attr, i32> {
        if let Some(&(at, a)) = self.cache.get(&node) {
            if now.checked_sub(at).is_some_and(|age| age < 1_000_000_000) {
                return Ok(a);
            }
        }
        let path = self.path(node)?.to_owned();
        let info = self.with_handle(
            &path,
            access::READ_ATTRIBUTES,
            CreateDisposition::Open,
            0,
            |b, h| b.query_file(h),
        )?;
        let a = attr(node, &info);
        self.cache.insert(node, (now, a));
        Ok(a)
    }
    pub fn readdir(&mut self, node: u64) -> Result<Vec<DirectoryEntry>, i32> {
        let path = self.path(node)?.to_owned();
        self.with_handle(
            &path,
            access::LIST_DIRECTORY,
            CreateDisposition::Open,
            options::DIRECTORY_FILE,
            |b, h| b.list_handle(h),
        )
    }
    pub fn mkdir(&mut self, parent: u64, name: &str, now: u64) -> Result<Attr, i32> {
        let path = self.child(parent, name)?;
        self.invalidate();
        let info = self.with_handle(
            &path,
            access::READ_ATTRIBUTES,
            CreateDisposition::Create,
            options::DIRECTORY_FILE,
            |b, h| b.query_file(h),
        )?;
        let n = self.node(path, parent);
        let a = attr(n, &info);
        self.cache.insert(n, (now, a));
        Ok(a)
    }
    fn remove(&mut self, parent: u64, name: &str, dir: bool) -> Result<(), i32> {
        let path = self.child(parent, name)?;
        self.invalidate();
        self.with_handle(
            &path,
            access::DELETE,
            CreateDisposition::Open,
            if dir {
                options::DIRECTORY_FILE
            } else {
                options::NON_DIRECTORY_FILE
            },
            |b, h| b.set_info(h, SetInfo::Disposition(true)),
        )?;
        self.forget(&path);
        Ok(())
    }
    fn forget(&mut self, path: &str) {
        for handle in self.handles.values_mut() {
            if handle.path.as_ref().is_some_and(|p| within(p, path)) {
                handle.path = None;
            }
        }
        let dead: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, n)| within(&n.path, path))
            .map(|(&id, n)| (id, n.path.clone()))
            .collect();
        for (id, p) in dead {
            self.nodes.remove(&id);
            self.paths.remove(&p);
            self.cache.remove(&id);
        }
    }
    pub fn unlink(&mut self, p: u64, n: &str) -> Result<(), i32> {
        self.remove(p, n, false)
    }
    pub fn rmdir(&mut self, p: u64, n: &str) -> Result<(), i32> {
        self.remove(p, n, true)
    }
    pub fn rename(&mut self, from: u64, name: &str, to: u64, new: &str) -> Result<(), i32> {
        let old = self.child(from, name)?;
        let dest = self.child(to, new)?;
        if old == dest {
            return Ok(());
        }
        self.invalidate();
        self.with_handle(&old, access::DELETE, CreateDisposition::Open, 0, |b, h| {
            b.set_info(
                h,
                SetInfo::Rename {
                    replace_if_exists: true,
                    name: &dest,
                },
            )
        })?;
        self.forget(&dest);
        for handle in self.handles.values_mut() {
            if let Some(path) = &mut handle.path {
                if within(path, &old) {
                    *path = format!("{}{}", dest, &path[old.len()..]);
                }
            }
        }
        let changes: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, n)| within(&n.path, &old))
            .map(|(&id, n)| (id, n.path.clone()))
            .collect();
        for (id, p) in changes {
            self.paths.remove(&p);
            let n = self.nodes.get_mut(&id).unwrap();
            n.path = format!("{}{}", dest, &p[old.len()..]);
            if p == old {
                n.parent = to;
            }
            self.paths.insert(n.path.clone(), id);
        }
        Ok(())
    }
}
