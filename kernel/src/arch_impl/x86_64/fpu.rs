//! Eager CPU state ownership. The console renderer uses SSE on a kernel
//! thread, so dispatch saves and restores their registers too.
use super::{
    xstate,
    xstate_layout::{self, Backend, LEGACY_SIZE},
};
use crate::task::scheduler::{Scheduler, MAX_CPUS};
use alloc::alloc::{alloc_zeroed, dealloc, Layout as AllocLayout};
use core::{
    arch::asm,
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
};

struct Area {
    bytes: NonNull<u8>,
    allocation: AllocLayout,
}

impl Area {
    fn allocate(size: usize) -> Result<Self, &'static str> {
        let allocation =
            AllocLayout::from_size_align(size, 64).map_err(|_| "Invalid xstate size")?;
        // SAFETY: valid nonzero layout. The kernel allocator returns committed memory.
        let bytes =
            NonNull::new(unsafe { alloc_zeroed(allocation) }).ok_or("Failed to allocate xstate")?;
        Ok(Self { bytes, allocation })
    }

    fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.bytes.as_ptr(), self.allocation.size()) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.bytes.as_ptr(), self.allocation.size()) }
    }
}

// Owning pointer with access serialized by thread custody and local IRQ exclusion.
unsafe impl Send for Area {}
unsafe impl Sync for Area {}

impl Drop for Area {
    fn drop(&mut self) {
        unsafe { dealloc(self.bytes.as_ptr(), self.allocation) };
    }
}

pub struct FpuState {
    saved: Area,
    replacement: Area,
    standard: Option<Area>,
}

impl core::fmt::Debug for FpuState {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        out.write_str("FpuState")
    }
}

impl FpuState {
    pub fn allocate(user: bool) -> Result<Self, &'static str> {
        let layout = xstate::layout();
        // Only userspace can import and export signal images. The renderer
        // and other kernel threads still own a persistent saved image.
        let scratch_size = if user {
            layout.standard_size.max(layout.image_size())
        } else {
            layout.image_size()
        };
        let mut saved = Area::allocate(scratch_size)?;
        layout.initial(saved.bytes_mut());
        let replacement = Area::allocate(scratch_size)?;
        let standard = if user {
            Some(Area::allocate(layout.signal_size())?)
        } else {
            None
        };
        Ok(Self {
            saved,
            replacement,
            standard,
        })
    }

    pub fn inherit(tid: u64, unpublished: Option<&Self>) -> Result<Self, &'static str> {
        let mut state = Self::allocate(true)?;
        crate::arch_without_interrupts(|| {
            let layout = xstate::layout();
            let user = state.standard.as_mut().unwrap().bytes_mut();
            if crate::per_cpu::current_thread_id_lock_free() == Some(tid) && is_live(tid) {
                snapshot(state.replacement.bytes_mut());
                layout.export_from(user, state.replacement.bytes(), true);
            } else if let Some(parent) = unpublished {
                layout.export(user, parent.saved.bytes());
            } else {
                return Err("Cannot snapshot parent xstate without its CPU custody");
            }
            let extended = layout.signal_size() > LEGACY_SIZE;
            assert!(layout.import(
                state.saved.bytes_mut(),
                user,
                extended,
                xstate::mxcsr_mask()
            ));
            Ok(())
        })?;
        Ok(state)
    }

    fn save(&mut self) {
        save_to(&mut self.saved);
    }

    fn restore(&self) {
        restore(self.saved.bytes.as_ptr());
    }

    fn reset(&mut self, live: bool) {
        xstate::layout().initial(self.replacement.bytes_mut());
        if live {
            restore(self.replacement.bytes.as_ptr());
        }
        core::mem::swap(&mut self.saved, &mut self.replacement);
    }

    fn export(&mut self, live: bool) -> &[u8] {
        let layout = xstate::layout();
        let standard = self
            .standard
            .as_mut()
            .expect("user thread xstate scratch")
            .bytes_mut();
        if live {
            snapshot(self.replacement.bytes_mut());
            layout.export_from(standard, self.replacement.bytes(), true);
        } else {
            layout.export(standard, self.saved.bytes());
        }
        standard
    }

    fn import(&mut self, addr: u64, extended: bool, live: bool) -> bool {
        let layout = xstate::layout();
        if addr == 0 {
            self.reset(live);
            return true;
        }
        let length = if extended {
            layout.signal_size()
        } else {
            LEGACY_SIZE
        };
        let alignment = if extended { 64 } else { 16 };
        if addr % alignment != 0 || !crate::memory::layout::is_valid_user_range(addr, length) {
            return false;
        }
        let standard = &mut self
            .standard
            .as_mut()
            .expect("user thread xstate scratch")
            .bytes_mut()[..length];
        if crate::syscall::userptr::read_user_bytes(standard.as_mut_ptr(), addr, length).is_err()
            || !layout.import(
                self.replacement.bytes_mut(),
                standard,
                extended,
                xstate::mxcsr_mask(),
            )
        {
            return false;
        }
        if !live {
            core::mem::swap(&mut self.saved, &mut self.replacement);
            return true;
        }
        xstate_layout::replace_live(
            &mut self.saved,
            &mut self.replacement,
            &mut (),
            |_, saved| save_to(saved),
            |_, incoming| try_restore(incoming.bytes.as_ptr()),
            |_, saved| restore(saved.bytes.as_ptr()),
        )
    }
}

