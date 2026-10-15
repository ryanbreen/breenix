//! Translation Lookaside Buffer (TLB) management
//!
//! The TLB caches virtual-to-physical address translations, and must be
//! flushed when page table entries are modified to ensure the CPU sees
//! the updated mappings.
//!
//! ## x86_64: shootdown
//!
//! `invlpg` and a CR3 reload act on the executing CPU only. Every flush that
//! follows a page-table change (`flush_page`, `flush_all`) therefore also asks
//! every other online CPU to make the same invalidation, and waits until each
//! one has acknowledged before returning, so a caller that frees a frame after
//! the flush knows no CPU can still reach it through a stale translation.
//!
//! Without PCIDs a CR3 write drops every non-global translation, so a CPU can
//! hold translations only for the root it has loaded since its last CR3 write,
//! which kernel threads keep using after a user thread leaves. Every CR3 load
//! is announced first: Rust writes call `note_root_load`, `set_next_cr3` does
//! it for the root the interrupt-return stubs load, and the stubs' other load
//! restores `saved_process_cr3`, the root the CPU entered the kernel on. The
//! syscall-return signal path reloads the running process's own root, already
//! announced by whichever of these put this CPU on it.
//!
//! A flush of one user address space (`flush_user_page`, `flush_user_pages`) therefore goes only to
//! the CPUs whose announced root, `next_cr3` or `saved_process_cr3` names that
//! space; an unmap by a process running alone costs no NMI. A flush that names
//! no address space (`flush_page`, `flush_all`: kernel mappings, or callers
//! that do not say whose table they changed) asks every other online CPU. The
//! receiver skips work that cannot apply to it: an address-space release is a
//! no-op on a CPU that does not have that root loaded, so it is sent only to
//! CPUs not known to be on the master kernel PML4.
//!
//! An idle CPU leaves the user root it was lazily running on for the master
//! kernel PML4 (`load_kernel_root`) before it halts, so no flush of a user
//! address space, and no release of one, waits for a CPU that is halted. A
//! halted vCPU answers only once its host runs it again, and under nested
//! virtualization that took up to 265 ms. The kernel root is announced after
//! the CR3 write, not before, so a CPU is skipped only once it no longer has
//! the user root loaded.
//!
//! Requests are delivered as NMIs. The kernel spins on many locks with
//! interrupts masked; a fixed-vector IPI to a CPU spinning on a lock the
//! initiator holds would never be taken, and the initiator would wait forever.
//! An NMI is taken whatever RFLAGS.IF says. The handler uses no lock, no GS
//! base and no allocation: it finds its CPU from its local APIC id.
//!
//! Each CPU posts its requests in its own slots, so initiators do not queue
//! behind one another: a target that is slow to answer delays only the
//! initiators that sent to it. An initiator that had interrupts enabled waits
//! with them enabled and preemption held off.
//!
//! With one CPU online every entry point costs one atomic load beyond the
//! local invalidation.
//!
//! On aarch64 the `tlbi ...is` forms broadcast in hardware; nothing here
//! changes that path.

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{tlb, VirtAddr};
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::tlb;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

/// Flush a single page from the TLB, on every CPU that may cache it.
///
/// This is more efficient than flushing the entire TLB when only
/// a single page mapping has changed.
#[inline]
pub fn flush_page(addr: VirtAddr) {
    tlb::flush(addr);
    #[cfg(target_arch = "x86_64")]
    shootdown::request(shootdown::Request::Page(addr.as_u64()));
}

/// Flush one page of the user address space whose page-table root is `root`,
/// on every CPU that may cache a translation of that space.
///
/// The caller has already changed the descriptor: a CPU that announces `root`
/// after this looks finds the new one when it walks the table.
#[inline]
pub fn flush_user_page(root: u64, addr: VirtAddr) {
    tlb::flush(addr);
    #[cfg(target_arch = "x86_64")]
    shootdown::request_for_root(shootdown::Request::Page(addr.as_u64()), root);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = root;
}

/// Flush every translation of the user address space whose page-table root is
/// `root`, on every CPU that may cache one of them.
#[inline]
pub fn flush_user_all(root: u64) {
    tlb::flush_all();
    #[cfg(target_arch = "x86_64")]
    shootdown::request_for_root(shootdown::Request::All, root);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = root;
}

