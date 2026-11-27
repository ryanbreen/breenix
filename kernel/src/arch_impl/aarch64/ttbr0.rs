//! Shared TTBR0 transition helpers for AArch64, and the per-address-space
//! ASIDs those transitions install.
//!
//! Each user address space runs under its own ASID (`super::asid`), so an
//! install that changes address space needs no TLB invalidation: the new
//! ASID cannot match the old space's non-global entries. User leaves are
//! non-global (`memory::arch_stub`), and so are the kernel root's TTBR0
//! identity blocks, which run under `asid::KERNEL_ASID` (boot.S and the UEFI
//! loader both build them with nG). The only invalidation left on a switch is
//! the local one a CPU takes once after a generation rollover.

use core::sync::atomic::{AtomicU64, Ordering};

use super::asid::{AsidSpace, Binding, KERNEL_ASID};

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;

/// Bits [63:48] of a TTBR0 value: the ASID field the corridor installs verbatim.
const TTBR0_ASID_MASK: u64 = 0xFFFF_0000_0000_0000;
const TTBR0_ASID_SHIFT: u32 = 48;

/// TCR_EL1.AS: use the 16-bit TTBR ASID field rather than its low 8 bits.
const TCR_AS: u64 = 1 << 36;
/// TCR_EL1.A1: take the ASID from TTBR1 instead of TTBR0. Must stay clear.
const TCR_A1: u64 = 1 << 22;

/// The ASID allocator and the registry of live process roots. It is an
/// `IrqSafeMutex`, so a holder has local interrupts masked, and it is a leaf:
/// its holders take no other lock.
/// claim-lint:ok: 7 of 7 `ASIDS.lock()` sites are in this file and call only `AsidSpace` methods under it
static ASIDS: crate::irq_safe_mutex::IrqSafeMutex<Option<AsidSpace>> =
    crate::irq_safe_mutex::IrqSafeMutex::new(None);

/// ASID width in use, for diagnostics: 0 until `init_asids` runs.
static ASID_BITS: AtomicU64 = AtomicU64::new(0);
/// Local TLB flushes taken because a generation rolled over.
static ASID_ROLLOVER_FLUSHES: AtomicU64 = AtomicU64::new(0);
/// Process roots left unregistered because the registry could not grow.
static ASID_REGISTRY_REFUSED: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn asid_field(value: u64) -> u16 {
    (value >> TTBR0_ASID_SHIFT) as u16
}

/// Choose the ASID width and create the allocator. CPU 0 calls this once,
/// after the heap and before any process exists or any secondary CPU copies
/// TCR_EL1 (`smp::set_smp_ttbrs`).
///
/// ID_AA64MMFR0_EL1.ASIDBits (bits [7:4]) is 0b0010 when 16-bit ASIDs are
/// implemented and 0b0000 for 8 bits. 16-bit ASIDs are used only with
/// TCR_EL1.AS set; until then the upper byte of the field is ignored. The
/// ASID is taken from TTBR0 (TCR_EL1.A1 clear).
pub fn init_asids() {
    let mmfr0: u64;
    let tcr: u64;
    unsafe {
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack));
        core::arch::asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack));
    }
    let bits = if (mmfr0 >> 4) & 0xF == 0b0010 { 16 } else { 8 };
    let wanted = if bits == 16 { (tcr & !TCR_A1) | TCR_AS } else { tcr & !(TCR_A1 | TCR_AS) };
    if wanted != tcr {
        // A change to how TLB entries are tagged: drop this CPU's TLB after
        // it. No other CPU is online and no user ASID has run.
        unsafe {
            core::arch::asm!(
                "msr tcr_el1, {}",
                "isb",
                "tlbi vmalle1",
                "dsb nsh",
                "isb",
                in(reg) wanted,
                options(nostack)
            );
        }
    }
    let space = AsidSpace::new(bits, super::constants::MAX_CPUS);
    *ASIDS.lock() = Some(space);
    ASID_BITS.store(bits as u64, Ordering::Relaxed);
}

