//! Shared TTBR0 transition helpers for AArch64 teardown paths.

use core::sync::atomic::{AtomicU64, Ordering};

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;

/// Bits [63:48] of a TTBR0 value: the ASID field the corridor installs verbatim.
const TTBR0_ASID_MASK: u64 = 0xFFFF_0000_0000_0000;

/// The ASID userspace runs under, positioned in TTBR0 bits [63:48].
///
/// `set_next_ttbr0_for_thread` in
/// `kernel/src/arch_impl/aarch64/context_switch.rs` tags the root it hands the
/// dispatch path with ASID 1, for the reason stated there: the boot identity
/// map's TLB entries are ASID 0, and combined with the nG bits on process
/// page-table entries a non-zero ASID is what keeps a user VA from matching one
/// of them.
/// claim-lint:ok: 1 of 1 site in the kernel tags a root before publishing it
/// into `next_cr3` -- `set_next_ttbr0_for_thread`, which since R157/ASID-05
/// applies the tag by calling `process_root_ttbr0` rather than by or-ing a
/// literal; that routing is pinned by
/// `the_discipline_publishes_the_dispatch_asid` in
/// `tests/ttbr0_shadow_reconciliation_structure.rs`.
///
/// It is not only the register that has to carry it. `switch_ttbr0_if_needed`
/// publishes the tagged value into `saved_process_cr3`, and the
/// `.Lrestore_saved_ttbr` arm of `syscall_entry.S` installs that word verbatim,
/// ASID bits included -- so a site that publishes an untagged root decides that
/// the next return to EL0 runs on ASID 0.
/// claim-lint:ok: the dispatch tag is now this constant, reached through
/// `process_root_ttbr0`, and the corridor install is `syscall_entry.S`'s
/// `.Lrestore_saved_ttbr`; the routing is pinned by
/// `the_discipline_publishes_the_dispatch_asid` and the count of places that
/// construct the tag by `the_asid_tag_is_constructed_in_one_place`, both in
/// `tests/ttbr0_shadow_reconciliation_structure.rs`.
pub(crate) const USER_ASID_TTBR0: u64 = 1 << 48;

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
/// and the userspace ASID does not apply to it.
static ASID_PUBLISH_KERNEL: AtomicU64 = AtomicU64::new(0);
/// A publish of a process root carrying the userspace ASID.
static ASID_PUBLISH_TAGGED: AtomicU64 = AtomicU64::new(0);
/// A publish of a process root whose ASID field is NOT the userspace ASID.
/// This is the defect class: the corridor installs the word verbatim.
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
    if value & TTBR0_ASID_MASK == USER_ASID_TTBR0 {
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
    crate::serial_println!(
        "[TTBR0_ASID_CENSUS:untagged={}:tagged={}:kernel={}:cleared={}]",
        ASID_PUBLISH_UNTAGGED.load(Ordering::Relaxed),
        ASID_PUBLISH_TAGGED.load(Ordering::Relaxed),
        ASID_PUBLISH_KERNEL.load(Ordering::Relaxed),
        ASID_PUBLISH_CLEARED.load(Ordering::Relaxed),
    );
}

/// Normalise a process page-table root to the ASID userspace runs under.
///
/// The ASID field is REPLACED rather than or-ed into: an or-only tag would
/// preserve a foreign ASID a caller happened to hand over, and this kernel has
/// exactly one userspace ASID. `TTBR0_ROOT_MASK` clears bits [63:48] and
/// [11:0], so what survives is the root physical address the caller chose.
#[inline(always)]
pub(crate) fn process_root_ttbr0(root: u64) -> u64 {
    (root & TTBR0_ROOT_MASK) | USER_ASID_TTBR0
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

/// Return the kernel TTBR0 root, falling back to the boot identity table before
/// per-CPU state has been populated.
#[inline(always)]
pub fn kernel_ttbr0() -> u64 {
    let ttbr0 = crate::per_cpu_aarch64::get_kernel_cr3();
    if ttbr0 == 0 {
        0x4200_0000
    } else {
        ttbr0
    }
}

/// Switch TTBR0 to the kernel page table and invalidate stale translations.
///
/// Exit, exec, and fault cleanup all use this implementation so none of them
/// can retire a process page-table root while the CPU still has it installed.
///
/// The block carries `nostack` and deliberately not `nomem`. `nomem` tells the
/// compiler the asm reads and writes no memory, which is a licence to move
/// memory accesses across the block -- including the per-CPU shadow stores a
/// caller makes around this install: `quiesce_ttbr0_for_exit` zeroes both
/// words immediately after it. Without `nomem` the compiler must assume the
/// block may read or write that same memory, so it keeps those accesses on the
/// side of the block the source puts them on. That constrains the compiler
/// only. No instruction is added, and no claim is made about what another CPU
/// observes: the hardware ordering is still the `dsb ishst` before the `msr`
/// and the `dsb ish; isb` after.
#[inline(always)]
pub fn switch_ttbr0_to_kernel() {
    let ttbr0 = kernel_ttbr0();

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0,
            options(nostack)
        );
    }
}

