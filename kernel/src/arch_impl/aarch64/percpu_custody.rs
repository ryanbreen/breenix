//! Per-CPU stack custody: the arithmetic, and only the arithmetic.
//!
//! Every per-CPU stack decision on the aarch64 dispatch path — the top a CPU
//! publishes, the destination it pivots onto, the saved SP it resumes a thread
//! on — reduces to three pure questions:
//!
//! 1. which slot, if any, does an address belong to (`slot_of`);
//! 2. does a slot's two-word ownership record read back as a claim
//!    (`decode_owner_record`);
//! 3. may CPU `cpu` use an address that names slot `slot` (`slot_admits`).
//!
//! They live here, apart from `constants.rs`, because everything else in
//! `constants.rs` reaches memory or the platform and can therefore only be
//! executed on the target. These three cannot: they take their inputs as
//! arguments and return an answer. `tests/percpu_stack_custody.rs` includes
//! this file directly and executes them, so deleting or inverting the custody
//! rule reddens a host test rather than only a source-shape ratchet — the gap
//! the round-2, round-3 and round-4 reviews each recorded as still open.
//!
//! The offsets are here for the same reason: where the ownership record sits
//! relative to the sentinels that bracket it is a property of arithmetic, and
//! the host test proves it by exhaustion instead of by reading a comment.

// ============================================================================
// The bottom of a per-CPU slot's idle/exception half
// ============================================================================
//
// AArch64 stacks grow DOWN. The idle/exception half runs from
// `percpu_kernel_stack_bottom(cpu)` up to `percpu_kernel_stack_top(cpu)`, so an
// overrun of that half walks DOWN through these offsets in DESCENDING order:
// it reaches `OVERRUN_SENTINEL_OFFSET` first, the ownership record next, and
// `BOUNDARY_CANARY_OFFSET` last.
//
// That ordering is the point. The ownership record feeds a dispatch-path
// decision (`slot_admits`), so a downward overrun that rewrote the record while
// every sentinel still read clean would let stack bytes answer a custody
// question. It cannot: a contiguous downward write cannot reach any byte of the
// record without first destroying the sentinel above it, and the record is only
// trusted while that sentinel is intact.

/// Offset of the half-boundary canary word, at the very bottom of the
/// idle/exception half. It is the LAST word a downward overrun destroys.
pub const BOUNDARY_CANARY_OFFSET: u64 = 0;

/// Offset of the two-`u64` stack-ownership record.
pub const OWNER_RECORD_OFFSET: u64 = 8;

/// Size of the ownership record: `word0 = MAGIC ^ owner`, `word1 = owner`.
pub const OWNER_RECORD_BYTES: u64 = 16;

/// Offset of the sentinel standing between the ownership record and the
/// idle/exception stack above it.
pub const OVERRUN_SENTINEL_OFFSET: u64 = OWNER_RECORD_OFFSET + OWNER_RECORD_BYTES;

/// Bytes at the bottom of the idle/exception half reserved for the sentinels
/// and the ownership record, and therefore not available as stack.
pub const RESERVED_HALF_BOTTOM_BYTES: u64 = OVERRUN_SENTINEL_OFFSET + 8;

// The canary is strictly below the record, and the record is strictly below the
// overrun sentinel. Swapping any two of them — the layout this branch
// inherited put the record ABOVE the canary with nothing above the record —
// fails the build here rather than silently letting stack bytes answer a
// custody question.
const _: () = assert!(BOUNDARY_CANARY_OFFSET + 8 <= OWNER_RECORD_OFFSET);
const _: () = assert!(OWNER_RECORD_OFFSET + OWNER_RECORD_BYTES <= OVERRUN_SENTINEL_OFFSET);
const _: () = assert!(RESERVED_HALF_BOTTOM_BYTES >= OVERRUN_SENTINEL_OFFSET + 8);

// ============================================================================
// The three decisions
// ============================================================================

/// The per-CPU stack slot an address belongs to, or `None` outside the region.
///
/// Stack tops are exclusive upper bounds — slot `cpu`'s top is
/// `base + (cpu + 1) * stride`, one past the last byte of the slot — so the
/// region is treated as `(base, base + size]` and attribution uses the last
/// addressable byte below the value. A half-open `[base, ...)` test with a
/// plain `(addr - base) / stride` would put every legitimate own-slot top in
/// the NEXT slot and push the last slot's top out of the region entirely.
///
/// Two comparisons and a shift: this runs on the dispatch path.
#[inline]
pub fn slot_of(base: u64, region_size: u64, stride_shift: u32, addr: u64) -> Option<usize> {
    if addr <= base || addr > base + region_size {
        return None;
    }
    Some(((addr - 1 - base) >> stride_shift) as usize)
}

/// The CPU a slot's ownership record names, or `None` when the two words are
/// not a claim.
///
/// Both words must agree — `word0 ^ magic == word1` — and name a CPU in range.
/// Anything else (all zeroes before publication, a half-written record, stack
/// data that reached the record) reads as unpublished rather than as some
/// arbitrary CPU.
#[inline]
pub fn decode_owner_record(word0: u64, word1: u64, magic: u64, max_cpus: usize) -> Option<usize> {
    let owner = word0 ^ magic;
    if owner != word1 || owner >= max_cpus as u64 {
        return None;
    }
    Some(owner as usize)
}

/// Whether CPU `cpu` may use an address that `slot_of` attributed to `slot`.
///
/// THE custody rule. The producer that CHOOSES a dispatch SP, the setter guard
/// that ADJUDICATES an install, the pivot that RUNS on a destination and the
/// dispatch that RESUMES a thread all reduce to this one function, so a value
/// one of them normalises cannot be refused by another.
///
/// Acceptance is positive — the address has to be attributable to `cpu` — not
/// "did not trip a scan". Three outcomes, in the order they are cheapest to
/// decide:
///
/// * `None`: the address names no per-CPU slot at all — an ordinary
///   heap-backed thread kernel stack, or CPU 0's platform boot stack on
///   Parallels. Admitted without reading any memory.
/// * The address names `cpu`'s OWN slot and the slot's claim is `cpu` or
///   nothing at all. Accepting an unpublished slot is what keeps very early
///   boot working, and it is not a hole: an address naming a different CPU's
///   slot is rejected on the arithmetic alone, before `claim` is consulted at
///   all — which is also why `claim` is a closure and not a value.
/// * Anything else belongs to another CPU.
#[inline]
pub fn slot_admits<F>(cpu: usize, slot: Option<usize>, claim: F) -> bool
where
    F: FnOnce(usize) -> Option<usize>,
{
    let Some(slot) = slot else {
        return true;
    };
    slot == cpu && claim(slot).map_or(true, |owner| owner == cpu)
}
