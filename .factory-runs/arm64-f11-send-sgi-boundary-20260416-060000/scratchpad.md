# Scratchpad - F11 Send SGI Boundary

## 2026-04-16T06:00:00-04:00

Starting M1: add breadcrumb sites.

- Base branch verified: `diagnostic/f10-isr-unblock-boundary`.
- Work branch created in a fresh worktree:
  `/Users/wrb/fun/code/breenix-worktrees/f11-send-sgi-boundary`.
- F10 exit reviewed. Primary prior stall stopped after `UNBLOCK_PER_SGI`;
  secondary stopped after `UNBLOCK_AFTER_CPU`.
- Target locations found:
  - `kernel/src/arch_impl/aarch64/gic.rs`: `send_sgi(sgi_id, target_cpu)`.
  - `kernel/src/task/scheduler.rs`: `isr_unblock_for_io(tid)` calls
    `ISR_WAKEUP_BUFFERS[cpu].push(tid)`.
  - `kernel/src/drivers/ahci/mod.rs`: AHCI trace site constants and renderer.

## 2026-04-16T06:05:00-04:00

Patched M1:

- Added AHCI site constants 15-23 and renderer names.
- Added `trace_sgi_boundary(site, target_cpu)` in `gic.rs` so each SGI
  breadcrumb consistently encodes `target_cpu` in `slot_mask`.
- Inserted SGI breadcrumbs around target-list construction, SGIR composition,
  the `msr`, the `isb`, and function exit.
- Inserted `WAKEBUF_BEFORE_PUSH` and `WAKEBUF_AFTER_PUSH` immediately around
  the existing `ISR_WAKEUP_BUFFERS[cpu].push(tid)` call.

## 2026-04-16T06:10:00-04:00

M1 validation:

- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc
  -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
  completed cleanly.
- `grep -E '^(warning|error)' /tmp/f11-aarch64-build.log` produced no output.
- `git diff --check` passed.

Starting M2: commit the code-only diagnostic patch.

## 2026-04-16T06:35:00-04:00

First five-run sweep showed SGI breadcrumbs but no `UNBLOCK_*` or `WAKEBUF_*`
correlation in most timeout dumps. The added seven-site SGI trace is high volume
enough that the existing SPI34 stuck-state dump depth of 16 entries can omit the
preceding F10 corridor. About to widen only the SPI34 postmortem AHCI-ring dump
from 16 to 64 entries so the F11 breadcrumbs remain correlatable. This is still
postmortem diagnostic output, not hot-path SGI or scheduler semantics.

## 2026-04-16T06:55:00-04:00

Final sweep complete:

- Build remained clean after widening the SPI34 postmortem dump depth.
- 5x `./run.sh --parallels --test 60` captured under
  `logs/breenix-parallels-cpu0/f11-send-sgi/run{1..5}/`.
- Runs 3 and 5 captured AHCI timeout dumps with F11 SGI sites.
- Run 5 captured the best primary sequence:
  `WAKE_ENTER -> UNBLOCK_ENTRY -> UNBLOCK_AFTER_CPU ->
  WAKEBUF_BEFORE_PUSH -> WAKEBUF_AFTER_PUSH -> UNBLOCK_AFTER_BUFFER ->
  UNBLOCK_AFTER_NEED_RESCHED -> UNBLOCK_BEFORE_SGI_SCAN -> UNBLOCK_PER_SGI
  -> SGI_ENTRY -> SGI_AFTER_MPIDR -> SGI_AFTER_COMPOSE -> SGI_BEFORE_MSR`.
- No matching `SGI_AFTER_MSR` appeared for that contiguous SGI call before the
  sequence moved on; later SGI calls on CPU 0 make this a best-effort
  last-site verdict rather than a fully correlated call-id proof.
- Secondary wake-buffer signature did not reproduce in run 5: both
  `WAKEBUF_BEFORE_PUSH` and `WAKEBUF_AFTER_PUSH` appeared.
