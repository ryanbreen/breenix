//! Window memory: buffers and tree pages.
//!
//! Each buffer (and each window's tree pages) is a shared-memory object the kernel holds.
//! The owner sees it through a window mapping (`MappingOrigin::Window`): shared, never copied
//! by fork, and with a protection only this module changes. The kernel reads and writes the
//! pixels through the object's frames, each of which a `Mem` holds a reference to, so a
//! composite, capture or replay copy that holds a `Mem` keeps its pages alive even after the
//! window is destroyed or resized.

use alloc::{sync::Arc, vec::Vec};
use breengel_abi::errno::{EFAULT, ENOMEM, ESRCH};

use crate::ipc::shmem::{Frame, MappingOrigin, ShmObject};
use crate::memory::vma::Protection;
use crate::syscall::memory_common::PAGE_SIZE;

/// Allocated window memory, counted by the trace counter `window_buffer_allocs` per buffer.
pub struct Mem {
    obj: Arc<ShmObject>,
    frames: Vec<Frame>,
    len: u64,
}

impl core::fmt::Debug for Mem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Mem").field("len", &self.len).finish()
    }
}

impl Mem {
    /// `len` bytes (rounded up to whole pages) of zeroed memory, every page resident.
    pub fn new(len: u64) -> Result<Arc<Mem>, i32> {
        let len = len.div_ceil(PAGE_SIZE).max(1) * PAGE_SIZE;
        let obj = ShmObject::new(len, 0o600, 0, 0).map_err(|_| ENOMEM)?;
        let pages = (len / PAGE_SIZE) as usize;
        let mut frames = Vec::new();
        frames.try_reserve_exact(pages).map_err(|_| ENOMEM)?;
        let mut mem = Mem {
            obj,
            frames,
            len,
        };
        for page in 0..pages as u64 {
            mem.obj.ensure_page(page * PAGE_SIZE).map_err(|_| ENOMEM)?;
            let frame = mem.obj.frame_at(page * PAGE_SIZE).ok_or(ENOMEM)?;
            mem.frames.push(frame);
        }
        Arc::try_new(mem).map_err(|_| ENOMEM)
    }

