# F20c CPU 0 WFI Wake-Path Diagnostic + Fix

## VERDICT

FAIL. Do not merge as a fix.

Phase 1 produced the requested end-of-boot per-CPU tick dump and confirmed H2
for the observable `main` `--test 45` path: CPU 0's virtual timer interrupt
delivery stops after early boot. No Phase 2 fix was committed because the data
does not identify a safe non-prohibited fix, and Breenix already contains the
suggested Linux-style CVAL re-arm path.

## What I Built

- `kernel/src/arch_impl/aarch64/context_switch.rs`: added F20c idle-entry and
  post-WFI atomic snapshots around `idle_loop_arm64`.
- `kernel/src/main_aarch64.rs`: added one-shot diagnostic kernel threads that
  emit `[END_OF_BOOT_AUDIT]` after roughly 40 seconds measured by `CNTVCT_EL0`.
- `kernel/src/task/completion.rs`: gated ARM64-only AHCI trace calls so the
  standard host build remains clean.
- `kernel/src/task/scheduler.rs`: gated an ARM64-only diagnostic helper so the
  standard host build remains clean.
- `docs/planning/f20c-cpu0-wake-path/phase1.md`: recorded the diagnostic,
  build results, boot data, and H2 verdict.
- `docs/planning/f20c-cpu0-wake-path/phase2.md`: documented why no fix was
  committed and recommended a specific F20d SGI probe.

## Original Ask

Distinguish whether CPU 0 wakes from WFI but skips the existing post-WFI dump
path (H1), or whether CPU 0's virtual timer PPI genuinely stops firing (H2).
Then either commit a verdict-driven fix or document why no fix is possible and
recommend the next probe.

## How This Meets The Ask

- Phase 1 diagnostic: implemented in commit `b52a154b`
  (`diagnostic(arm64): F20c end-of-boot per-CPU tick counter dump`).
- Phase 1 verdict: implemented in `phase1.md`; H2 is confirmed for the
  observable main-branch boot path.
- Phase 2 fix: not implemented; `phase2.md` explains why no evidence-backed
  non-prohibited fix is available.
- Sweep: not run because no fix commit exists and merge criteria are unmet.
- PR: not opened because this is a diagnostic/failure outcome, not a mergeable
  fix.

## Phase 1 Verdict

H2: CPU 0's PPI27 virtual timer is not firing after early boot.

Supporting data from `./run.sh --parallels --test 45`:

```text
[ahci]   tick_count=[9,8215,8214,8216,8213,8215,8215,8244]
[END_OF_BOOT_AUDIT] tick_count=[9,32107,32107,32109,32112,32117,32107,32138]
[END_OF_BOOT_AUDIT] hw_tick_count=[9,32109,32109,32111,32114,32119,32109,32140]
[END_OF_BOOT_AUDIT] timer_ctl=[0x1,0x1,0x1,0x1,0x1,0x1,0x1,0x1]
```

CPU 0 stayed at 9 ticks while every other CPU advanced by about 24k more ticks.
The AHCI timeout also showed CPU0 PPI27 enabled and pending, with the CPU0 timer
deadline already expired by about 10.2 seconds.

Important caveat: on `main`, this boot did not reproduce F20b's CPU0
`pre_wfi` idle-loop row. CPU0's `idle_arm_tick[0]` and `post_wfi_count[0]` were
both zero. That does not weaken the PPI-delivery finding, but it means the exact
F20b WFI edge was not reobserved on this branch.

## Phase 2 Outcome

No fix committed. The obvious H2 direction, programming `CNTV_CVAL_EL0` to
`CNTVCT + period` with `CNTV_CTL_EL0 = 1`, is already implemented in the timer
re-arm path and is also called before idle WFI. Changing timer interrupt,
exception, syscall-entry, or GIC code was prohibited for this run.

Specific F20d recommendation: add a one-shot non-timer kthread on a nonzero CPU
that detects CPU0 tick freeze, sends one SGI to CPU0 via the existing GIC send
path, and records whether CPU0 takes any interrupt or resumes timer ticks.

## Validation

Clean build gates:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
result: clean, no warning/error lines

cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
result: clean, no warning/error lines
```

Phase 1 boot:

```text
./run.sh --parallels --test 45
result: audit emitted; H2 diagnostic data captured
```

Sweep table:

| Run | Command | host_cpu_avg < 150% | boot_script_completed=1 | timer_tick_count >= 24 | Result |
|---|---|---:|---:|---:|---|
| 1 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 2 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 3 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 4 | Not run | N/A | N/A | N/A | Blocked: no fix |
| 5 | Not run | N/A | N/A | N/A | Blocked: no fix |

## Self-Audit

- No changes to `kernel/src/arch_impl/aarch64/timer_interrupt.rs`.
- No changes to `kernel/src/arch_impl/aarch64/exception.rs`.
- No changes to `kernel/src/arch_impl/aarch64/syscall_entry.rs`.
- No changes to `kernel/src/arch_impl/aarch64/gic.rs`.
- No polling fallback was added.
- F1-F19 were not reverted.
- QEMU cleanup was run before Parallels boot tests.

## PR

No PR was opened. Nothing was merged.

