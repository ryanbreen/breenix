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
//! A flush of one user address space (`flush_user_page`) therefore goes only to
//! the CPUs whose announced root, `next_cr3` or `saved_process_cr3` names that
//! space; an unmap by a process running alone costs no NMI. A flush that names
//! no address space (`flush_page`, `flush_all`: kernel mappings, or callers
//! that do not say whose table they changed) asks every other online CPU. The
//! receiver skips work that cannot apply to it: an address-space release is a
//! no-op on a CPU that does not have that root loaded.
//!
//! Requests are delivered as NMIs. The kernel spins on many locks with
//! interrupts masked; a fixed-vector IPI to a CPU spinning on a lock the
//! initiator holds would never be taken, and the initiator would wait forever.
//! An NMI is taken whatever RFLAGS.IF says. The handler uses no lock, no GS
//! base and no allocation: it finds its CPU from its local APIC id.
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
/// check, as the retirement proof already does. Returns after every target
/// has acknowledged.
#[cfg(target_arch = "x86_64")]
pub fn release_root_on_other_cpus(root: u64) {
    shootdown::request(shootdown::Request::ReleaseRoot(root));
}

#[cfg(target_arch = "x86_64")]
pub use shootdown::shootdown_nmi_handler;

#[cfg(target_arch = "x86_64")]
mod shootdown {
    use core::sync::atomic::{fence, AtomicBool, AtomicU64, AtomicU8, Ordering};

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

    /// Whether `cpu` may hold a translation of the address space `root`: it
    /// announced that root (or none yet), the interrupt-return stub is about
    /// to load it (`next_cr3`), or the stub restores it (`saved_process_cr3`).
    fn may_hold(cpu: usize, root: u64) -> bool {
        let loaded = LOADED_ROOT[cpu].load(Ordering::SeqCst);
        if loaded == UNKNOWN_ROOT || loaded == root {
            return true;
        }
        let names_root = |value: u64| value != 0 && (value & ROOT_MASK) == root;
        let data = crate::per_cpu::cpu_data(cpu);
        if data.is_null() {
            return true;
        }
        // SAFETY: `data` is CPU `cpu`'s slot of the static per-CPU array; the
        // two fields are word-sized and read without tearing.
        let (next, saved) = unsafe {
            (
                (&raw const (*data).next_cr3).read_volatile(),
                (&raw const (*data).saved_process_cr3).read_volatile(),
            )
        };
        names_root(next) || names_root(saved)
    }

    /// Held by the one CPU whose request is in flight.
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
    static REQUEST_KIND: AtomicU8 = AtomicU8::new(0);
    static REQUEST_ARG: AtomicU64 = AtomicU64::new(0);
    /// Set by the initiator for each target; cleared by the target once its
    /// invalidation is done.
    static PENDING: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

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

        x86_64::instructions::interrupts::without_interrupts(|| {
            use crate::arch_impl::PerCpuOps;
            let me = crate::arch_impl::x86_64::percpu::X86PerCpu::cpu_id() as usize;

            let targets = (0..MAX_CPUS)
                .filter(|&cpu| cpu != me && smp::is_cpu_online(cpu) && targeted(cpu))
                .fold(0u64, |mask, cpu| mask | 1 << cpu);
            if targets == 0 {
                return;
            }

            // Waiting here with interrupts masked is safe: the holder's NMI
            // still reaches this CPU and is answered.
            while IN_FLIGHT
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }

            REQUEST_KIND.store(kind, Ordering::Relaxed);
            REQUEST_ARG.store(arg, Ordering::Relaxed);

            for cpu in (0..MAX_CPUS).filter(|cpu| targets & (1 << cpu) != 0) {
                PENDING[cpu].store(true, Ordering::Relaxed);
            }
            // The request words and every PENDING flag are visible before any
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
                while PENDING[cpu].load(Ordering::Acquire) {
                    if crate::arch_impl::x86_64::timer::rdtsc() > deadline {
                        panic!(
                            "TLB shootdown: CPU {} did not acknowledge within {} s",
                            cpu, ACK_TIMEOUT_SECONDS
                        );
                    }
                    core::hint::spin_loop();
                }
            }

            IN_FLIGHT.store(false, Ordering::Release);
        });
    }

    /// The NMI handler: carry out the request addressed to this CPU, if any,
    /// and acknowledge it.
    ///
    /// No lock, no allocation, no GS-relative access: the NMI may have
    /// interrupted any instruction, including an entry stub before its GS
    /// handling. The CPU is found from its local APIC id.
    pub extern "x86-interrupt" fn shootdown_nmi_handler(_frame: InterruptStackFrame) {
        let cpu = if apic::active() {
            smp::cpu_of_apic_id(apic::id())
        } else {
            None
        };
        let Some(cpu) = cpu.filter(|&cpu| PENDING[cpu].load(Ordering::Acquire)) else {
            UNCLAIMED_NMIS.fetch_add(1, Ordering::Relaxed);
            return;
        };

        let arg = REQUEST_ARG.load(Ordering::Relaxed);
        match REQUEST_KIND.load(Ordering::Relaxed) {
            KIND_PAGE => x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(arg)),
            KIND_ALL => x86_64::instructions::tlb::flush_all(),
            KIND_RELEASE_ROOT => release_root(cpu, arg),
            _ => {}
        }

        PENDING[cpu].store(false, Ordering::Release);
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

        let names_root = |value: u64| value != 0 && (value & !0xfff) == (root & !0xfff);

        let data = crate::per_cpu::cpu_data(cpu);
        // SAFETY: `cpu` is this CPU, found from its APIC id; the field is only
        // written by this CPU, and NMIs do not nest.
        let saved = unsafe { (&raw const (*data).saved_process_cr3).read_volatile() };
        if names_root(saved) {
            return;
        }

        let (loaded, flags) = Cr3::read();
        if names_root(loaded.start_address().as_u64()) {
            if let Some(kernel) = crate::memory::kernel_page_table::master_kernel_pml4() {
                note_root_load(cpu, kernel.start_address().as_u64());
                // SAFETY: the master kernel PML4 maps every kernel address the
                // interrupted code can be using; a dead process's user half is
                // what this CPU stops seeing.
                unsafe { Cr3::write(kernel, flags) };
            }
        }
    }
}