fn save_to(area: &mut Area) {
    let layout = xstate::layout();
    // A standard snapshot is used for signal export. The rollback image is
    // an ordinary persistent scheduler image and uses its matching backend.
    let pointer = area.bytes.as_ptr();
    unsafe {
        match layout.backend {
            Backend::Fxsave => {
                asm!("fxsave64 [{}]", in(reg) pointer, options(nostack, preserves_flags))
            }
            Backend::Xsave => xsave(pointer, layout.user),
            Backend::Xsaveopt => asm!("xsaveopt64 [{}]", in(reg) pointer,
                in("eax") layout.user as u32, in("edx") (layout.user >> 32) as u32,
                options(nostack, preserves_flags)),
            Backend::Xsaves => asm!("xsaves64 [{}]", in(reg) pointer,
                in("eax") layout.user as u32, in("edx") (layout.user >> 32) as u32,
                options(nostack, preserves_flags)),
        }
    }
}

unsafe fn xsave(pointer: *mut u8, mask: u64) {
    unsafe {
        asm!("xsave64 [{}]", in(reg) pointer, in("eax") mask as u32, in("edx") (mask >> 32) as u32,
            options(nostack, preserves_flags));
    }
}

fn snapshot(bytes: &mut [u8]) {
    let layout = xstate::layout();
    xstate_layout::initialize(bytes, false, 0);
    unsafe {
        if layout.backend == Backend::Fxsave {
            asm!("fxsave64 [{}]", in(reg) bytes.as_mut_ptr(), options(nostack, preserves_flags));
        } else {
            xsave(bytes.as_mut_ptr(), layout.user);
        }
    }
}

fn restore(pointer: *const u8) {
    restore_with(xstate::layout(), pointer);
}

fn restore_with(layout: &xstate_layout::Layout, pointer: *const u8) {
    unsafe {
        match layout.backend {
            Backend::Fxsave => {
                asm!("fxrstor64 [{}]", in(reg) pointer, options(nostack, readonly, preserves_flags))
            }
            Backend::Xsave | Backend::Xsaveopt => asm!("xrstor64 [{}]", in(reg) pointer,
                in("eax") layout.user as u32, in("edx") (layout.user >> 32) as u32,
                options(nostack, readonly, preserves_flags)),
            Backend::Xsaves => asm!("xrstors64 [{}]", in(reg) pointer,
                in("eax") layout.user as u32, in("edx") (layout.user >> 32) as u32,
                options(nostack, readonly, preserves_flags)),
        }
    }
}

