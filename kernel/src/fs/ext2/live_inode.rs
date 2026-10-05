//! Runtime inode identity. The mounted filesystem owns the table; external
//! handles count observers independently of that owner. Drop never performs I/O.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

static NEXT_MOUNT: AtomicU64 = AtomicU64::new(1);
static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileKey {
    pub mount_instance: u64,
    pub inode: u32,
    pub incarnation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MountPin {
    pub mount_id: usize,
    pub instance: u64,
}

fn identity(next: &AtomicU64) -> Result<u64, &'static str> {
    next.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        value.checked_add(1)
    })
    .map_err(|_| "ext2 identity space exhausted")
}

impl MountPin {
    pub(super) fn new(mount_id: usize) -> Result<Self, &'static str> {
        Ok(Self {
            mount_id,
            instance: identity(&NEXT_MOUNT)?,
        })
    }

    pub fn verify(&self, fs: &super::Ext2Fs) -> Result<(), &'static str> {
        if fs.mount_id != self.mount_id || fs.mount_pin != *self {
            return Err("Stale ext2 mount identity");
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct LiveInode {
    pub key: FileKey,
    pub mount: MountPin,
    pub size: AtomicU64,
    pub size_epoch: AtomicU64,
    /// Cached pages and reverse map for file mappings.
    pub map: crate::memory::file_map::MapState,
    external_handles: AtomicUsize,
    pending: AtomicBool,
    pub(super) orphan: AtomicBool,
}

impl LiveInode {
    pub(super) fn unused(&self) -> bool {
        self.external_handles.load(Ordering::Acquire) == 0
    }

    /// Queue this orphan for the finalizer service.
    pub(super) fn defer(&self) {
        self.pending.store(true, Ordering::Release);
        super::writeback::request();
    }

    pub(super) fn publish_size(&self, size: u64) {
        if self.size.swap(size, Ordering::AcqRel) != size {
            self.size_epoch.fetch_add(1, Ordering::Release);
        }
    }
}

#[derive(Debug)]
pub struct FileHandle {
    pub(crate) object: Arc<LiveInode>,
}

impl FileHandle {
    fn acquire(object: Arc<LiveInode>) -> Self {
        object.external_handles.fetch_add(1, Ordering::AcqRel);
        Self { object }
    }

    pub fn verify(&self, fs: &super::Ext2Fs) -> Result<u32, &'static str> {
        self.object.mount.verify(fs)?;
        if self.object.key.mount_instance != self.object.mount.instance {
            return Err("Stale ext2 inode mount identity");
        }
        // A counted handle prevents reclamation/reuse under the same FS guard.
        // No resident/identity index lock belongs on the empty-cache I/O path.
        Ok(self.object.key.inode)
    }

    /// Give up the only external handle without deferring it. The caller
    /// holds the filesystem write guard and reclaims the orphan itself; no
    /// other handle can appear because an unlinked inode has no path.
    pub(super) fn release_sole(self) -> Result<Arc<LiveInode>, Self> {
        if self
            .object
            .external_handles
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(self);
        }
        let handle = core::mem::ManuallyDrop::new(self);
        // SAFETY: the handle is never dropped, so its Arc is moved out once.
        Ok(unsafe { core::ptr::read(&handle.object) })
    }
}

impl Clone for FileHandle {
    fn clone(&self) -> Self {
        Self::acquire(self.object.clone())
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        // Only an orphan's last observer leaves work behind. Unused linked
        // inodes are pruned by the next pin and never wake the finalizer.
        if self.object.external_handles.fetch_sub(1, Ordering::AcqRel) == 1
            && self.object.orphan.load(Ordering::Acquire)
        {
            self.object.defer();
        }
    }
}

/// Unused linked entries one pin examines for retirement.
const PRUNE_PER_PIN: usize = 2;
/// Inodes one ext2 service pass evicts file-mapping cache pages from.
pub const EVICTION_BATCH: usize = 32;

struct Table {
    objects: BTreeMap<u32, Arc<LiveInode>>,
    prune_cursor: u32,
}

impl Table {
    /// Retire unused linked entries from a rotating window, so a pin costs a
    /// bounded number of lookups however many entries the table holds. A pin
    /// examines more entries than it can add, so stale entries do not pile up.
    fn prune(&mut self) {
        use core::ops::Bound::{Excluded, Included, Unbounded};
        let mut stale = [0u32; PRUNE_PER_PIN];
        let mut found = 0;
        let cursor = self.prune_cursor;
        for (&inode, object) in self
            .objects
            .range((Excluded(cursor), Unbounded))
            .chain(self.objects.range((Unbounded, Included(cursor))))
            .take(PRUNE_PER_PIN)
        {
            self.prune_cursor = inode;
            if object.unused() && !object.orphan.load(Ordering::Acquire) && !object.map.nonempty() {
                stale[found] = inode;
                found += 1;
            }
        }
        for inode in &stale[..found] {
            self.objects.remove(inode);
        }
    }
}

pub(super) struct LiveInodes {
    table: Mutex<Table>,
}

