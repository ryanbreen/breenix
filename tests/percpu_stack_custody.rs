//! The per-CPU stack custody rule, EXECUTED.
//!
//! Every ratchet this campaign has written about custody pins a shape around
//! the predicate — which writers consult it, which pivots are adjudicated,
//! which identity the decision is made with. The predicate itself, the thing
//! all of them delegate to, reddened nothing: deleting the `slot == cpu`
//! conjunct or inverting it left every host suite green. That was round-2 N1,
//! round-3 N1 and round-4 N3, three times recorded as still open.
//!
//! It stayed open because the decision lived in `constants.rs`, next to
//! `read_volatile` and the platform base, and can only be built for the target.
//! The arithmetic now lives in `kernel/src/arch_impl/aarch64/percpu_custody.rs`,
//! which reaches nothing — the tests below include that file directly and run
//! the real function, not a copy of it. The link back (`constants.rs` must
//! delegate to this module rather than re-inline the rule) is pinned by
//! `context_restore_structure.rs`, so the coverage cannot be orphaned by
//! rewriting the kernel-side predicate.

#[path = "../kernel/src/arch_impl/aarch64/percpu_custody.rs"]
mod custody;

use custody::{
    decode_owner_record, slot_admits, slot_of, BOUNDARY_CANARY_OFFSET, OVERRUN_SENTINEL_OFFSET,
    OWNER_RECORD_BYTES, OWNER_RECORD_OFFSET, RESERVED_HALF_BOTTOM_BYTES,
};

/// The kernel's aarch64 geometry, mirrored here so the cases read like the
/// addresses in the serials.
const BASE: u64 = 0xFFFF_0000_4300_0000;
const STRIDE_SHIFT: u32 = 21;
const STRIDE: u64 = 1 << STRIDE_SHIFT;
const MAX_CPUS: usize = 8;
const REGION_SIZE: u64 = MAX_CPUS as u64 * STRIDE;
const MAGIC: u64 = 0x5354_4B4F_574E_4552;

fn slot(addr: u64) -> Option<usize> {
    slot_of(BASE, REGION_SIZE, STRIDE_SHIFT, addr)
}

/// `percpu_kernel_stack_top(cpu)` — the exclusive top of slot `cpu`.
fn kernel_stack_top(cpu: usize) -> u64 {
    BASE + (cpu as u64 + 1) * STRIDE
}

/// A published record, exactly as `publish_percpu_stack_owner` writes it.
fn published(owner: usize) -> (u64, u64) {
    (MAGIC ^ owner as u64, owner as u64)
}

/// The claim a slot with a published owner reports.
fn claim_of(owner: usize) -> impl FnOnce(usize) -> Option<usize> {
    move |_| Some(owner)
}

fn no_claim(_: usize) -> Option<usize> {
    None
}

// ===========================================================================
// slot_of — attribution
// ===========================================================================

#[test]
fn a_slot_top_is_attributed_to_its_own_slot_and_not_the_next_one() {
    for cpu in 0..MAX_CPUS {
        assert_eq!(
            slot(kernel_stack_top(cpu)),
            Some(cpu),
            "the exclusive top of slot {cpu} must attribute to slot {cpu}: stack tops are one \
             past the last byte, so a half-open test puts every own-slot top in the next slot"
        );
    }
}

#[test]
fn the_last_slots_top_stays_inside_the_region() {
    let last = MAX_CPUS - 1;
    assert_eq!(slot(kernel_stack_top(last)), Some(last));
    assert_eq!(
        slot(kernel_stack_top(last) + 1),
        None,
        "one byte past the region is outside it"
    );
}

#[test]
fn addresses_outside_the_region_name_no_slot() {
    // A heap-backed thread kernel stack, the case every ordinary thread takes.
    assert_eq!(slot(0xFFFF_0000_5426_6000), None);
    // The region base itself is the exclusive bottom.
    assert_eq!(slot(BASE), None);
    assert_eq!(slot(0), None);
    assert_eq!(slot(BASE - 1), None);
    // The first addressable byte of slot 0.
    assert_eq!(slot(BASE + 1), Some(0));
}

#[test]
fn an_interior_address_is_attributed_to_the_slot_it_stands_in() {
    // The round-3 specimen: CPU 3 standing 240 bytes below the top of CPU 0's
    // scheduler half.
    let sched_top_cpu0 = BASE + STRIDE / 2;
    assert_eq!(slot(sched_top_cpu0 - 240), Some(0));
    // The round-4 §7 peer: an EL1 frame SP 0x120 below CPU 3's exception top.
    assert_eq!(slot(kernel_stack_top(3) - 0x120), Some(3));
}

// ===========================================================================
// slot_admits — the custody rule itself
// ===========================================================================

#[test]
fn a_cpu_may_use_its_own_slot() {
    for cpu in 0..MAX_CPUS {
        assert!(
            slot_admits(cpu, slot(kernel_stack_top(cpu)), claim_of(cpu)),
            "CPU {cpu} must be admitted onto its own published slot"
        );
    }
}

#[test]
fn a_cpu_may_not_use_another_cpus_slot() {
    for cpu in 0..MAX_CPUS {
        for other in 0..MAX_CPUS {
            if other == cpu {
                continue;
            }
            assert!(
                !slot_admits(cpu, slot(kernel_stack_top(other)), claim_of(other)),
                "CPU {cpu} must be refused slot {other}"
            );
        }
    }
}