/// Invalidate removed user leaves before their frames can be released.
/// x86 uses one root-targeted full shootdown for a multi-page set; ARM64
/// broadcasts the addresses for all ASIDs with one pair of data barriers.
pub fn flush_user_pages(root: u64, pages: &[u64]) {
    if let [page] = pages {
        flush_user_page(root, VirtAddr::new(*page));
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        for &page in pages {
            tlb::flush(VirtAddr::new(page));
        }
        if !pages.is_empty() {
            shootdown::request_for_root(shootdown::Request::All, root);
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        tlb::flush_pages(pages);
        let _ = root;
    }
}

/// Announce that this CPU is about to load page-table root `root` into CR3.
/// Called before every CR3 write, so a flush of that address space reaches
/// this CPU from the moment it can cache one of its translations.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn note_root_load(root: u64) {
    let cpu = if crate::per_cpu::is_initialized() {
        crate::per_cpu::cpu_id()
    } else {
        0
    };
    shootdown::note_root_load(cpu, root);
}

/// `note_root_load` for CPU `cpu`, before its per-CPU data is set up.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn note_root_load_on(cpu: usize, root: u64) {
    shootdown::note_root_load(cpu, root);
}

/// Flush the entire TLB, on every CPU.
///
/// This forces the CPU to reload all address translations from the page tables.
/// Note: Writing to CR3 also flushes the entire TLB, but this function
/// provides an explicit way to do it.
#[inline]
pub fn flush_all() {
    tlb::flush_all();
    #[cfg(target_arch = "x86_64")]
    shootdown::request(shootdown::Request::All);
}

/// Ensure TLB consistency after page table switch
///
/// This function should be called after switching page tables (writing to CR3)
/// to ensure all TLB entries are properly invalidated. While writing to CR3
/// flushes the TLB on x86_64, this provides an explicit guarantee and
/// documents the intent. A page-table switch changes only the executing CPU,
/// so this flush is local.
#[inline]
pub fn flush_after_page_table_switch() {
    // On x86_64, writing to CR3 flushes the entire TLB, but we can
    // explicitly flush to be absolutely certain and for documentation
    tlb::flush_all();
}

/// Make every other online CPU stop using the page-table root `root` before
/// its frames are returned (address-space teardown).
///
/// A CPU whose CR3 names `root` (a kernel thread or idle running lazily on a
/// dead process's tables) loads the master kernel PML4 instead, which drops
/// every non-global translation of the old space. A CPU whose saved
/// user-return CR3 names `root` is left alone: its interrupt-return path may
/// already hold that value in a register and would reload it after the NMI,
/// so that shadow is a proof blocker the caller re-checks after this returns,
/// never something this request clears. The executing CPU is the caller's to
/// check, as the retirement proof already does. A CPU that has loaded the
/// master kernel PML4 since it last announced any other root is not asked.
/// Returns after every target has acknowledged.
#[cfg(target_arch = "x86_64")]
pub fn release_root_on_other_cpus(root: u64) {
    shootdown::request_release(root);
}

/// Load the master kernel PML4 on this CPU, which is `cpu`, if it is on
/// another root, and announce it once loaded. For a CPU that will run no user
/// code before its next dispatch: an idle CPU, or a kernel thread. Interrupts
/// are masked across the write and the announcement, so no dispatch loads
/// another root between them. Takes no lock and uses no GS-relative data.
#[cfg(target_arch = "x86_64")]
pub fn load_kernel_root(cpu: usize) {
    use x86_64::registers::control::Cr3;

    let Some(kernel) = crate::memory::kernel_page_table::master_kernel_pml4() else {
        return;
    };
    x86_64::instructions::interrupts::without_interrupts(|| {
        let (loaded, flags) = Cr3::read();
        if loaded != kernel {
            // SAFETY: the master kernel PML4 maps every kernel address; the
            // user half of the root it replaces is not used before the next
            // dispatch, which loads its own root.
            unsafe { Cr3::write(kernel, flags) };
        }
        shootdown::note_kernel_root_loaded(cpu, kernel.start_address().as_u64());
    });
}

#[cfg(target_arch = "x86_64")]
pub use shootdown::shootdown_nmi_handler;

#[cfg(target_arch = "x86_64")]
mod shootdown {
    use core::sync::atomic::{fence, AtomicU64, AtomicU8, Ordering};

    use x86_64::structures::idt::InterruptStackFrame;

