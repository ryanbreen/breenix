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
//! The target set is every other online CPU. A CPU may hold a translation for
//! any address space it has run since its last CR3 write, and x86 lets kernel
//! threads run on whatever CR3 the previous thread left loaded, so the
//! initiator cannot know which CPUs hold a given address space without
//! tracking every CR3 write; it asks all of them. The receiver skips work that
//! cannot apply to it: an address-space release is a no-op on a CPU that does
//! not have that root loaded.
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

            let mut targets = 0u64;
            for cpu in 0..MAX_CPUS {
                if cpu != me && smp::is_cpu_online(cpu) {
                    PENDING[cpu].store(true, Ordering::Relaxed);
                    targets |= 1 << cpu;
                }
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
                // SAFETY: the master kernel PML4 maps every kernel address the
                // interrupted code can be using; a dead process's user half is
                // what this CPU stops seeing.
                unsafe { Cr3::write(kernel, flags) };
            }
        }
    }
}
