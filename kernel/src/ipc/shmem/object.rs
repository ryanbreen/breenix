//! Sparse shared storage. Allocations and page zeroing run outside object/PM locks.
use crate::irq_safe_mutex::IrqSafeMutex;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::{PhysFrame, Size4KiB};
use crate::syscall::errno::{EAGAIN, EINVAL, ENOMEM, EPERM};
use crate::syscall::memory_common::{self, PAGE_SIZE};
use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{PhysFrame, Size4KiB};

static NEXT_INO: AtomicU64 = AtomicU64::new(0x100000);

// Intrusive treap: one fallibly allocated node per resident page, prepared
// before taking inner. Resizing a sparse object allocates no page-index array.
struct PageNode {
    index: u64,
    priority: u64,
    height: u8,
    count: u64,
    frame: PhysFrame<Size4KiB>,
    left: Option<Box<PageNode>>,
    right: Option<Box<PageNode>>,
}
impl Drop for PageNode {
    fn drop(&mut self) {
        release_frame(self.frame);
    }
}
fn priority(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn find(root: &Option<Box<PageNode>>, index: u64) -> Option<PhysFrame<Size4KiB>> {
    let mut node = root.as_deref();
    while let Some(n) = node {
        if index == n.index {
            return Some(n.frame);
        }
        node = if index < n.index {
            n.left.as_deref()
        } else {
            n.right.as_deref()
        };
    }
    None
}
fn refresh(node: &mut PageNode) {
    node.height = 1 + node
        .left
        .as_ref()
        .map_or(0, |n| n.height)
        .max(node.right.as_ref().map_or(0, |n| n.height));
    node.count =
        1 + node.left.as_ref().map_or(0, |n| n.count) + node.right.as_ref().map_or(0, |n| n.count);
}
fn insert(root: &mut Option<Box<PageNode>>, node: Box<PageNode>) {
    let Some(mut top) = root.take() else {
        *root = Some(node);
        return;
    };
    if node.index < top.index {
        insert(&mut top.left, node);
        if top.left.as_ref().is_some_and(|n| n.priority < top.priority) {
            let mut left = top.left.take().unwrap();
            top.left = left.right.take();
            refresh(&mut top);
            left.right = Some(top);
            top = left;
        }
    } else {
        insert(&mut top.right, node);
        if top
            .right
            .as_ref()
            .is_some_and(|n| n.priority < top.priority)
        {
            let mut right = top.right.take().unwrap();
            top.right = right.left.take();
            refresh(&mut top);
            right.left = Some(top);
            top = right;
        }
    }
    refresh(&mut top);
    *root = Some(top);
}
// Detach pages >= first without walking or freeing the detached pages
// while interrupts are masked. Treap height bounds the locked section.
fn split(
    root: Option<Box<PageNode>>,
    first: u64,
) -> (Option<Box<PageNode>>, Option<Box<PageNode>>) {
    let Some(mut node) = root else {
        return (None, None);
    };
    if node.index < first {
        let (keep, remove) = split(node.right.take(), first);
        node.right = keep;
        refresh(&mut node);
        (Some(node), remove)
    } else {
        let (keep, remove) = split(node.left.take(), first);
        node.left = remove;
        refresh(&mut node);
        (keep, Some(node))
    }
}

struct ShmInner {
    size: u64,
    pages: Option<Box<PageNode>>,
    mode: u32,
    uid: u32,
    gid: u32,
    linked: bool,
    resizing: bool,
    generation: u64,
    atime: i64,
    dtime: i64,
    lpid: i32,
}

/// Anonymous backing also used by System V segments.
pub struct ShmObject {
    ino: u64,
    inner: IrqSafeMutex<ShmInner>,
    maps: AtomicUsize,
}
impl core::fmt::Debug for ShmObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ShmObject").field("ino", &self.ino).finish()
    }
}

