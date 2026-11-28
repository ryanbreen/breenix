//! The operand of an ARM64 TLB invalidate by VA (`TLBI VAAE1IS` and
//! `TLBI VAAE1`), with no kernel dependencies, so `tools/kmem-host-tests`
//! compiles this file as is.
//!
//! The operand's layout (Arm ARM, TLBI VAAE1IS): bits [43:0] carry VA[55:12],
//! bits [47:44] are the TTL hint when FEAT_TTL is implemented, and bits
//! [63:48] are RES0 (the instruction matches any ASID).

/// VA[55:12] in operand bits [43:0].
const VA_FIELD: u64 = (1 << 44) - 1;

/// The `TLBI VAAE1IS` operand that invalidates the 4 KiB page at `va`: VA[55:12],
/// with TTL 0 (no level or granule hint, which matches each entry) and the
/// RES0 bits clear. A TTBR1 address has VA[63:56] set, and those bits must
/// not reach the TTL: a hint that does not describe the entry leaves the CPU
/// free to keep it.
pub const fn vaa_operand(va: u64) -> u64 {
    (va >> 12) & VA_FIELD
}