/// Run `step` on the allocator until it stops asking for registry storage,
/// growing the registry outside the lock in between: the lock is taken on
/// the switch path, so the allocation happens with it released.
fn with_registry_room(
    step: impl Fn(&mut AsidSpace) -> Result<(), super::asid::NeedsCapacity>,
) -> Result<(), &'static str> {
    loop {
        let needed = match ASIDS.lock().as_mut() {
            None => return Ok(()),
            Some(space) => match step(space) {
                Ok(()) => return Ok(()),
                Err(need) => need.capacity,
            },
        };
        let mut storage = alloc::vec::Vec::new();
        storage
            .try_reserve_exact(needed)
            .map_err(|_| "no memory for the ASID registry")?;
        let unused = ASIDS.lock().as_mut().map(|space| space.grow(storage));
        drop(unused);
    }
}

/// Make room to register one more process root. Called before the root frame
/// is allocated, so a failure here has no frame to give back.
pub fn reserve_address_space() -> Result<(), &'static str> {
    with_registry_room(|space| space.has_room())
}

/// Record a new process root. It gets an ASID when it is first installed.
///
/// After `reserve_address_space` this fails only if other registrations took
/// the room in between and growing again found no memory. The root then
/// stays unregistered, which is slower but still correct: each bind gives it
/// a one-off ASID, and its invalidations are not narrowed to one ASID. The
/// count is in the `[ASID:...]` line.
pub fn register_address_space(root: u64) {
    let root = root & TTBR0_ROOT_MASK;
    if with_registry_room(|space| space.register(root)).is_err() {
        ASID_REGISTRY_REFUSED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Forget a process root before its frame can be reissued. A CPU no longer
/// names it (`is_ttbr0_root_live`), and its ASID stays out of use until the
/// generation ends, so no TLB maintenance is needed here.
pub fn retire_address_space(root: u64) {
    if let Some(space) = ASIDS.lock().as_mut() {
        space.unregister(root & TTBR0_ROOT_MASK);
    }
}

/// The ASID `root` runs under now, for invalidation targeted at that space.
/// A space with no ASID in the current generation gets no target, and the
/// caller invalidates across ASIDs instead.
///
/// Ordering: the caller has already written the descriptors it is about to
/// invalidate. The full `dsb ish` makes those writes complete before the
/// context is read, so a CPU that binds this space a new ASID afterwards (it
/// does so under the same lock) walks the new descriptors.
pub fn user_asid(root: u64) -> Option<u16> {
    unsafe {
        core::arch::asm!("dsb ish", options(nostack));
    }
    ASIDS.lock().as_ref()?.current_asid(root & TTBR0_ROOT_MASK)
}

/// The ASIDs `cpu`'s two TTBR0 shadow words carry: the return corridor may
/// install either without binding again.
fn shadow_asids(cpu: usize) -> [u16; 2] {
    crate::per_cpu_aarch64::ttbr0_shadow_snapshot(cpu)
        .map_or([KERNEL_ASID; 2], |(saved, next)| [asid_field(saved), asid_field(next)])
}

/// Bind `root` to an ASID for THIS CPU and return the TTBR0 value to install.
///
/// The caller has local interrupts masked from this call until the value is
/// installed or published in this CPU's shadow words, so the ASID is used on
/// the CPU it was bound for. When the allocator asks for it (a generation
/// rolled over since this CPU last bound), this CPU's TLB is invalidated
/// locally -- `tlbi vmalle1`, not the inner-shareable broadcast -- before the
/// value can be installed.
#[inline(always)]
pub(crate) fn bind_process_root(root: u64) -> u64 {
    let root = root & TTBR0_ROOT_MASK;
    let cpu = super::percpu::Aarch64PerCpu::cpu_id() as usize;
    let binding = ASIDS.lock().as_mut().map(|space| space.bind(cpu, root, &shadow_asids));
    let Binding { asid, flush_local } = match binding {
        Some(binding) => binding,
        None => {
            // Before `init_asids` no process exists; keep the shared tag and
            // the broadcast invalidation the kernel used before ASIDs.
            unsafe {
                core::arch::asm!("dsb ishst", "tlbi vmalle1is", "dsb ish", "isb", options(nostack));
            }
            return root | (1 << TTBR0_ASID_SHIFT);
        }
    };
    if flush_local {
        ASID_ROLLOVER_FLUSHES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            core::arch::asm!("tlbi vmalle1", "dsb nsh", "isb", options(nostack));
        }
    }
    root | ((asid as u64) << TTBR0_ASID_SHIFT)
}

/// Emit the ASID allocator's state from normal context.
pub fn emit_asid_state() {
    let (generation, rollovers, registered, unregistered) = match ASIDS.lock().as_ref() {
        Some(space) => (
            space.generation(),
            space.rollovers(),
            space.registered() as u64,
            space.unregistered_binds(),
        ),
        None => (0, 0, 0, 0),
    };
    crate::diag_println!(
        "[ASID:bits={}:generation={}:rollovers={}:local_flushes={}:spaces={}:refused={}:unregistered_binds={}]",
        ASID_BITS.load(Ordering::Relaxed),
        generation,
        rollovers,
        ASID_ROLLOVER_FLUSHES.load(Ordering::Relaxed),
        registered,
        ASID_REGISTRY_REFUSED.load(Ordering::Relaxed),
        unregistered,
    );
}

// ---------------------------------------------------------------------------
// Runtime census of what the corridor shadow words are handed (#786 follow-on)
// ---------------------------------------------------------------------------
//
// The structural ratchet in `tests/ttbr0_shadow_reconciliation_structure.rs`
// reads shapes: which function publishes, and whether the value it publishes
// was normalised in that same function. Nothing it can read says what the
// published WORD held on a running kernel, and the defect this census exists
// for was a value defect -- `adopt_process_ttbr0` published its caller's
// ASID-untagged root, so the `.Lrestore_saved_ttbr` arm of `syscall_entry.S`
// returned EL0 to ASID 0 while the dispatch path returned it to ASID 1.
// claim-lint:ok: the shape ratchet's own census -- 17 of 17 publishes and
// their dispositions -- is printed by
// `every_shadow_publish_has_an_accounted_asid -- --nocapture` in
// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/05-suite-green-with-census.txt
//
// So the shadow publishes are counted where they are WRITTEN -- both per-CPU
// setters in `super::percpu` call `note_shadow_publish` -- and sorted into the
// four dispositions a published word can legitimately have. The count is
// lock-free, allocation-free, does no formatting and takes no lock: four
// relaxed counters and, on the non-zero arm, one per-CPU read of this CPU's
// kernel root. `emit_asid_census` is the only part that prints, and it runs
// from normal context alongside the root-custody summary.
// claim-lint:ok: 2 of 2 per-CPU setters call it, pinned by
// `the_shadow_setters_feed_the_runtime_census` in
// `tests/ttbr0_shadow_reconciliation_structure.rs`
//
// WHERE THIS RUNS (R157/ASID-02 -- the round that added the counter said the
// opposite, and was wrong). `note_shadow_publish` is `#[inline(always)]` into
// both setters, and both setters ARE reached from the exception-return
// corridor. `syscall_entry.S` and `boot.S` each `bl
// check_need_resched_and_switch_arm64`; that function reaches
// `dispatch_thread_locked`, which calls `set_next_ttbr0_for_thread` and
// `switch_ttbr0_if_needed`, and `dispatch_idle_locked`, which calls
// `setup_idle_return_locked`. `handle_sync_exception` reaches
// `set_idle_stack_for_eret`. Every one of those publishes a shadow word, so
// every one of them now runs the counter. The `.S` files are untouched; the
// code they branch to is not. Saying otherwise is what licensed adding an RMW
// here without measuring it.
// claim-lint:ok: 4 of 4 corridor-reached publish sites are named in this paragraph,
// each appearing in the census table of docs/planning/green-
// program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md
//
// WHAT IT COSTS. Measured, not asserted: disassembling the shipped
// production-profile kernel with and without the two `note_shadow_publish`
// calls and diffing the instruction count of `switch_ttbr0_if_needed` -- the
// corridor-reached function that publishes twice -- is the arithmetic recorded
// in the round document. It is a static instruction count on one function, not
// a cycle measurement, and it does not measure LL/SC retry under contention.
// claim-lint:ok: the two objdumps, the counts and the delta are in
// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/08-corridor-instruction-delta.txt
//
// What this cannot see: the two stores `syscall_entry.S` makes itself. The
// entry path copies the live `ttbr0_el1` into `saved_process_cr3` and the
// return path clears `next_cr3` with `xzr`; both are in assembly on the
// syscall hot path, neither introduces a value the register did not already
// hold, and neither is counted here.
// claim-lint:ok: 2 of 2 assembly stores to the shadow offsets are in
// `kernel/src/arch_impl/aarch64/syscall_entry.S` (offset 80 from `ttbr0_el1`,
// offset 64 from `xzr`); the Rust-side writes all funnel through the two
// setters in `kernel/src/arch_impl/aarch64/percpu.rs`.

/// A publish of the literal 0: the corridor arm for that word is disarmed.
static ASID_PUBLISH_CLEARED: AtomicU64 = AtomicU64::new(0);
/// A publish of this CPU's kernel root. The kernel root runs under ASID 0 by
/// construction -- it is the boot identity map -- so it is not a process root
/// and no user ASID applies to it.
static ASID_PUBLISH_KERNEL: AtomicU64 = AtomicU64::new(0);
/// A publish of a process root carrying a user ASID (any but `KERNEL_ASID`).
static ASID_PUBLISH_TAGGED: AtomicU64 = AtomicU64::new(0);
/// A publish of a process root under the kernel ASID. This is the defect
/// class: the corridor installs the word verbatim, and the kernel root's
/// identity blocks are tagged with that ASID.
static ASID_PUBLISH_UNTAGGED: AtomicU64 = AtomicU64::new(0);

/// Count one publish into `saved_process_cr3` or `next_cr3`.
///
/// Called from both per-CPU setters, which is 2 of 2 Rust-side writes of
/// either word. No lock, no allocation, no formatting, no page-table walk: a
/// compare against this CPU's kernel root and one relaxed increment.
/// claim-lint:ok: the 2 setters and the ordering of the count before the
/// write are pinned by `the_shadow_setters_feed_the_runtime_census` in
/// `tests/ttbr0_shadow_reconciliation_structure.rs`
#[inline(always)]
pub(crate) fn note_shadow_publish(value: u64) {
    if value == 0 {
        ASID_PUBLISH_CLEARED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if value & TTBR0_ROOT_MASK == kernel_ttbr0() & TTBR0_ROOT_MASK {
        ASID_PUBLISH_KERNEL.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if value & TTBR0_ASID_MASK != (KERNEL_ASID as u64) << TTBR0_ASID_SHIFT {
        ASID_PUBLISH_TAGGED.fetch_add(1, Ordering::Relaxed);
    } else {
        ASID_PUBLISH_UNTAGGED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Emit the ASID census from normal context.
///
/// `untagged` is the field the prod-profile and strict gates fail on. The other
/// three are carried so the line can be read as evidence rather than as an
/// assertion about a counter that might not have been reached: a boot that
/// dispatched userspace has a large `tagged`, and a `tagged=0` line says the
/// census saw no process-root publish at all.
/// claim-lint:ok: 3 of 3 production boots end at `tagged` above 19000 with
/// `untagged=0`, in
/// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/04-prod-boot1-serial.txt
/// and its 2 siblings
pub fn emit_asid_census() {
    crate::diag_println!(
        "[TTBR0_ASID_CENSUS:untagged={}:tagged={}:kernel={}:cleared={}]",
        ASID_PUBLISH_UNTAGGED.load(Ordering::Relaxed),
        ASID_PUBLISH_TAGGED.load(Ordering::Relaxed),
        ASID_PUBLISH_KERNEL.load(Ordering::Relaxed),
        ASID_PUBLISH_CLEARED.load(Ordering::Relaxed),
    );
}

#[inline(always)]
pub(crate) fn roots_match(left: u64, right: u64) -> bool {
    let left = left & TTBR0_ROOT_MASK;
    left != 0 && left == right & TTBR0_ROOT_MASK
}

/// Read this CPU's architectural TTBR0 whole, ASID field included.
#[inline(always)]
fn local_ttbr0() -> u64 {
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }
    ttbr0
}

/// Read this CPU's architectural TTBR0 root, with the ASID field masked off.
#[inline(always)]
pub(crate) fn local_ttbr0_root() -> u64 {
    local_ttbr0() & TTBR0_ROOT_MASK
}

/// Return the kernel TTBR0 root, falling back to the live root before
/// per-CPU state has been populated.
#[inline(always)]
pub fn kernel_ttbr0() -> u64 {
    let ttbr0 = crate::per_cpu_aarch64::get_kernel_cr3();
    if ttbr0 == 0 {
        local_ttbr0_root()
    } else {
        ttbr0
    }
}

/// Switch TTBR0 to the kernel page table.
///
/// Exit, exec, and fault cleanup all use this implementation so none of them
/// can retire a process page-table root while the CPU still has it installed.
///
/// No TLB invalidation: the kernel root runs under `KERNEL_ASID`, and the
/// process root this CPU leaves keeps its own ASID, which no other address
/// space is given until a generation rollover, after which each CPU drops its
/// TLB before running a new-generation ASID. Its entries therefore cannot
/// translate for another space, and a root is retired only once no CPU's
/// shadow words name it (`is_ttbr0_root_live_in_mask`).
///
/// The block carries `nostack` and deliberately not `nomem`. `nomem` tells the
/// compiler the asm reads and writes no memory, which is a licence to move
/// memory accesses across the block -- including the per-CPU shadow stores a
/// caller makes around this install: `quiesce_ttbr0_for_exit` zeroes both
/// words immediately after it. Without `nomem` the compiler must assume the
/// block may read or write that same memory, so it keeps those accesses on the
/// side of the block the source puts them on. That constrains the compiler
/// only: the hardware ordering is the `dsb ishst` before the `msr` (page-table
/// stores complete before the new root can be walked) and the `isb` after,
/// which makes the new root and ASID the context of later instructions.
#[inline(always)]
pub fn switch_ttbr0_to_kernel() {
    let ttbr0 = kernel_ttbr0();

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            ttbr0 = in(reg) ttbr0,
            options(nostack)
        );
    }
}

/// Mask IRQ and FIQ on this CPU, returning the DAIF to restore. A bind is
/// valid only on the CPU that made it, so a bind and the install or publish
/// of its result happen with no chance to migrate in between.
#[inline(always)]
fn mask_local_interrupts() -> u64 {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", "msr daifset, #3", out(reg) daif, options(nomem, nostack));
    }
    daif
}

#[inline(always)]
fn restore_local_interrupts(daif: u64) {
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack));
    }
}