pub fn page_count(size: u64) -> Result<usize, u64> {
    if size > i64::MAX as u64 {
        return Err(EINVAL as u64);
    }
    usize::try_from(size.div_ceil(PAGE_SIZE)).map_err(|_| ENOMEM as u64)
}
pub(crate) fn release_frame(frame: PhysFrame<Size4KiB>) {
    if crate::memory::frame_metadata::frame_decref(frame) {
        crate::memory::frame_allocator::deallocate_frame(frame);
    }
}
impl ShmObject {
    pub fn new(size: u64, mode: u32, uid: u32, gid: u32) -> Result<Arc<ShmObject>, u64> {
        page_count(size)?;
        Arc::try_new(Self {
            ino: NEXT_INO.fetch_add(1, Ordering::Relaxed),
            inner: IrqSafeMutex::new(ShmInner {
                size,
                pages: None,
                mode: mode & 0o7777,
                uid,
                gid,
                linked: true,
                resizing: false,
                generation: 0,
                atime: 0,
                dtime: 0,
                lpid: 0,
            }),
            maps: AtomicUsize::new(0),
        })
        .map_err(|_| ENOMEM as u64)
    }
    pub fn size(&self) -> u64 {
        self.inner.lock().size
    }
    pub fn ino(&self) -> u64 {
        self.ino
    }
    pub fn attachments(&self) -> usize {
        self.maps.load(Ordering::Acquire)
    }
    /// System V attachment activity, including implicit detach at exec/exit.
    pub fn activity(&self) -> (i64, i64, i32) {
        let inner = self.inner.lock();
        (inner.atime, inner.dtime, inner.lpid)
    }
    fn record_activity(&self, attach: bool) {
        let now = crate::time::current_unix_time();
        let pid = crate::arch_without_interrupts(|| {
            #[cfg(target_arch = "x86_64")]
            let ptr = {
                use crate::arch_impl::PerCpuOps;
                crate::arch_impl::x86_64::percpu::X86PerCpu::current_thread_ptr()
            };
            #[cfg(target_arch = "aarch64")]
            let ptr = crate::per_cpu_aarch64::current_thread_ptr();
            let ptr = ptr as *const crate::task::thread::Thread;
            if ptr.is_null() {
                return 0;
            }
            // IRQ masking stabilizes the current thread; its group identity is
            // immutable. Do not call getpid's PM-taking fallback from drop.
            unsafe {
                (*ptr)
                    .cpu_account
                    .as_ref()
                    .map_or(0, |a| a.process_id as i32)
            }
        });
        let mut inner = self.inner.lock();
        if attach {
            inner.atime = now;
        } else {
            inner.dtime = now;
        }
        inner.lpid = pid;
    }
    pub fn allocated_pages(&self) -> u64 {
        self.inner.lock().pages.as_ref().map_or(0, |n| n.count)
    }
    pub fn metadata(&self) -> (u32, u32, u32, bool) {
        let i = self.inner.lock();
        (i.mode, i.uid, i.gid, i.linked)
    }
    pub fn unlink(&self) {
        self.inner.lock().linked = false;
    }
    pub fn chmod(&self, mode: u32, cred: &crate::fs::permissions::Credentials) -> Result<(), u64> {
        let mut i = self.inner.lock();
        if cred.euid != 0 && cred.euid != i.uid {
            return Err(EPERM as u64);
        }
        i.mode = mode & 0o7777;
        if cred.euid != 0 && !cred.in_group(i.gid) {
            i.mode &= !0o2000;
        }
        Ok(())
    }
    pub fn chown(
        &self,
        uid: u32,
        gid: u32,
        cred: &crate::fs::permissions::Credentials,
    ) -> Result<(), u64> {
        let mut i = self.inner.lock();
        if cred.euid != 0
            && (cred.euid != i.uid
                || (uid != u32::MAX && uid != i.uid)
                || (gid != u32::MAX && gid != i.gid && !cred.in_group(gid)))
        {
            return Err(EPERM as u64);
        }
        if uid != u32::MAX {
            i.uid = uid;
        }
        if gid != u32::MAX {
            i.gid = gid;
        }
        i.mode &= !0o6000;
        Ok(())
    }
    pub fn resize(&self, len: u64) -> Result<(), u64> {
        page_count(len)?;
        let old = loop {
            let mut inner = self.inner.lock();
            if !inner.resizing {
                inner.resizing = true;
                inner.generation = inner.generation.wrapping_add(1);
                break inner.size;
            }
            drop(inner);
            crate::task::scheduler::yield_current();
        };
        // No object lock held while taking PM; faults retry during this window.
        let result = if len < old {
            super::invalidate_object(self, len)
        } else {
            Ok(())
        };
        let removed = {
            let mut inner = self.inner.lock();
            if result.is_err() {
                inner.resizing = false;
                return result;
            }
            // Writes through a mapping past EOF must not become file contents
            // on growth. Clear from min(old,new) even when the size increases.
            let tail = len.min(old);
            if tail % PAGE_SIZE != 0 {
                if let Some(frame) = find(&inner.pages, tail / PAGE_SIZE) {
                    unsafe {
                        core::ptr::write_bytes(
                            (crate::memory::physical_memory_offset().as_u64()
                                + frame.start_address().as_u64()
                                + tail % PAGE_SIZE) as *mut u8,
                            0,
                            (PAGE_SIZE - tail % PAGE_SIZE) as usize,
                        );
                        #[cfg(target_arch = "aarch64")]
                        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
                    }
                }
            }
            let (keep, remove) = split(inner.pages.take(), len.div_ceil(PAGE_SIZE));
            inner.pages = keep;
            inner.size = len;
            inner.resizing = false;
            remove
        };
        drop(removed);
        Ok(())
    }
    /// Called outside PM. A concurrent shrink prevents stale publication;
    /// EAGAIN requests a retry after the resize transition completes.
    pub(crate) fn ensure_page(&self, offset: u64) -> Result<(), u64> {
        let index = offset / PAGE_SIZE;
        let generation = {
            let inner = self.inner.lock();
            if inner.resizing {
                return Err(EAGAIN as u64);
            }
            if index >= inner.size.div_ceil(PAGE_SIZE) {
                return Ok(());
            }
            if find(&inner.pages, index).is_some() {
                return Ok(());
            }
            inner.generation
        };
        let mut frames = memory_common::allocate_zeroed_frames(1).ok_or(ENOMEM as u64)?;
        let frame = frames.pop().ok_or(ENOMEM as u64)?;
        crate::memory::frame_metadata::frame_register(frame);
        // The node owns the registered frame, including allocation failure.
        let node = PageNode {
            index,
            priority: priority(index),
            height: 1,
            count: 1,
            frame,
            left: None,
            right: None,
        };
        let node = Box::try_new(node).map_err(|_| ENOMEM as u64)?;
        let mut inner = self.inner.lock();
        if inner.resizing || inner.generation != generation {
            drop(inner);
            drop(node);
            return Err(EAGAIN as u64);
        }
        if find(&inner.pages, index).is_some() {
            drop(inner);
            drop(node);
            return Ok(());
        }
        // A malicious sparse access pattern cannot drive recursive tree
        // operations past a bounded kernel-stack depth. A normal tree grows
        // logarithmically; the rare degenerate case fails fallibly.
        if inner.pages.as_ref().is_some_and(|n| n.height >= 63) {
            drop(inner);
            drop(node);
            return Err(ENOMEM as u64);
        }
        insert(&mut inner.pages, node);
        Ok(())
    }
    pub(crate) fn resizing(&self) -> bool {
        self.inner.lock().resizing
    }
    pub(crate) fn frame_at(&self, offset: u64) -> Option<PhysFrame<Size4KiB>> {
        let inner = self.inner.lock();
        if inner.resizing || offset / PAGE_SIZE >= inner.size.div_ceil(PAGE_SIZE) {
            return None;
        }
        let frame = find(&inner.pages, offset / PAGE_SIZE)?;
        crate::memory::frame_metadata::frame_incref(frame);
        Some(frame)
    }
}

