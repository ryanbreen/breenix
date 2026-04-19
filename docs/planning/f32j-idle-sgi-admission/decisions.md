# Decisions - F32j Idle Sleep Gate + GIC SGI Admission

## 2026-04-19 - Idle loop file location

**Choice:** Implement the idle gate in `kernel/src/arch_impl/aarch64/context_switch.rs`, where `idle_loop_arm64` currently lives.

**Alternatives considered:** Edit `kernel/src/main_aarch64.rs` as named in the original prompt.

**Evidence:** `rg` shows `main_aarch64.rs` only references setup for `idle_loop_arm64`; the executable idle WFI loop is `context_switch.rs:3225`.

## 2026-04-19 - GIC admission root-cause candidate

**Choice:** Treat SGI enable state as the leading fix candidate.

**Alternatives considered:** Group assignment, PMR/priority, CPU interface group enable, SGI routing, and SGI send barriers.

**Evidence:** Current GIC init writes `GICR_IGROUPR0 = 0xffff_ffff`, sets SGI/PPI priority to `0xa0`, enables `ICC_IGRPEN1_EL1`, and `send_sgi()` already uses Linux's `dsb ishst` before `ICC_SGI1R_EL1` plus `isb` after. But `init_gicv3_redistributor()` disables all SGI/PPI lines and no call enables SGI0 or SGI1. F32i saw SGI0 pending in `GICR_ISPENDR0` while `HPPIR1` was spurious, which is consistent with a pending but disabled interrupt.