/// Install `ttbr0_value` as this CPU's running process root and leave the
/// per-CPU TTBR0 shadows describing what the register now holds.
///
/// The shadows are not bookkeeping: `saved_process_cr3` (per-CPU offset 80) and
/// `next_cr3` (offset 64) are both read by the syscall return corridor in
/// `syscall_entry.S`, which installs `next_cr3` when it is non-zero and
/// otherwise restores `saved_process_cr3`, and both are read by
/// `is_ttbr0_root_live_in_mask` when a page-table root is considered for
/// reclamation. A site that writes the register with a raw `msr` and leaves the
/// shadows alone is therefore not "just" out of sync -- it lets the next return
/// to EL0 install whatever root the shadows still name.
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
/// The asm block carries `nostack` and deliberately not `nomem`. The two
/// shadow stores below are the memory this install has to stay ordered
/// against, and a caller's page-table stores are the memory that has to be
/// settled before it; `nomem` would tell the compiler the block reads and
/// writes no memory, leaving it free to move either across the barriers.
/// Without `nomem` the compiler must assume the block may read or write that
/// same memory, so it keeps those accesses on the side of the block the source
/// puts them on. The change constrains the compiler only. No instruction is
/// added, and no claim is made about what another CPU observes: the hardware
/// ordering is still the `dsb ishst` before the `msr` and the `dsb ish; isb`
/// after.
#[inline(always)]
pub fn adopt_process_ttbr0(ttbr0_value: u64) {
    // Both the register and the shadow published below carry the ASID
    // userspace is dispatched under. Normalising here rather than at each
    // routed call site makes the ASID a property of the discipline instead of
    // one each caller has to remember, and the field is not the caller's to
    // choose in any case.
    // claim-lint:ok: of the 10 routed process-root install decision sites, all
    // 10 hand over a root with an empty ASID field since R157/ASID-05; the
    // 2 that did not -- `launch_init_from_elf`, which or-ed ASID 1 in, and
    // `switch_to_process_page_table`, which or-ed back whatever ASID the
    // register held -- were changed in that round to hand over the root alone.
    // The sites are enumerated in
    // docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md
    // and the change is recorded in
    // docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md
    let ttbr0_value = process_root_ttbr0(ttbr0_value);

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0_value,
            options(nostack)
        );
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(ttbr0_value);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Re-install the root the CALLING thread's own process already owns, on the
/// way out of a blocking syscall, and leave the corridor words naming it.
///
/// This is the blocking-resume disposition and it is not the same decision as
/// adopting a root. The caller has changed no mapping and chosen no new
/// address space: it blocked, something else may have run on this CPU, and it
/// needs the register and the two shadow words to name its own root again
/// before the return to EL0. So the register write -- and with it the
/// inner-shareable broadcast invalidation -- is skipped when TTBR0 already
/// holds exactly this root under this ASID, the same guard the dispatch path's
/// `switch_ttbr0_if_needed` and `switch_to_process_page_table` already apply.
///
/// The shadow publication is NOT guarded, and that asymmetry is the point.
/// The register can be right while `next_cr3` still holds a pending redirect
/// the corridor would apply first, which is #786 exactly; the words are the
/// only thing that can be stale on the skip arm, so both are written on both
/// arms.
///
/// Why skipping the invalidation is safe HERE and is not offered to the adopt
/// path: a root can only be reclaimed once no CPU shadow names it
/// (`is_ttbr0_root_live_in_mask`), and every transition off a root on any CPU
/// runs a broadcast `tlbi vmalle1is`. For this CPU's register to still hold
/// the resuming thread's root, this CPU installed it and installed nothing
/// since, so its shadow still names it and the frame cannot have been
/// reclaimed and re-issued underneath. That argument is about root REUSE. It
/// is not an argument that a concurrent unmap under the same root is covered
/// -- TLB maintenance for a mapping change belongs to the code that changes
/// the mapping -- and the corridor's own unconditional invalidation on every
/// syscall return is unaffected by this guard either way.
/// claim-lint:ok: 2 of 2 sibling guards are cited by path --
/// `switch_ttbr0_if_needed` in
/// `kernel/src/arch_impl/aarch64/context_switch.rs` and
/// `switch_to_process_page_table` in `kernel/src/memory/process_memory.rs`.
#[inline(always)]
pub fn restore_process_ttbr0(root: u64) {
    let root = process_root_ttbr0(root);

    if local_ttbr0() == root {
        unsafe {
            super::percpu::Aarch64PerCpu::set_saved_process_cr3(root);
            super::percpu::Aarch64PerCpu::set_next_cr3(0);
        }
        return;
    }

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