impl LiveInodes {
    pub fn new() -> Self {
        Self {
            table: Mutex::new(Table {
                objects: BTreeMap::new(),
                prune_cursor: 0,
            }),
        }
    }

    /// Caller holds the selected filesystem guard through path lookup and pin.
    pub fn pin(&self, mount: MountPin, inode: u32, size: u64) -> Result<FileHandle, &'static str> {
        let mut table = self.table.lock();
        // A count can only rise from zero here, under this lock, so unused
        // linked entries are safe to drop; orphans stay until reclaimed.
        table.prune();
        let object = if let Some(object) = table.objects.get(&inode) {
            // An entry kept while unused may predate a size change the caller
            // has just read under the guard.
            object.publish_size(size);
            object.map.resync_unbound(size);
            object.clone()
        } else {
            let object = Arc::new(LiveInode {
                key: FileKey {
                    mount_instance: mount.instance,
                    inode,
                    incarnation: identity(&NEXT_INCARNATION)?,
                },
                mount,
                size: AtomicU64::new(size),
                size_epoch: AtomicU64::new(0),
                map: crate::memory::file_map::MapState::new(size),
                external_handles: AtomicUsize::new(0),
                pending: AtomicBool::new(false),
                orphan: AtomicBool::new(false),
            });
            table.objects.insert(inode, object.clone());
            object
        };
        Ok(FileHandle::acquire(object))
    }

    /// The live entry for `inode`, if one exists.
    pub fn get(&self, inode: u32) -> Option<Arc<LiveInode>> {
        self.table.lock().objects.get(&inode).cloned()
    }

    /// Up to `EVICTION_BATCH` entries whose file-mapping cache holds pages
    /// no binding covers or awaits writeback, in key order starting after
    /// `after` and wrapping, so entries that stay pending cannot keep later
    /// ones out of every batch.
    pub fn evictions(&self, after: u32) -> alloc::vec::Vec<Arc<LiveInode>> {
        use core::ops::Bound::{Excluded, Included, Unbounded};
        let table = self.table.lock();
        table
            .objects
            .range((Excluded(after), Unbounded))
            .chain(table.objects.range((Unbounded, Included(after))))
            .map(|(_, object)| object)
            .filter(|object| {
                !object.orphan.load(Ordering::Acquire)
                    && (object.map.eviction_pending() || object.map.writeback_pending())
            })
            .take(EVICTION_BATCH)
            .cloned()
            .collect()
    }

    /// Whether any entry's file-mapping cache still awaits eviction.
    pub fn evictions_pending(&self) -> bool {
        self.table.lock().objects.values().any(|object| {
            !object.orphan.load(Ordering::Acquire)
                && (object.map.eviction_pending() || object.map.writeback_pending())
        })
    }

    pub fn publish_size(&self, inode: u32, size: u64) {
        if let Some(object) = self.table.lock().objects.get(&inode) {
            object.publish_size(size);
        }
    }

    /// Snapshot dirty mappings without resurrecting an unused orphan. A live
    /// orphan's count must be incremented atomically against the last drop.
    pub(crate) fn dirty_handles(&self) -> Result<alloc::vec::Vec<FileHandle>, &'static str> {
        let table = self.table.lock();
        let mut handles = alloc::vec::Vec::new();
        for object in table
            .objects
            .values()
            .filter(|object| object.map.has_dirty())
        {
            handles
                .try_reserve(1)
                .map_err(|_| "Out of memory for sync handles")?;
            if object.orphan.load(Ordering::Acquire) {
                if object
                    .external_handles
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        if count == 0 {
                            None
                        } else {
                            count.checked_add(1)
                        }
                    })
                    .is_err()
                {
                    continue;
                }
                handles.push(FileHandle {
                    object: object.clone(),
                });
            } else {
                handles.push(FileHandle::acquire(object.clone()));
            }
        }
        Ok(handles)
    }

    pub fn pending(&self, after: u32) -> alloc::vec::Vec<Arc<LiveInode>> {
        use core::ops::Bound::{Excluded, Included, Unbounded};
        let table = self.table.lock();
        table
            .objects
            .range((Excluded(after), Unbounded))
            .chain(table.objects.range((Unbounded, Included(after))))
            .map(|(_, object)| object)
            .filter(|object| object.unused() && object.pending.load(Ordering::Acquire))
            .take(32)
            .cloned()
            .collect()
    }

    pub fn remove(&self, object: &Arc<LiveInode>) {
        let mut table = self.table.lock();
        if object.unused()
            && table
                .objects
                .get(&object.key.inode)
                .is_some_and(|entry| Arc::ptr_eq(entry, object))
        {
            table.objects.remove(&object.key.inode);
        }
    }

    /// Whether an external handle or an unreclaimed orphan still depends on
    /// this mount. Unused linked entries awaiting pruning do not count.
    pub fn is_pinned(&self) -> bool {
        self.table.lock().objects.values().any(|object| {
            !object.unused() || object.orphan.load(Ordering::Acquire) || object.map.nonempty()
        })
    }
}