    use crate::arch_impl::x86_64::{apic, smp};
    use crate::task::scheduler::MAX_CPUS;

    #[derive(Clone, Copy)]
    pub(super) enum Request {
        /// `invlpg` this address.
        Page(u64),
        /// Reload CR3: every non-global translation.
        All,
        /// Leave this page-table root if it is loaded and this CPU is not
        /// returning to it.
        ReleaseRoot(u64),
    }

    const KIND_PAGE: u8 = 1;
    const KIND_ALL: u8 = 2;
    const KIND_RELEASE_ROOT: u8 = 3;

    /// A CPU that has announced no root yet: it may hold any.
    const UNKNOWN_ROOT: u64 = u64::MAX;
    const ROOT_MASK: u64 = !0xfff;
    /// Set in an announcement made after the CR3 write it names, which only
    /// the master kernel PML4's is (`note_kernel_root_loaded`): the CPU has
    /// already left whatever root it was on.
    const KERNEL_ROOT_LOADED: u64 = 1;

    /// The page-table root each CPU last announced before a CR3 write.
    static LOADED_ROOT: [AtomicU64; MAX_CPUS] =
        [const { AtomicU64::new(UNKNOWN_ROOT) }; MAX_CPUS];

    /// Record that `cpu` is about to load `root`. A full barrier, so the
    /// announcement is visible before the CR3 write lets the CPU walk the
    /// table: an initiator that changes a descriptor and then misses this
    /// announcement is ordered before the walk, which sees the change.
    pub(super) fn note_root_load(cpu: usize, root: u64) {
        if cpu < MAX_CPUS {
            LOADED_ROOT[cpu].swap(root & ROOT_MASK, Ordering::SeqCst);
        }
    }

    /// Record that `cpu` has loaded the master kernel PML4 `root`.
    pub(super) fn note_kernel_root_loaded(cpu: usize, root: u64) {
        if cpu < MAX_CPUS {
            LOADED_ROOT[cpu].swap((root & ROOT_MASK) | KERNEL_ROOT_LOADED, Ordering::SeqCst);
        }
    }

    /// `cpu`'s `next_cr3` and `saved_process_cr3`: the roots its
    /// interrupt-return stubs load or restore. Both 0 before per-CPU setup.
    fn return_roots(cpu: usize) -> Option<(u64, u64)> {
        let data = crate::per_cpu::cpu_data(cpu);
        if data.is_null() {
            return None;
        }
        // SAFETY: `data` is CPU `cpu`'s slot of the static per-CPU array; the
        // two fields are word-sized and read without tearing.
        Some(unsafe {
            (
                (&raw const (*data).next_cr3).read_volatile(),
                (&raw const (*data).saved_process_cr3).read_volatile(),
            )
        })
    }

    fn names_root(value: u64, root: u64) -> bool {
        value != 0 && (value & ROOT_MASK) == root
    }

    /// Whether `cpu` may hold a translation of the address space `root`: it
    /// announced that root (or none yet), the interrupt-return stub is about
    /// to load it (`next_cr3`), or the stub restores it (`saved_process_cr3`).
    fn may_hold(cpu: usize, root: u64) -> bool {
        let loaded = LOADED_ROOT[cpu].load(Ordering::SeqCst);
        if loaded == UNKNOWN_ROOT || loaded & ROOT_MASK == root {
            return true;
        }
        return_roots(cpu).map_or(true, |(next, saved)| {
            names_root(next, root) || names_root(saved, root)
        })
    }

    /// Whether `cpu` may have `root` in CR3 or load it on an interrupt return.
    /// Stricter than `may_hold`, because a root's frames are freed once its
    /// release has been answered: an announcement can run ahead of the CR3
    /// write it names, and an abandoned dispatch leaves the CPU on the root it
    /// had, so only a CPU that has loaded the master kernel PML4 since its
    /// last announcement of any other root is known to be off `root`.
    fn may_have_loaded(cpu: usize, root: u64) -> bool {
        let loaded = LOADED_ROOT[cpu].load(Ordering::SeqCst);
        if loaded == UNKNOWN_ROOT || loaded & KERNEL_ROOT_LOADED == 0 {
            return true;
        }
        return_roots(cpu).map_or(true, |(next, saved)| {
            names_root(next, root) || names_root(saved, root)
        })
    }