/// Install `ttbr0_value`'s root as this CPU's running process root, under the
/// ASID the allocator binds it to, and leave the per-CPU TTBR0 shadows
/// describing what the register now holds.
///
/// The shadows are not bookkeeping: `saved_process_cr3` (per-CPU offset 80) and
/// `next_cr3` (offset 64) are both read by the syscall return corridor in
/// `syscall_entry.S`, which installs `next_cr3` when it is non-zero and
/// otherwise restores `saved_process_cr3`, and both are read by
/// `is_ttbr0_root_live_in_mask` when a page-table root is considered for
/// reclamation, and by an ASID rollover, which keeps the ASIDs they carry.
/// A site that writes the register with a raw `msr` and leaves the shadows
/// alone is therefore not "just" out of sync -- it lets the next return to
/// EL0 install whatever root the shadows still name.
/// claim-lint:ok: the corridor reads are cited in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
///
/// That is issue #786: `launch_init_from_elf` installed init's root with a raw
/// `msr`, the shadows still held the kernel root a preceding idle redirect had
/// published into `next_cr3`, and init's first `svc` returned onto the kernel
/// root -- an instruction abort at init's own return address (ESR
/// `0x8200000e`, a level-2 permission fault: the page is mapped by the kernel
/// root's identity map but is not EL0-executable).
/// claim-lint:ok: the corridor reads and the 7-function install census are in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
///
/// Clearing `next_cr3` is part of the install, not a separate courtesy: after
/// this call the architectural register is the decision, so a pending "switch
/// to some other root on the way out" request is either the same root or a
/// stale one, and applying either on the return path is wrong.
///
/// The ASID field of `ttbr0_value` is ignored: the allocator chooses it
/// (`bind_process_root`). Interrupts stay masked from the bind to the shadow
/// stores, so the ASID is installed on the CPU it was bound for. The install
/// issues no TLB invalidation: the new ASID cannot match another space's
/// entries, and a pending rollover flush was taken by the bind.
///
/// The asm block carries `nostack` and deliberately not `nomem`. The two
/// shadow stores below are the memory this install has to stay ordered
/// against, and a caller's page-table stores are the memory that has to be
/// settled before it; `nomem` would tell the compiler the block reads and
/// writes no memory, leaving it free to move either across the barriers.
/// The hardware ordering is the `dsb ishst` before the `msr` and the `isb`
/// after.
#[inline(always)]
pub fn adopt_process_ttbr0(ttbr0_value: u64) {
    let daif = mask_local_interrupts();
    let ttbr0_value = bind_process_root(ttbr0_value);

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            ttbr0 = in(reg) ttbr0_value,
            options(nostack)
        );
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(ttbr0_value);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
    restore_local_interrupts(daif);
}

