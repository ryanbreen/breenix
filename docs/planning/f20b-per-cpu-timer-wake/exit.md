# F20b Per-CPU Timer Delivery at WFI Transition

## VERDICT

FAIL. Do not merge.

The audit succeeded and narrowed the failure, but no safe fix was found. The
clean audit disproved the proposed mask candidates for CPU0: before WFI, CPU0
has virtual timer enabled, a near-future CVAL, PPI27 enabled in GICR, PMR open,
Group 1 enabled, and DAIF.I/F clear. CPU0 still did not emit a `post_wfi` row.

## Phase 1 Diagnostic

Committed as:

```text
diagnostic(arm64): F20b per-CPU idle-entry audit dump
```

The diagnostic is gated until after scheduler idle contexts are reset, then
captures one `pre_wfi` row per CPU and one `post_wfi` row on actual wake.

## Phase 2 Audit Output

Source log:

```text
.factory-runs/f20b-per-cpu-timer-wake-20260417-000000/phase2-serial-final.log
```

Verbatim parsed audit rows:

```text
pre_wfi 0 0x80000000 0x1 0x361ebf6 0x3618e37 0x5dbf 0xf0 0x1 0x0 0xff 0x8000000 0x0 0x8000000
pre_wfi 1 0x80000001 0x1 0x361ef01 0x3619143 0x5dbe 0xf0 0x1 0x0 0xff 0x8000000 0x0 0x8000000
pre_wfi 3 0x80000003 0x1 0x362c8a2 0x3626ae4 0x5dbe 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 3 0x80000003 0x1 0x3632bc2 0x362d132 0x5a90 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
pre_wfi 7 0x80000007 0x1 0x3638726 0x3632967 0x5dbf 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
pre_wfi 5 0x80000005 0x1 0x363a523 0x3634765 0x5dbe 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 7 0x80000007 0x1 0x363eb23 0x3638f32 0x5bf1 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 5 0x80000005 0x1 0x3640631 0x363ae30 0x5801 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
pre_wfi 6 0x80000006 0x1 0x364e437 0x3648679 0x5dbe 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 6 0x80000006 0x1 0x36542da 0x364e550 0x5d8a 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
pre_wfi 4 0x80000004 0x1 0x365e3df 0x3658621 0x5dbe 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 4 0x80000004 0x1 0x3664341 0x365e5b6 0x5d8b 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
pre_wfi 2 0x80000002 0x1 0x36c6a7f 0x36c0cc0 0x5dbf 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
post_wfi 2 0x80000002 0x1 0x36cc902 0x36c6dfd 0x5b05 0xf0 0x1 0x0 0xff 0x8000000 0x1 0x8000000
```

Column order:

```text
moment cpu mpidr cntv_ctl cntv_cval cntvct cntv_delta icc_pmr icc_igrpen1 daif icc_rpr gicr_isenabler0 gicr_ispendr0 gicr_icenabler0
```

CPU1 also emitted a `post_wfi` row in the raw log, but a CPU0 timer breadcrumb
interrupted one field. CPU0 emitted no `post_wfi` row.

## Phase 3 Attempts

| Attempt | Result |
|---|---|
| Add `isb` between `msr daifclr, #0xf` and `wfi` | No CPU0 `post_wfi`; 30-second boot completion regressed. Backed out. |
| Enable SGI0/SGI1 per redistributor | `GICR_ISENABLER0` became `0x8000003`, but 30-second boot completion regressed. Backed out. |

No Phase 3 fix commit was made.

## Phase 3 Fix Description

Not applicable. F20b did not identify a minimal safe fix to commit. Because no
fix was committed, there is no Linux-cited implementation delta in this branch.
The Linux comparison point remains `drivers/irqchip/irq-gic-v3.c::gic_cpu_init`:
the audited Breenix state already has per-CPU PPI27 enabled before WFI.

## Validation

No 5-run merge sweep was performed because Phase 3 did not produce an acceptable
fix. The required merge condition is therefore unmet.

| Run | Command | host_cpu_avg < 150% | boot_script_completed=1 | timer_tick_count >= 24 | Result |
|---|---|---|---|---|---|
| 1 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 2 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 3 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 4 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 5 | Not run | N/A | N/A | N/A | Blocked: no fix |

Build quality gates run during the investigation:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
result: clean after fixing pre-existing non-aarch64 cfg warnings/errors

cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
result: clean, no warning/error lines
```

## Self-Audit

- No changes were made to `kernel/src/arch_impl/aarch64/timer_interrupt.rs`.
- No changes were made to `kernel/src/arch_impl/aarch64/exception.rs`.
- No changes were made to `kernel/src/arch_impl/aarch64/syscall_entry.rs`.
- No polling fallback or busy-wait idle workaround was added.
- The tested but regressing Phase 3 code changes were backed out.
- QEMU cleanup was run before QEMU/VM test phases; temporary Parallels VMs were stopped and deleted.

## PR

No PR was opened and nothing was merged.