    /// Request slots: two per CPU. Slot `2 * cpu` is used by a caller that had
    /// interrupts enabled and waits with them enabled; slot `2 * cpu + 1` by
    /// a caller with them masked, or in interrupt context, which waits with
    /// them masked. A wait in the first can be interrupted by a handler that
    /// sends a request of its own from the second; nothing interrupts a wait
    /// in the second except an NMI, and NMI handlers send no request.
    const SLOTS: usize = 2 * MAX_CPUS;
    const _: () = assert!(SLOTS <= 64);
    static REQUEST_KIND: [AtomicU8; SLOTS] = [const { AtomicU8::new(0) }; SLOTS];
    static REQUEST_ARG: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    /// Per target CPU, bit `slot` is set while that slot's request is
    /// outstanding there. Set by the initiator; cleared by the target once its
    /// invalidation is done.
    static PENDING: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// NMIs that found no request addressed to their CPU.
    static UNCLAIMED_NMIS: AtomicU64 = AtomicU64::new(0);

    /// An online CPU that has not acknowledged within this long is not going
    /// to: it is wedged with NMIs blocked, and the kernel stops rather than
    /// free memory that CPU may still translate to.
    const ACK_TIMEOUT_SECONDS: u64 = 5;

    /// Deliver `request` to every other online CPU and wait for every
    /// acknowledgement. One atomic load when only this CPU is online.
    pub(super) fn request(request: Request) {
        deliver(request, |_| true);
    }

    /// Deliver `request` to every other online CPU that may hold a translation
    /// of the address space `root`, and wait for those acknowledgements.
    pub(super) fn request_for_root(request: Request, root: u64) {
        // The descriptor change is ordered before every announcement read.
        fence(Ordering::SeqCst);
        let root = root & ROOT_MASK;
        deliver(request, |cpu| may_hold(cpu, root));
    }

    /// Ask every other online CPU that may still have `root` loaded to leave
    /// it, and wait for those acknowledgements.
    pub(super) fn request_release(root: u64) {
        fence(Ordering::SeqCst);
        let root = root & ROOT_MASK;
        deliver(Request::ReleaseRoot(root), |cpu| may_have_loaded(cpu, root));
    }

    /// Deliver `request` to the other online CPUs `targeted` selects.
    fn deliver(request: Request, targeted: impl Fn(usize) -> bool) {
        if smp::cpus_online() <= 1 {
            return;
        }
        let (kind, arg) = match request {
            Request::Page(addr) => (KIND_PAGE, addr),
            Request::All => (KIND_ALL, 0),
            Request::ReleaseRoot(root) => (KIND_RELEASE_ROOT, root),
        };

        // Preemption held off keeps this thread on this CPU, and so on its
        // slot, while it waits with interrupts enabled.
        let interruptible = x86_64::instructions::interrupts::are_enabled()
            && crate::per_cpu::is_initialized()
            && !crate::per_cpu::in_interrupt();
        if interruptible {
            crate::per_cpu::preempt_disable();
            post_and_wait(kind, arg, &targeted, 0);
            crate::per_cpu::preempt_enable();
        } else {
            x86_64::instructions::interrupts::without_interrupts(|| {
                post_and_wait(kind, arg, &targeted, 1)
            });
        }
    }

    /// Post one request in this CPU's slot `level`, NMI each target and wait
    /// for all of them to acknowledge.
    fn post_and_wait(kind: u8, arg: u64, targeted: &impl Fn(usize) -> bool, level: usize) {
        use crate::arch_impl::PerCpuOps;
        let me = crate::arch_impl::x86_64::percpu::X86PerCpu::cpu_id() as usize;

        let targets = (0..MAX_CPUS)
            .filter(|&cpu| cpu != me && smp::is_cpu_online(cpu) && targeted(cpu))
            .fold(0u64, |mask, cpu| mask | 1 << cpu);
        if targets == 0 {
            return;
        }

        let slot = 2 * me + level;
        let bit = 1u64 << slot;
        REQUEST_KIND[slot].store(kind, Ordering::Relaxed);
        REQUEST_ARG[slot].store(arg, Ordering::Relaxed);
        for cpu in (0..MAX_CPUS).filter(|cpu| targets & (1 << cpu) != 0) {
            PENDING[cpu].fetch_or(bit, Ordering::Release);
        }
        // The request words and every PENDING bit are visible before any
        // target can take its NMI; `send_ipi` also fences before the ICR
        // write.
        fence(Ordering::SeqCst);

        for cpu in (0..MAX_CPUS).filter(|cpu| targets & (1 << cpu) != 0) {
            let apic_id = smp::cpu_apic_id(cpu)
                .unwrap_or_else(|| panic!("TLB shootdown: online CPU {} has no APIC id", cpu));
            if let Err(reason) = apic::send_ipi(apic_id, apic::Ipi::Nmi) {
                panic!("TLB shootdown: NMI to CPU {} failed: {}", cpu, reason);
            }
        }

        let deadline = crate::arch_impl::x86_64::timer::rdtsc()
            + crate::arch_impl::x86_64::timer::frequency_hz() * ACK_TIMEOUT_SECONDS;
        for cpu in (0..MAX_CPUS).filter(|cpu| targets & (1 << cpu) != 0) {
            while PENDING[cpu].load(Ordering::Acquire) & bit != 0 {
                if crate::arch_impl::x86_64::timer::rdtsc() > deadline {
                    panic!(
                        "TLB shootdown: CPU {} did not acknowledge within {} s",
                        cpu, ACK_TIMEOUT_SECONDS
                    );
                }
                core::hint::spin_loop();
            }
        }
    }

