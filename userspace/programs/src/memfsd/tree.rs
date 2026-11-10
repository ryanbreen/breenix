//! In-memory operation dispatcher for memfsd_test, also exercised on the host.
use std::collections::BTreeMap;
use ufs_abi::{payload::{Attr, Reader, Writer, ROOT, S_IFDIR, S_IFREG}, Op, Request, MAX_DATA};

const CAPACITY: usize = 32 * 1024 * 1024;
const NODE_CAP: usize = 4096;
const EINVAL: i32 = 22;
type Result<T> = std::result::Result<T, i32>;

struct Node {
    parent: u64,
    mode: u32,
    linked: bool,
    children: BTreeMap<Vec<u8>, u64>,
    data: Vec<u8>,
}
impl Node {
    fn new(parent: u64, mode: u32) -> Self {
        Self { parent, mode, linked: true, children: BTreeMap::new(), data: Vec::new() }
    }
    fn is_dir(&self) -> bool { self.mode & 0o170000 == S_IFDIR }
}
struct Handle { node: u64, flags: u32 }
pub struct Tree {
    nodes: BTreeMap<u64, Node>,
    handles: BTreeMap<u64, Handle>,
    next_node: u64,
    next_handle: u64,
}
impl Tree {
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(ROOT, Node::new(ROOT, S_IFDIR | 0o755));
        Self { nodes, handles: BTreeMap::new(), next_node: ROOT + 1, next_handle: 1 }
    }
    fn node(&self, id: u64) -> Result<&Node> { self.nodes.get(&id).ok_or(2) }
    fn dir(&self, id: u64) -> Result<&Node> {
        let n = self.node(id)?;
        if n.is_dir() { Ok(n) } else { Err(20) }
    }
    fn lookup(&self, parent: u64, name: &[u8]) -> Result<u64> {
        self.dir(parent)?.children.get(name).copied().ok_or(2)
    }
    fn attr(&self, id: u64) -> Result<Vec<u8>> {
        let node = self.node(id)?;
        let nlink = if !node.linked { 0 } else if node.is_dir() {
            2 + node.children.values().filter(|id| self.nodes[id].is_dir()).count() as u32
        } else { 1 };
        let attr = Attr { node: id, size: node.data.len() as u64, mode: node.mode, nlink,
            uid: 0, gid: 0, atime_ns: 0, mtime_ns: 0 };
        let mut bytes = vec![0; Attr::SIZE];
        attr.encode(&mut bytes).map_err(|_| EINVAL)?;
        Ok(bytes)
    }
    fn used(&self) -> usize { self.nodes.values().map(|n| n.data.len()).sum() }
    fn resize(&mut self, id: u64, size: usize) -> Result<()> {
        let n = self.node(id)?;
        if n.is_dir() { return Err(21); }
        if size.saturating_sub(n.data.len()) > CAPACITY - self.used() { return Err(28); }
        self.nodes.get_mut(&id).ok_or(2)?.data.resize(size, 0);
        Ok(())
    }
    fn handle(&self, handle: u64, node: u64) -> Result<&Handle> {
        self.handles.get(&handle).filter(|h| h.node == node).ok_or(9)
    }
    fn collect(&mut self, node: u64) {
        if self.nodes.get(&node).is_some_and(|n| !n.linked)
            && !self.handles.values().any(|h| h.node == node) { self.nodes.remove(&node); }
    }
    fn detach(&mut self, parent: u64, name: &[u8], id: u64) {
        self.nodes.get_mut(&parent).unwrap().children.remove(name);
        self.nodes.get_mut(&id).unwrap().linked = false;
        self.collect(id);
    }
    pub fn serve(&mut self, req: &Request<'_>) -> Result<Vec<u8>> {
        let mut r = Reader::new(req.payload);
        // Parse and validate the entire payload before changing the tree.
        match req.op {
            Op::Lookup => {
                let name = r.name().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                self.attr(self.lookup(req.node, name)?)
            }
            Op::Getattr => { r.finish().map_err(|_| EINVAL)?; self.attr(req.node) }
            Op::Setattr => {
                let size = r.u64().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                let size = usize::try_from(size).map_err(|_| 27)?;
                self.resize(req.node, size)?;
                self.attr(req.node)
            }
            Op::Create | Op::Mkdir => {
                let mode = r.u32().map_err(|_| EINVAL)? & 0o777;
                let name = r.name().map_err(|_| EINVAL)?.to_vec();
                r.finish().map_err(|_| EINVAL)?;
                if self.dir(req.node)?.children.contains_key(&name) { return Err(17); }
                if self.nodes.len() >= NODE_CAP { return Err(28); }
                let id = self.next_node;
                self.next_node += 1;
                let kind = if req.op == Op::Mkdir { S_IFDIR } else { S_IFREG };
                self.nodes.insert(id, Node::new(req.node, kind | mode));
                self.nodes.get_mut(&req.node).unwrap().children.insert(name, id);
                self.attr(id)
            }
            Op::Open => {
                let flags = r.u32().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                let node = self.node(req.node)?;
                if flags & 3 == 3 { return Err(EINVAL); }
                if node.is_dir() && (flags & 3 != 0 || flags & 0x200 != 0) { return Err(21); }
                if flags & 0x200 != 0 {
                    if flags & 3 == 0 { return Err(13); }
                    self.resize(req.node, 0)?;
                }
                let h = self.next_handle;
                self.next_handle += 1;
                self.handles.insert(h, Handle { node: req.node, flags });
                Ok(h.to_le_bytes().to_vec())
            }
            Op::Read | Op::Write => {
                let handle = r.u64().map_err(|_| EINVAL)?;
                let offset = r.u64().map_err(|_| EINVAL)?;
                let flags = self.handle(handle, req.node)?.flags;
                if self.node(req.node)?.is_dir() { return Err(21); }
                if req.op == Op::Read {
                    let count = r.u32().map_err(|_| EINVAL)? as usize;
                    r.finish().map_err(|_| EINVAL)?;
                    if count > MAX_DATA { return Err(EINVAL); }
                    if flags & 3 == 1 { return Err(9); }
                    let data = &self.node(req.node)?.data;
                    let start = offset.min(data.len() as u64) as usize;
                    Ok(data[start..start + count.min(data.len() - start)].to_vec())
                } else {
                    let bytes = r.rest();
                    if req.payload.len() > MAX_DATA { return Err(EINVAL); }
                    if flags & 3 == 0 { return Err(9); }
                    let offset = usize::try_from(offset).map_err(|_| 27)?;
                    let end = offset.checked_add(bytes.len()).ok_or(27)?;
                    if end > self.node(req.node)?.data.len() { self.resize(req.node, end)?; }
                    self.nodes.get_mut(&req.node).unwrap().data[offset..end].copy_from_slice(bytes);
                    Ok((bytes.len() as u32).to_le_bytes().to_vec())
                }
            }
            Op::Readdir => {
                let cookie = r.u64().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                let mut buf = vec![0; MAX_DATA];
                let mut w = Writer::new(&mut buf);
                for (index, (name, id)) in self.dir(req.node)?.children.iter().enumerate() {
                    if (index as u64) < cookie { continue; }
                    if w.len() + 22 + name.len() > MAX_DATA { break; }
                    w.u64(index as u64 + 1).map_err(|_| EINVAL)?;
                    w.u64(*id).map_err(|_| EINVAL)?;
                    w.u32(self.node(*id)?.mode).map_err(|_| EINVAL)?;
                    w.name(name).map_err(|_| EINVAL)?;
                }
                let used = w.len();
                buf.truncate(used);
                Ok(buf)
            }
            Op::Unlink | Op::Rmdir => {
                let name = r.name().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                let id = self.lookup(req.node, name)?;
                let node = self.node(id)?;
                if req.op == Op::Unlink && node.is_dir() { return Err(21); }
                if req.op == Op::Rmdir {
                    if !node.is_dir() { return Err(20); }
                    if !node.children.is_empty() { return Err(39); }
                }
                self.detach(req.node, name, id);
                Ok(Vec::new())
            }
            Op::Rename => {
                let parent = r.u64().map_err(|_| EINVAL)?;
                let from = r.name().map_err(|_| EINVAL)?;
                let to = r.name().map_err(|_| EINVAL)?.to_vec();
                r.finish().map_err(|_| EINVAL)?;
                let id = self.lookup(req.node, from)?;
                self.dir(parent)?;
                if req.node == parent && from == to { return Ok(Vec::new()); }
                // Refuse moving a directory below itself.
                let mut ancestor = parent;
                loop {
                    if ancestor == id { return Err(EINVAL); }
                    if ancestor == ROOT { break; }
                    ancestor = self.node(ancestor)?.parent;
                }
                if let Some(replaced) = self.dir(parent)?.children.get(&to).copied() {
                    let old = self.node(replaced)?;
                    let new = self.node(id)?;
                    if old.is_dir() && !new.is_dir() { return Err(21); }
                    if !old.is_dir() && new.is_dir() { return Err(20); }
                    if old.is_dir() && !old.children.is_empty() { return Err(39); }
                    self.detach(parent, &to, replaced);
                }
                self.nodes.get_mut(&req.node).unwrap().children.remove(from);
                self.nodes.get_mut(&parent).unwrap().children.insert(to, id);
                self.nodes.get_mut(&id).unwrap().parent = parent;
                Ok(Vec::new())
            }
            Op::Fsync | Op::Release => {
                let handle = r.u64().map_err(|_| EINVAL)?;
                r.finish().map_err(|_| EINVAL)?;
                self.handle(handle, req.node)?;
                if req.op == Op::Release { self.handles.remove(&handle); self.collect(req.node); }
                Ok(Vec::new())
            }
            Op::Statfs => {
                r.finish().map_err(|_| EINVAL)?;
                self.node(req.node)?;
                let mut buf = vec![0; 56];
                let mut w = Writer::new(&mut buf);
                let free = (CAPACITY - self.used()) / 4096;
                for n in [4096, CAPACITY / 4096, free, free, NODE_CAP, NODE_CAP - self.nodes.len(), ufs_abi::NAME_MAX] {
                    w.u64(n as u64).map_err(|_| EINVAL)?;
                }
                Ok(buf)
            }
        }
    }
}