// The fault handler matches a kernel #GP(0) at one of these instruction
// labels and redirects it to the wrapper’s failure return.
core::arch::global_asm!(
    r#"
    .text
    .global breenix_xrstor_fault
    .global breenix_fxrstor_fault
    .global breenix_xrstors_fault
    .global breenix_xrestore_fixup
    .global breenix_try_xrestore
breenix_try_xrestore:
    mov ecx, edx
    mov eax, esi
    mov rdx, rsi
    shr rdx, 32
    cmp ecx, 2
    je 2f
    test ecx, ecx
    je 1f
breenix_xrstor_fault:
    xrstor64 [rdi]
    jmp 3f
1:
breenix_fxrstor_fault:
    fxrstor64 [rdi]
    jmp 3f
2:
breenix_xrstors_fault:
    xrstors64 [rdi]
3:
    mov eax, 1
    ret
breenix_xrestore_fixup:
    xor eax, eax
    ret
"#
);

extern "C" {
    fn breenix_try_xrestore(pointer: *const u8, mask: u64, backend: u32) -> u32;
    static breenix_xrstor_fault: u8;
    static breenix_fxrstor_fault: u8;
    static breenix_xrstors_fault: u8;
    static breenix_xrestore_fixup: u8;
}

fn try_restore(pointer: *const u8) -> bool {
    let layout = xstate::layout();
    let backend = match layout.backend {
        Backend::Fxsave => 0,
        Backend::Xsave | Backend::Xsaveopt => 1,
        Backend::Xsaves => 2,
    };
    unsafe { breenix_try_xrestore(pointer, layout.user, backend) == 1 }
}

pub fn restore_fixup(rip: u64, code: u64) -> Option<u64> {
    if code == 0
        && (rip == core::ptr::addr_of!(breenix_xrstor_fault) as u64
            || rip == core::ptr::addr_of!(breenix_fxrstor_fault) as u64
            || rip == core::ptr::addr_of!(breenix_xrstors_fault) as u64)
    {
        Some(core::ptr::addr_of!(breenix_xrestore_fixup) as u64)
    } else {
        None
    }
}

/// The restore operand includes storage for the enabled components
/// (Intel SDM Vol. 1, §13.13 permits accesses throughout the operand).
pub struct InitialState(Area);

impl InitialState {
    pub fn allocate(layout: &xstate_layout::Layout) -> Result<Self, &'static str> {
        let mut image = Area::allocate(layout.image_size())?;
        layout.initial(image.bytes_mut());
        Ok(Self(image))
    }

    pub fn restore(self, layout: &xstate_layout::Layout) {
        restore_with(layout, self.0.bytes.as_ptr());
    }
}

pub fn start_idle(thread: &mut crate::task::thread::Thread) {
    thread.fpu_mut().restore();
    OWNER[crate::per_cpu::cpu_id()].store(thread.id, Ordering::Relaxed);
}

pub fn frame_size() -> usize {
    xstate::layout().signal_size()
}

pub fn minimum_stack() -> usize {
    use crate::signal::{
        constants::MINSIGSTKSZ, trampoline::SIGNAL_TRAMPOLINE_SIZE, types::SignalFrame,
    };
    MINSIGSTKSZ
        .max(SignalFrame::SIZE + frame_size() + 63 + SIGNAL_TRAMPOLINE_SIZE + 15 + 15 + 8 + 128)
}

pub fn export_signal<T>(tid: u64, write: impl FnOnce(&[u8]) -> T) -> Option<T> {
    crate::arch_without_interrupts(|| {
        let current = crate::per_cpu::current_thread().filter(|thread| thread.id == tid)?;
        let live = is_live(tid);
        Some(write(current.fpu_mut().export(live)))
    })
}