/// Re-install the root the CALLING thread's own process already owns, on the
/// way out of a blocking syscall, and leave the corridor words naming it.
///
/// This is the blocking-resume disposition and it is not the same decision as
/// adopting a root. The caller has changed no mapping and chosen no new
/// address space: it blocked, something else may have run on this CPU, and it
/// needs the register and the two shadow words to name its own root again
/// before the return to EL0. So the register write is skipped when TTBR0
/// already holds exactly this root under the ASID it is bound to now, the
/// same guard the dispatch path's `switch_ttbr0_if_needed` and
/// `switch_to_process_page_table` already apply.
///
/// The shadow publication is NOT guarded, and that asymmetry is the point.
/// The register can be right while `next_cr3` still holds a pending redirect
/// the corridor would apply first, which is #786 exactly; the words are the
/// only thing that can be stale on the skip arm, so both are written on both
/// arms.
///
/// The bind runs on the skip arm too: it is what records that this CPU runs
/// the space now, and what takes a pending rollover flush. TLB maintenance
/// for a mapping change belongs to the code that changes the mapping.
/// claim-lint:ok: 2 of 2 sibling guards are cited by path --
/// `switch_ttbr0_if_needed` in
/// `kernel/src/arch_impl/aarch64/context_switch.rs` and
/// `switch_to_process_page_table` in `kernel/src/memory/process_memory.rs`.
#[inline(always)]
pub fn restore_process_ttbr0(root: u64) {
    let daif = mask_local_interrupts();
    let bound = bind_process_root(root);

    if local_ttbr0() == bound {
        unsafe {
            super::percpu::Aarch64PerCpu::set_saved_process_cr3(bound);
            super::percpu::Aarch64PerCpu::set_next_cr3(0);
        }
        restore_local_interrupts(daif);
        return;
    }
    restore_local_interrupts(daif);

    adopt_process_ttbr0(root);
}