    pub fn object(&self) -> &Arc<ShmObject> {
        &self.obj
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    fn page_ptr(&self, page: usize) -> *mut u8 {
        let base = crate::memory::physical_memory_offset().as_u64();
        (base + self.frames[page].start_address().as_u64()) as *mut u8
    }

    /// Run `f` on each page-contiguous piece of `[offset, offset + len)`, which must lie in the
    /// memory, as (kernel pointer, bytes, offset into the range).
    fn chunks(&self, offset: u64, len: usize, mut f: impl FnMut(*mut u8, usize, usize)) {
        debug_assert!(offset + len as u64 <= self.len);
        let mut done = 0usize;
        while done < len {
            let at = offset + done as u64;
            let page = (at / PAGE_SIZE) as usize;
            let within = (at % PAGE_SIZE) as usize;
            let n = (PAGE_SIZE as usize - within).min(len - done);
            // SAFETY: the page is one of this memory's frames, held by `self`, and the
            // piece stays inside it.
            let ptr = unsafe { self.page_ptr(page).add(within) };
            f(ptr, n, done);
            done += n;
        }
    }

    /// Copy `dst.len()` bytes at `offset` out of the memory.
    pub fn read(&self, offset: u64, dst: &mut [u8]) {
        let base = dst.as_mut_ptr();
        self.chunks(offset, dst.len(), |ptr, n, at| {
            // SAFETY: both ranges are valid for `n` bytes and do not overlap.
            unsafe { core::ptr::copy_nonoverlapping(ptr, base.add(at), n) }
        });
    }

    /// Copy `src` into the memory at `offset`.
    pub fn write(&self, offset: u64, src: &[u8]) {
        let base = src.as_ptr();
        self.chunks(offset, src.len(), |ptr, n, at| {
            // SAFETY: both ranges are valid for `n` bytes and do not overlap.
            unsafe { core::ptr::copy_nonoverlapping(base.add(at), ptr, n) }
        });
    }

    /// Copy `len` bytes at `offset` from `src` to the same offset here.
    pub fn copy_from(&self, src: &Mem, offset: u64, len: usize) {
        let mut row = [0u8; 256];
        let mut done = 0usize;
        while done < len {
            let n = (len - done).min(row.len());
            src.read(offset + done as u64, &mut row[..n]);
            self.write(offset + done as u64, &row[..n]);
            done += n;
        }
    }
}

impl Drop for Mem {
    fn drop(&mut self) {
        for frame in self.frames.drain(..) {
            crate::ipc::shmem::release_frame(frame);
        }
    }
}

fn current_thread() -> Result<u64, i32> {
    crate::syscall::memory_common::get_current_thread_id().ok_or(ESRCH)
}

/// Map `mem` into the calling process, read-write or read-only. ENOMEM past `RLIMIT_AS`.
pub fn map(mem: &Mem, writable: bool) -> Result<u64, i32> {
    crate::ipc::shmem::map_window(&mem.obj, mem.len, writable).map_err(|e| e as i32)
}

/// Whether `vma` is the window mapping of `mem` at exactly `[start, start + len)`.
fn is_mapping_of(vma: &crate::memory::vma::Vma, mem: &Mem, start: u64) -> bool {
    vma.start.as_u64() == start
        && vma.end.as_u64() == start + mem.len
        && vma.shm.as_ref().is_some_and(|m| {
            m.origin() == MappingOrigin::Window && Arc::ptr_eq(m.object(), &mem.obj)
        })
}

/// Remove the calling process's mapping of `mem` at `start`, if it is still there. A range
/// the process unmapped itself, or reused for another mapping, is left alone.
pub fn unmap(mem: &Mem, start: u64) {
    let Ok(tid) = current_thread() else {
        return;
    };
    let mut guard = crate::process::manager_for_address_space(tid);
    let Some((_, process)) = guard
        .as_mut()
        .and_then(|m| m.find_address_space_by_thread_mut(tid))
    else {
        return;
    };
    if process.vmas.iter().any(|v| is_mapping_of(v, mem, start)) {
        let _ = crate::syscall::mmap::unmap_range(process, start, start + mem.len);
    }
}

/// Make the calling process's mapping of `mem` at `start` writable or read-only. Pages already
/// installed get the new protection at once, on every CPU; later faults install them with it.
pub fn protect(mem: &Mem, start: u64, writable: bool) -> Result<(), i32> {
    let tid = current_thread()?;
    let prot = if writable {
        Protection::from_bits_truncate(Protection::READ.bits() | Protection::WRITE.bits())
    } else {
        Protection::READ
    };
    let mut guard = crate::process::manager_for_address_space(tid);
    let (_, process) = guard
        .as_mut()
        .and_then(|m| m.find_address_space_by_thread_mut(tid))
        .ok_or(ESRCH)?;
    let Some(index) = process.vmas.iter().position(|v| is_mapping_of(v, mem, start)) else {
        // The owner unmapped its own view: nothing of it can be written.
        return Ok(());
    };
    if process.vmas[index].prot == prot {
        return Ok(());
    }
    let page_table = process.page_table.as_deref_mut().ok_or(ENOMEM)?;
    let flags = crate::memory::anon_map::page_flags(prot);
    let end = start + mem.len;
    let mut from = start;
    while let Some(page) = page_table.next_mapped_page(from, end) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        page_table
            .update_page_flags(page, flags)
            .map_err(|_| ENOMEM)?;
        crate::syscall::memory_common::flush_tlb(page.start_address());
    }
    process.vmas[index].prot = prot;
    Ok(())
}

/// Check that every page of the user range `[addr, addr + len)` can be written by the
/// caller, without changing its contents: each page's first word in the range is exchanged
/// with itself. EFAULT names a range that would fault part way, so an op can refuse it before
/// it writes anything.
pub fn probe_writable(addr: u64, len: u64) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    if addr == 0 || !crate::memory::layout::is_valid_user_range(addr, len as usize) {
        return Err(EFAULT);
    }
    let end = addr.checked_add(len).ok_or(EFAULT)?;
    let mut at = addr;
    while at < end {
        let word = at & !3;
        let value = crate::syscall::userptr::read_user_u32(word).map_err(|_| EFAULT)?;
        crate::syscall::userptr::cmpxchg_user_u32(word, value, value).map_err(|_| EFAULT)?;
        at = (at & !(PAGE_SIZE - 1)) + PAGE_SIZE;
    }
    Ok(())
}

/// Copy `len` bytes from the user range at `addr` into a new kernel vector.
pub fn copy_in(addr: u64, len: usize) -> Result<Vec<u8>, i32> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    v.resize(len, 0);
    crate::syscall::userptr::read_user_bytes(v.as_mut_ptr(), addr, len).map_err(|_| EFAULT)?;
    Ok(v)
}

/// Copy `src` to the user range at `addr`.
pub fn copy_out(addr: u64, src: &[u8]) -> Result<(), i32> {
    if src.is_empty() {
        return Ok(());
    }
    crate::syscall::userptr::write_user_bytes(addr, src.as_ptr(), src.len()).map_err(|_| EFAULT)
}
