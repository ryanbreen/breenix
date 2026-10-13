//! Sparse shared storage. Allocations and page zeroing run outside object/PM locks.
use crate::irq_safe_mutex::IrqSafeMutex;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::{PhysFrame, Size4KiB};
use crate::syscall::errno::{EAGAIN, EINVAL, ENOMEM, EPERM};
use crate::syscall::memory_common::{self, PAGE_SIZE};
use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, AtomicUsize, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{PhysFrame, Size4KiB};

static NEXT_INO: AtomicU64 = AtomicU64::new(0x100000);

// Compressed binary radix tree: two fallible nodes per resident page at most.
// Branch bits strictly decrease, bounding find/insert/split/drop to 51 levels
// for the page indices of an i64-sized object, independent of touch order.
struct PageNode {
    index: u64,
    bit: u64,
    count: u64,
    frame: Option<PhysFrame<Size4KiB>>,
    left: Option<Box<PageNode>>,
    right: Option<Box<PageNode>>,
}
impl Drop for PageNode {
    fn drop(&mut self) {
        if let Some(frame) = self.frame {
            release_frame(frame);
        }
    }
}
fn find(root: &Option<Box<PageNode>>, index: u64) -> Option<PhysFrame<Size4KiB>> {
    let mut node = root.as_deref();
    while let Some(n) = node {
        if n.bit == 0 {
            return (n.index == index).then_some(n.frame).flatten();
        }
        node = if index & n.bit == 0 {
            n.left.as_deref()
        } else {
            n.right.as_deref()
        };
    }
    None
}
fn refresh(node: &mut PageNode) {
    node.index = node.left.as_ref().map_or(node.index, |n| n.index);
    node.count =
        node.left.as_ref().map_or(0, |n| n.count) + node.right.as_ref().map_or(0, |n| n.count);
}
fn insert(root: &mut Option<Box<PageNode>>, node: Box<PageNode>, mut branch: Box<PageNode>) {
    let Some(top) = root.as_ref() else {
        *root = Some(node);
        return;
    };
    let diff = top.index ^ node.index;
    let bit = if diff == 0 {
        0
    } else {
        1u64 << (63 - diff.leading_zeros())
    };
    if top.bit > bit {
        let top = root.as_mut().unwrap();
        if node.index & top.bit == 0 {
            insert(&mut top.left, node, branch);
        } else {
            insert(&mut top.right, node, branch);
        }
        refresh(top);
    } else {
        branch.bit = bit;
        if node.index & bit == 0 {
            branch.left = Some(node);
            branch.right = root.take();
        } else {
            branch.left = root.take();
            branch.right = Some(node);
        }
        refresh(&mut branch);
        *root = Some(branch);
    }
}
// Only one branch per level straddles first. Detached subtrees are freed
// after releasing inner, never while interrupts are masked.
fn split(
    root: Option<Box<PageNode>>,
    first: u64,
) -> (Option<Box<PageNode>>, Option<Box<PageNode>>) {
    let Some(mut node) = root else {
        return (None, None);
    };
    if node.bit == 0 {
        return if node.index < first {
            (Some(node), None)
        } else {
            (None, Some(node))
        };
    }
    let low = node.index & !(node.bit | (node.bit - 1));
    if first <= low {
        return (None, Some(node));
    }
    if first > low | node.bit | (node.bit - 1) {
        return (Some(node), None);
    }
    if first <= low | node.bit {
        let (keep, remove) = split(node.left.take(), first);
        if remove.is_some() {
            node.left = remove;
            refresh(&mut node);
            (keep, Some(node))
        } else {
            (keep, node.right.take())
        }
    } else {
        let (keep, remove) = split(node.right.take(), first);
        if keep.is_some() {
            node.right = keep;
            refresh(&mut node);
            (Some(node), remove)
        } else {
            (node.left.take(), remove)
        }
    }
}

struct ShmInner {
    size: u64,
    pages: Option<Box<PageNode>>,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u32,
    resizing: bool,
    generation: u64,
}