/// Origin and the original maximum permission survive VMA splits and fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingOrigin {
    Mmap,
    SysV,
}
#[derive(Debug)]
pub struct ShmMapping {
    object: Arc<ShmObject>,
    origin: MappingOrigin,
    may_write: bool,
}
impl ShmMapping {
    pub fn new(object: &Arc<ShmObject>) -> Self {
        Self::with_origin(object, MappingOrigin::SysV, true)
    }
    pub fn with_origin(object: &Arc<ShmObject>, origin: MappingOrigin, may_write: bool) -> Self {
        object.maps.fetch_add(1, Ordering::AcqRel);
        if origin == MappingOrigin::SysV {
            object.record_activity(true);
        }
        Self {
            object: object.clone(),
            origin,
            may_write,
        }
    }
    pub fn object(&self) -> &Arc<ShmObject> {
        &self.object
    }
    pub fn may_write(&self) -> bool {
        self.may_write
    }
    pub fn origin(&self) -> MappingOrigin {
        self.origin
    }
    pub fn matches_attachment(&self, start: u64, offset: u64, addr: u64) -> bool {
        self.origin == MappingOrigin::SysV && start.checked_sub(offset) == Some(addr)
    }
}
impl Clone for ShmMapping {
    fn clone(&self) -> Self {
        Self::with_origin(&self.object, self.origin, self.may_write)
    }
}
impl Drop for ShmMapping {
    fn drop(&mut self) {
        self.object.maps.fetch_sub(1, Ordering::AcqRel);
        if self.origin == MappingOrigin::SysV {
            self.object.record_activity(false);
        }
    }
}