pub fn import_signal(tid: u64, addr: u64, extended: bool) -> bool {
    crate::arch_without_interrupts(|| {
        let Some(current) = crate::per_cpu::current_thread().filter(|thread| thread.id == tid)
        else {
            return false;
        };
        current.fpu_mut().import(addr, extended, is_live(tid))
    })
}

pub fn reset_current() {
    crate::arch_without_interrupts(|| {
        if let Some(current) = crate::per_cpu::current_thread() {
            let live = is_live(current.id);
            current.fpu_mut().reset(live);
        }
    });
}

fn is_live(tid: u64) -> bool {
    let owner = OWNER[crate::per_cpu::cpu_id()].load(Ordering::Relaxed);
    owner == tid || owner == NO_OWNER
}

pub fn x87_fault_code() -> i32 {
    let mut control = 0u16;
    let status: u16;
    unsafe {
        asm!("fnstcw [{}]", in(reg) &mut control as *mut u16, options(nostack, preserves_flags));
        asm!("fnstsw ax", out("ax") status, options(nostack, preserves_flags));
    }
    fault_code(u32::from(status & !control))
}

pub fn simd_fault_code() -> i32 {
    let mut mxcsr = 0u32;
    unsafe { asm!("stmxcsr [{}]", in(reg) &mut mxcsr, options(nostack, preserves_flags)) }
    fault_code(mxcsr & !(mxcsr >> 7))
}

fn fault_code(flags: u32) -> i32 {
    use crate::signal::constants::*;
    if flags & 0x01 != 0 {
        FPE_FLTINV
    } else if flags & 0x04 != 0 {
        FPE_FLTDIV
    } else if flags & 0x08 != 0 {
        FPE_FLTOVF
    } else if flags & 0x12 != 0 {
        FPE_FLTUND
    } else if flags & 0x20 != 0 {
        FPE_FLTRES
    } else {
        FPE_FLTINV
    }
}

/// No thread: a CPU before its first hand-over.
const NO_OWNER: u64 = u64::MAX;

/// The thread whose state each CPU's x87/SSE registers hold. Each entry is
/// read and written only by its own CPU, with interrupts masked.
static OWNER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(NO_OWNER) }; MAX_CPUS];

/// Give this CPU's x87/SSE registers to thread `incoming`: the thread holding
/// them saves them, `incoming` loads what it saved and becomes the owner.
/// Nothing happens when `incoming` already holds them. Before the CPU's first
/// hand-over the holder is `outgoing`, the thread the dispatch is leaving (a
/// thread that entered user mode without a dispatch, such as init, holds them
/// then).
///
/// Runs under the scheduler lock, with interrupts masked.
pub fn hand_over(sched: &mut Scheduler, outgoing: Option<u64>, incoming: u64) {
    hand_over_with(outgoing, incoming, |tid| {
        sched
            .get_thread_mut(tid)
            .map(|thread| thread as *mut crate::task::thread::Thread)
    });
}

/// `hand_over`, finding the threads with `thread`, which returns a thread the
/// caller's lock lets it write, or None.
pub fn hand_over_with(
    outgoing: Option<u64>,
    incoming: u64,
    mut thread: impl FnMut(u64) -> Option<*mut crate::task::thread::Thread>,
) {
    let cpu = crate::per_cpu::cpu_id();
    let owner = match OWNER[cpu].load(Ordering::Relaxed) {
        NO_OWNER => outgoing,
        owner => Some(owner),
    };
    if owner == Some(incoming) {
        return;
    }
    let Some(next) = thread(incoming) else { return };
    if let Some(holder) = owner.and_then(&mut thread) {
        // SAFETY: the caller's lock covers the thread, and no other reference
        // to it is live here.
        unsafe { (*holder).fpu_mut().save() };
    }
    // SAFETY: as above.
    unsafe { (*next).fpu_mut().restore() };
    OWNER[cpu].store(incoming, Ordering::Relaxed);
}
