//! Mount identity and traversal, independent of the kernel's locking and backends.
use crate::fs::{attrs::MountFlags, errno::Errno};
use crate::syscall::errno::{EBUSY, EINVAL, ENOENT, ENOMEM, ENOSPC};
use alloc::{sync::Arc, vec::Vec};

pub type InodeKey = (usize, u64);

pub struct Entry<B> {
    pub id: usize,
    pub path: Vec<u8>,
    pub kind: &'static str,
    pub flags: MountFlags,
    pub backend: B,
    pub root: u64,
    pub cover: Option<InodeKey>,
    pub parent: Option<InodeKey>,
    pub bind: Option<InodeKey>,
    pub source: Vec<u8>,
    /// Namespace custody is released after removal, outside the table lock.
    pub custody: Option<Arc<dyn Send + Sync>>,
    references: Arc<()>,
}

impl<B> Entry<B> {
    pub fn new(
        id: usize,
        path: Vec<u8>,
        kind: &'static str,
        flags: MountFlags,
        backend: B,
        root: u64,
        cover: Option<InodeKey>,
        parent: Option<InodeKey>,
    ) -> Self {
        Self {
            id,
            path,
            kind,
            flags,
            backend,
            root,
            cover,
            parent,
            bind: None,
            source: Vec::new(),
            custody: None,
            references: Arc::new(()),
        }
    }
}

pub struct MountTable<B> {
    entries: Vec<Entry<B>>,
    next_id: usize,
}

impl<B> MountTable<B> {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 0,
        }
    }

    pub fn allocate_id(&mut self) -> Result<usize, Errno> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(Errno(ENOSPC))?;
        Ok(id)
    }

    pub fn insert(&mut self, entry: Entry<B>) -> Result<(), Errno> {
        if self.entries.iter().any(|m| {
            m.id == entry.id
                || m.path == entry.path
                || entry.cover.is_some() && m.cover == entry.cover
        }) {
            return Err(Errno(EBUSY));
        }
        self.entries.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        self.next_id = self.next_id.max(entry.id.checked_add(1).ok_or(Errno(ENOSPC))?);
        self.entries.push(entry);
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry<B>> {
        self.entries.iter()
    }
    pub fn get(&self, id: usize) -> Option<&Entry<B>> {
        self.entries.iter().find(|m| m.id == id)
    }
    pub fn get_mut(&mut self, id: usize) -> Option<&mut Entry<B>> {
        self.entries.iter_mut().find(|m| m.id == id)
    }

    pub fn pin(&self, id: usize) -> Result<Arc<()>, Errno> {
        Ok(self.get(id).ok_or(Errno(ENOENT))?.references.clone())
    }

    pub fn set_bind(&mut self, id: usize, target: InodeKey) -> Result<(), Errno> {
        if target.0 == id || self.get(target.0).is_none() {
            return Err(Errno(EINVAL));
        }
        let entry = self.get_mut(id).ok_or(Errno(ENOENT))?;
        entry.bind = Some(target);
        entry.root = target.1;
        Ok(())
    }

    pub fn enter(&self, key: InodeKey) -> Option<InodeKey> {
        self.entries
            .iter()
            .find(|m| m.cover == Some(key))
            .map(|m| (m.id, m.root))
    }

    pub fn remove(&mut self, id: usize, force: bool) -> Result<Entry<B>, Errno> {
        let at = self
            .entries
            .iter()
            .position(|m| m.id == id)
            .ok_or(Errno(ENOENT))?;
        let children = self.entries.iter().any(|m| {
            m.id != id && (m.cover.is_some_and(|k| k.0 == id) || m.bind.is_some_and(|k| k.0 == id))
        });
        if children || !force && Arc::strong_count(&self.entries[at].references) != 1 {
            return Err(Errno(EBUSY));
        }
        Ok(self.entries.remove(at))
    }

    pub fn find_mount(&self, path: &[u8]) -> Option<usize> {
        self.entries
            .iter()
            .filter(|m| within(path, &m.path))
            .max_by_key(|m| m.path.len())
            .map(|m| m.id)
    }

    pub fn flags_at(&self, path: &[u8]) -> MountFlags {
        self.find_mount(path)
            .and_then(|id| self.get(id))
            .map(|m| m.flags)
            .unwrap_or_default()
    }
}

fn within(path: &[u8], prefix: &[u8]) -> bool {
    prefix == b"/"
        || path.starts_with(prefix) && matches!(path.get(prefix.len()), None | Some(b'/'))
}