#[test]
fn the_round_3_specimen_is_refused() {
    // [PERCPU_STACK_ALIEN:cpu=3:owner=0:sp=0xffff000043200000:tid=1201]
    let sp = 0xFFFF_0000_4320_0000;
    assert_eq!(slot(sp), Some(0), "the specimen address names slot 0");
    assert!(
        !slot_admits(3, slot(sp), claim_of(0)),
        "CPU 3 must be refused CPU 0's kernel stack top — the specimen this branch repairs"
    );
    assert!(
        slot_admits(0, slot(sp), claim_of(0)),
        "the same address is CPU 0's own and must stay admissible"
    );
}

#[test]
fn an_address_naming_no_slot_is_admitted_without_consulting_a_claim() {
    // The claim panics: an ordinary heap stack must be decided on the
    // arithmetic alone, which is also what keeps the dispatch path cheap.
    let heap = 0xFFFF_0000_5426_6000;
    assert!(slot_admits(3, slot(heap), |_| panic!(
        "a claim must not be read for an address that names no per-CPU slot"
    )));
}

#[test]
fn a_foreign_slot_is_refused_before_any_claim_is_read() {
    assert!(!slot_admits(3, slot(kernel_stack_top(0)), |_| panic!(
        "a claim must not be read for an address the arithmetic already refused"
    )));
}

#[test]
fn an_unpublished_own_slot_is_admitted_and_an_unpublished_foreign_slot_is_not() {
    // Early boot: no CPU has stamped its record yet.
    assert!(
        slot_admits(2, slot(kernel_stack_top(2)), no_claim),
        "an unpublished own slot must stay admissible or early boot cannot install a stack top"
    );
    assert!(
        !slot_admits(2, slot(kernel_stack_top(5)), no_claim),
        "an unpublished slot is not a hole: the arithmetic still refuses another CPU's slot"
    );
}

#[test]
fn a_claim_naming_another_cpu_refuses_the_slot() {
    assert!(
        !slot_admits(4, slot(kernel_stack_top(4)), claim_of(1)),
        "own-slot arithmetic must not override a record that names a different owner"
    );
}

// ===========================================================================
// decode_owner_record — what counts as a claim
// ===========================================================================

#[test]
fn the_publishers_pair_decodes_to_the_publisher() {
    for owner in 0..MAX_CPUS {
        let (word0, word1) = published(owner);
        assert_eq!(
            decode_owner_record(word0, word1, MAGIC, MAX_CPUS),
            Some(owner)
        );
    }
}

#[test]
fn a_record_that_is_not_a_published_pair_reads_as_no_claim() {
    let (word0, word1) = published(3);
    // Never written.
    assert_eq!(decode_owner_record(0, 0, MAGIC, MAX_CPUS), None);
    // Half written — the second word still holds what was there before.
    assert_eq!(decode_owner_record(word0, 0x4142_4344, MAGIC, MAX_CPUS), None);
    assert_eq!(decode_owner_record(0, word1, MAGIC, MAX_CPUS), None);
    // Stack bytes that happen to agree with each other but name no CPU.
    assert_eq!(
        decode_owner_record(MAGIC ^ 99, 99, MAGIC, MAX_CPUS),
        None,
        "an owner outside 0..MAX_CPUS is not a claim"
    );
    // Two plausible words written by something that is not the publisher.
    assert_eq!(
        decode_owner_record(0xFFFF_0000_4320_0000, 0xFFFF_0000_4320_0000, MAGIC, MAX_CPUS),
        None
    );
}

// ===========================================================================
// The bracket: where the record sits relative to the sentinels
// ===========================================================================

#[test]
fn the_ownership_record_is_bracketed_by_the_two_sentinels() {
    assert!(
        BOUNDARY_CANARY_OFFSET + 8 <= OWNER_RECORD_OFFSET,
        "the half-boundary canary must sit below the ownership record"
    );
    assert!(
        OWNER_RECORD_OFFSET + OWNER_RECORD_BYTES <= OVERRUN_SENTINEL_OFFSET,
        "a sentinel must stand between the ownership record and the stack above it"
    );
    assert!(RESERVED_HALF_BOTTOM_BYTES >= OVERRUN_SENTINEL_OFFSET + 8);
}

#[test]
fn no_downward_overrun_reaches_the_record_while_its_sentinel_reads_clean() {
    // AArch64 stacks grow down, so an overrun of the idle/exception half writes
    // a contiguous run of bytes from some low offset upward. Exhaust every
    // 8-byte-aligned low offset in the reserved region and its neighbourhood:
    // whenever such a write touches ANY byte of the ownership record it must
    // also have destroyed the sentinel above it. Move the record above the
    // sentinel — the layout this branch inherited — and this fails.
    for low in (0..RESERVED_HALF_BOTTOM_BYTES + 64).step_by(8) {
        let touches_record = low < OWNER_RECORD_OFFSET + OWNER_RECORD_BYTES;
        let destroyed_sentinel = low <= OVERRUN_SENTINEL_OFFSET;
        assert!(
            !touches_record || destroyed_sentinel,
            "a downward write reaching offset {low} rewrites the ownership record with the \
             overrun sentinel still intact"
        );
    }
}

#[test]
fn the_scheduler_half_cannot_reach_the_record_by_growing_down() {
    // The scheduler half's SP starts AT the half boundary (offset 0) and every
    // push writes below it, so the reserved words at and above offset 0 are not
    // scheduler stack. This is why the record may live above the canary at all,
    // and it is the fact the placement rests on.
    for low in (0..RESERVED_HALF_BOTTOM_BYTES).step_by(8) {
        assert!(
            low >= BOUNDARY_CANARY_OFFSET,
            "reserved word at {low} is below the scheduler-half top"
        );
    }
}