/// Anonymous backing also used by System V segments.
pub struct ShmObject {
    ino: u64,
    inner: IrqSafeMutex<ShmInner>,
    io_busy: AtomicBool,
    maps: AtomicUsize,
    atime: AtomicI64,
    dtime: AtomicI64,
    lpid: AtomicI32,
}
/// Serialize descriptor I/O and truncate without retaining an IRQ-masking
/// lock across allocation, user copy or PM acquisition.
pub(crate) struct ObjectIo<'a>(&'a ShmObject);
impl Drop for ObjectIo<'_> {
    fn drop(&mut self) {
        self.0.io_busy.store(false, Ordering::Release);
    }
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
                links: 1,
                resizing: false,
                generation: 0,
            }),
            io_busy: AtomicBool::new(false),
            maps: AtomicUsize::new(0),
            atime: AtomicI64::new(0),
            dtime: AtomicI64::new(0),
            lpid: AtomicI32::new(0),
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
        (
            self.atime.load(Ordering::Acquire),
            self.dtime.load(Ordering::Acquire),
            self.lpid.load(Ordering::Acquire),
        )
    }
    fn record_activity(&self, attach: bool, pid: i32) {
        let now = crate::time::current_unix_time_coarse();
        if attach {
            self.atime.store(now, Ordering::Release);
        } else {
            self.dtime.store(now, Ordering::Release);
        }
        self.lpid.store(pid, Ordering::Release);
    }
    pub fn allocated_pages(&self) -> u64 {
        self.inner.lock().pages.as_ref().map_or(0, |n| n.count)
    }
    pub fn metadata(&self) -> (u32, u32, u32, bool) {
        let i = self.inner.lock();
        (i.mode, i.uid, i.gid, i.links != 0)
    }
    pub fn unlink(&self) {
        let mut inner = self.inner.lock();
        inner.links = inner.links.saturating_sub(1);
    }
    pub fn links(&self) -> u32 {
        self.inner.lock().links
    }
    pub fn link(&self) -> Result<(), u64> {
        let mut inner = self.inner.lock();
        inner.links = inner
            .links
            .checked_add(1)
            .ok_or(crate::syscall::errno::EMLINK as u64)?;
        Ok(())
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
    pub(crate) fn io_guard(&self) -> ObjectIo<'_> {
        while self
            .io_busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            crate::task::scheduler::yield_current();
        }
        ObjectIo(self)
    }
    pub fn resize(&self, len: u64) -> Result<(), u64> {
        let _io = self.io_guard();
        self.resize_locked(len)
    }
    pub(crate) fn resize_locked(&self, len: u64) -> Result<(), u64> {
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
            bit: 0,
            count: 1,
            frame: Some(frame),
            left: None,
            right: None,
        };
        let node = Box::try_new(node).map_err(|_| ENOMEM as u64)?;
        let branch = Box::try_new(PageNode {
            index,
            bit: 0,
            count: 0,
            frame: None,
            left: None,
            right: None,
        })
        .map_err(|_| ENOMEM as u64)?;
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
        insert(&mut inner.pages, node, branch);
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

fn current_pid() -> i32 {
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
    pid
}

/// Origin and the original maximum permission survive VMA splits and fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingOrigin {
    Mmap,
    SysV,
}
/// Linux VM-open accounting is per VMA, including splits and fork.
#[derive(Debug)]
pub struct ShmMapping {
    object: Arc<ShmObject>,
    origin: MappingOrigin,
    may_write: bool,
    pid: i32,
    counted: bool,
}
impl ShmMapping {
    pub fn new(object: &Arc<ShmObject>) -> Self {
        Self::with_origin(object, MappingOrigin::SysV, true)
    }
    pub fn with_origin(object: &Arc<ShmObject>, origin: MappingOrigin, may_write: bool) -> Self {
        Self::for_pid(object, origin, may_write, current_pid())
    }
    fn for_pid(object: &Arc<ShmObject>, origin: MappingOrigin, may_write: bool, pid: i32) -> Self {
        object.maps.fetch_add(1, Ordering::AcqRel);
        if origin == MappingOrigin::SysV {
            object.record_activity(true, pid);
        }
        Self {
            object: object.clone(),
            origin,
            may_write,
            pid,
            counted: true,
        }
    }
    pub fn fork(&self, pid: i32) -> Self {
        Self::for_pid(&self.object, self.origin, self.may_write, pid)
    }
    /// Transfer the original VMA's lifetime to its replacement. Extra split
    /// pieces use ordinary clone/open accounting; this row is still open.
    pub fn replacement(&mut self) -> Self {
        debug_assert!(self.counted);
        self.counted = false;
        Self {
            object: self.object.clone(),
            origin: self.origin,
            may_write: self.may_write,
            pid: self.pid,
            counted: true,
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
        Self::for_pid(&self.object, self.origin, self.may_write, self.pid)
    }
}
impl Drop for ShmMapping {
    fn drop(&mut self) {
        if self.counted {
            self.object.maps.fetch_sub(1, Ordering::AcqRel);
            if self.origin == MappingOrigin::SysV {
                self.object.record_activity(false, self.pid);
            }
        }
    }
}