    /// The NMI handler: carry out every request addressed to this CPU, and
    /// acknowledge each.
    ///
    /// No lock, no allocation, no GS-relative access: the NMI may have
    /// interrupted any instruction, including an entry stub before its GS
    /// handling. The CPU is found from its local APIC id. A request posted
    /// after the PENDING read is left set; its NMI, held while this handler
    /// runs, is taken once it returns.
    pub extern "x86-interrupt" fn shootdown_nmi_handler(_frame: InterruptStackFrame) {
        let cpu = if apic::active() {
            smp::cpu_of_apic_id(apic::id())
        } else {
            None
        };
        let Some(cpu) = cpu else {
            UNCLAIMED_NMIS.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let pending = PENDING[cpu].load(Ordering::Acquire);
        if pending == 0 {
            UNCLAIMED_NMIS.fetch_add(1, Ordering::Relaxed);
            return;
        }

        for slot in (0..SLOTS).filter(|slot| pending & (1 << slot) != 0) {
            let arg = REQUEST_ARG[slot].load(Ordering::Relaxed);
            match REQUEST_KIND[slot].load(Ordering::Relaxed) {
                KIND_PAGE => x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(arg)),
                KIND_ALL => x86_64::instructions::tlb::flush_all(),
                KIND_RELEASE_ROOT => release_root(cpu, arg),
                _ => {}
            }
        }

        PENDING[cpu].fetch_and(!pending, Ordering::Release);
    }

    /// Leave `root` on this CPU, which is `cpu`.
    ///
    /// A saved user-return CR3 naming `root` means this CPU is still running,
    /// or returning to, a thread of that address space: its return stub reads
    /// that shadow into a register and then loads CR3 from it, so an NMI that
    /// lands between the two cannot stop the reload. Such a CPU is left
    /// untouched and the shadow stays visible; the initiator's proof treats it
    /// as a blocker. Otherwise nothing on this CPU will load `root` again
    /// (dead threads are never dispatched), and leaving it is final.
    fn release_root(cpu: usize, root: u64) {
        use x86_64::registers::control::Cr3;

        let data = crate::per_cpu::cpu_data(cpu);
        // SAFETY: `cpu` is this CPU, found from its APIC id; the field is only
        // written by this CPU, and NMIs do not nest.
        let saved = unsafe { (&raw const (*data).saved_process_cr3).read_volatile() };
        if names_root(saved, root & ROOT_MASK) {
            return;
        }

        let root = root & ROOT_MASK;
        let (loaded, flags) = Cr3::read();
        if !names_root(loaded.start_address().as_u64(), root) {
            return;
        }
        if let Some(kernel) = crate::memory::kernel_page_table::master_kernel_pml4() {
            // SAFETY: the master kernel PML4 maps every kernel address the
            // interrupted code can be using; a dead process's user half is
            // what this CPU stops seeing.
            unsafe { Cr3::write(kernel, flags) };
            // Only while the announcement still names `root`: the interrupted
            // code may have announced the root it is about to load, and that
            // announcement stays true once it does.
            let _ = LOADED_ROOT[cpu].compare_exchange(
                root,
                kernel.start_address().as_u64() | KERNEL_ROOT_LOADED,
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
        }
    }
}
