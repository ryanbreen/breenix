//! Window tree pages.
//!
//! Tree pages are shared memory mapped into the owner with window protections. GPU targets
//! hold pixel buffers separately in `gpu::window_targets`; pins retain tree memory while
//! captures copy it across a resize or destruction.

use alloc::{sync::Arc, vec::Vec};
use breengel_abi::errno::{EFAULT, ENOMEM, ESRCH};

#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

use crate::ipc::shmem::{Frame, MappingOrigin, ShmObject};
use crate::memory::vma::Protection;
use crate::syscall::memory_common::PAGE_SIZE;

/// Bytes of one page: protection changes by whole pages.
pub const PAGE_BYTES: u64 = PAGE_SIZE;

/// Allocated tree memory.
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

    pub fn len(&self) -> u64 {
        self.len
    }

    fn page_ptr(&self, page: usize) -> *mut u8 {
        let base = crate::memory::physical_memory_offset().as_u64();
        (base + self.frames[page].start_address().as_u64()) as *mut u8
    }

    /// Run `f` on each page-contiguous piece of `[offset, offset + len)`, which must lie in the
    /// memory, as (kernel pointer, bytes, offset into the range).
    pub(super) fn chunks(&self, offset: u64, len: usize, mut f: impl FnMut(*mut u8, usize, usize)) {
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

/// Whether `vma` is a piece of the window mapping of `mem` placed at `start`: the whole
/// mapping, or what is left of it after the owner unmapped part of it (or mapped over part
/// of it). Every piece keeps its place, `vma.start - vma.shm_offset == start`.
fn is_piece_of(vma: &crate::memory::vma::Vma, mem: &Mem, start: u64) -> bool {
    vma.start.as_u64().checked_sub(vma.shm_offset) == Some(start)
        && vma.shm.as_ref().is_some_and(|m| {
            m.origin() == MappingOrigin::Window && Arc::ptr_eq(m.object(), &mem.obj)
        })
}

/// Bytes of `mem` that no piece of its window mapping at `start` in `vmas` maps any more.
pub fn unmapped_len(mem: &Mem, start: u64, vmas: &[crate::memory::vma::Vma]) -> u64 {
    let mapped: u64 = vmas.iter().filter(|v| is_piece_of(v, mem, start)).map(|v| v.size()).sum();
    mem.len.saturating_sub(mapped)
}

/// Remove what is left of the calling process's mapping of `mem` at `start`: every piece of
/// it. A range the process unmapped itself, or reused for another mapping, is left alone.
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
    while let Some((from, to)) = process
        .vmas
        .iter()
        .find(|v| is_piece_of(v, mem, start))
        .map(|v| (v.start.as_u64(), v.end.as_u64()))
    {
        if crate::syscall::mmap::unmap_range(process, from, to).is_err() {
            return;
        }
    }
}

/// Make the calling process's mapping of `mem` at `start` writable or read-only.
pub fn protect(mem: &Mem, start: u64, writable: bool) -> Result<(), i32> {
    protect_range(mem, start, 0, mem.len, writable)
}

/// Make bytes `[offset, offset + len)` of the calling process's mapping of `mem` at `start`
/// writable or read-only, in every piece of the mapping that is left: the pieces are split at
/// the range's ends, pages already installed get the new protection at once on every CPU, and
/// later faults install them with it. ENOMEM, with nothing changed, when the split cannot be
/// recorded.
pub fn protect_range(mem: &Mem, start: u64, offset: u64, len: u64, writable: bool) -> Result<(), i32> {
    let tid = current_thread()?;
    let prot = if writable {
        Protection::from_bits_truncate(Protection::READ.bits() | Protection::WRITE.bits())
    } else {
        Protection::READ
    };
    let (lo, hi) = (start + offset, start + offset + len);
    let mut guard = crate::process::manager_for_address_space(tid);
    let (_, process) = guard
        .as_mut()
        .and_then(|m| m.find_address_space_by_thread_mut(tid))
        .ok_or(ESRCH)?;
    let in_range = |v: &crate::memory::vma::Vma| {
        is_piece_of(v, mem, start) && v.start.as_u64() < hi && lo < v.end.as_u64() && v.prot != prot
    };
    // Each piece changed may leave a prefix and a suffix.
    let pieces = process.vmas.iter().filter(|v| in_range(v)).count();
    process.vmas.try_reserve(2 * pieces).map_err(|_| ENOMEM)?;
    let flags = crate::memory::anon_map::page_flags(prot);
    while let Some(index) = process.vmas.iter().position(|v| in_range(v)) {
        let (vstart, vend) = (process.vmas[index].start.as_u64(), process.vmas[index].end.as_u64());
        let (from, to) = (vstart.max(lo), vend.min(hi));
        let page_table = process.page_table.as_deref_mut().ok_or(ENOMEM)?;
        let mut at = from;
        while let Some(page) = page_table.next_mapped_page(at, to) {
            at = page.start_address().as_u64() + PAGE_SIZE;
            page_table.update_page_flags(page, flags).map_err(|_| ENOMEM)?;
            crate::syscall::memory_common::flush_tlb(page.start_address());
        }
        let prefix = (vstart < from).then(|| process.vmas[index].piece(VirtAddr::new(vstart), VirtAddr::new(from)));
        let suffix = (to < vend).then(|| process.vmas[index].piece(VirtAddr::new(to), VirtAddr::new(vend)));
        process.vmas[index] = process.vmas[index].replacement_piece(VirtAddr::new(from), VirtAddr::new(to));
        process.vmas[index].prot = prot;
        process.vmas.extend(prefix.into_iter().chain(suffix));
    }
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