/// Leave the current userspace root and prevent an exception-return path from
/// reinstalling it. This must complete before publishing deferred exit work.
#[inline(always)]
pub fn quiesce_ttbr0_for_exit() {
    switch_ttbr0_to_kernel();
    unsafe {
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(0);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Return whether any online CPU still retains `root_phys` in a TTBR0 shadow.
///
/// TTBR0 values may carry an ASID, so compare only the physical root bits using
/// the same mask as the exception fault lookup paths.
pub fn is_ttbr0_root_live(root_phys: u64) -> bool {
    let mut online_mask = 0;
    for cpu_id in 0..super::constants::MAX_CPUS {
        if super::smp::is_cpu_online(cpu_id) {
            online_mask |= 1 << cpu_id;
        }
    }
    is_ttbr0_root_live_in_mask(root_phys, online_mask)
}

/// Return whether a CPU captured by `online_mask` retains `root_phys` in a
/// TTBR0 shadow. Remote hardware TTBR0 is not architecturally readable.
pub(crate) fn is_ttbr0_root_live_in_mask(root_phys: u64, online_mask: u64) -> bool {
    let root_phys = root_phys & TTBR0_ROOT_MASK;
    if root_phys == 0 {
        return false;
    }
    (0..super::constants::MAX_CPUS).any(|cpu_id| {
        if online_mask & (1 << cpu_id) == 0 {
            return false;
        }

        crate::per_cpu_aarch64::ttbr0_shadow_snapshot(cpu_id)
            .map(|(saved_process_ttbr0, next_ttbr0)| {
                roots_match(saved_process_ttbr0, root_phys) || roots_match(next_ttbr0, root_phys)
            })
            .unwrap_or(false)
    })
}
