//! Runtime inode identity. The mounted filesystem owns the table; external
//! handles count observers independently of that owner. Drop never performs I/O.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

static NEXT_MOUNT: AtomicU64 = AtomicU64::new(1);
static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);
pub(super) static FINALIZATION_PENDING: AtomicBool = AtomicBool::new(false);

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
    external_handles: AtomicUsize,
    pending: AtomicBool,
    pub(super) orphan: AtomicBool,
}

impl LiveInode {
    pub(super) fn unused(&self) -> bool {
        self.external_handles.load(Ordering::Acquire) == 0
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
}

impl Clone for FileHandle {
    fn clone(&self) -> Self {
        Self::acquire(self.object.clone())
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        if self.object.external_handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.object.pending.store(true, Ordering::Release);
            FINALIZATION_PENDING.store(true, Ordering::Release);
        }
    }
}

pub(super) struct LiveInodes {
    objects: Mutex<BTreeMap<u32, Arc<LiveInode>>>,
}

impl LiveInodes {
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(BTreeMap::new()),
        }
    }

    /// Caller holds the selected filesystem guard through path lookup and pin.
    pub fn pin(&self, mount: MountPin, inode: u32, size: u64) -> Result<FileHandle, &'static str> {
        let mut table = self.objects.lock();
        let object = if let Some(object) = table.get(&inode) {
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
                external_handles: AtomicUsize::new(0),
                pending: AtomicBool::new(false),
                orphan: AtomicBool::new(false),
            });
            table.insert(inode, object.clone());
            object
        };
        Ok(FileHandle::acquire(object))
    }

    pub fn publish_size(&self, inode: u32, size: u64) {
        if let Some(object) = self.objects.lock().get(&inode) {
            object.publish_size(size);
        }
    }

    pub fn pending(&self) -> alloc::vec::Vec<Arc<LiveInode>> {
        self.objects
            .lock()
            .values()
            .filter(|object| object.unused() && object.pending.load(Ordering::Acquire))
            .take(32)
            .cloned()
            .collect()
    }

    pub fn remove(&self, object: &Arc<LiveInode>) {
        let mut table = self.objects.lock();
        if object.unused()
            && table
                .get(&object.key.inode)
                .is_some_and(|entry| Arc::ptr_eq(entry, object))
        {
            table.remove(&object.key.inode);
        }
    }

    pub fn is_pinned(&self) -> bool {
        !self.objects.lock().is_empty()
    }
}
