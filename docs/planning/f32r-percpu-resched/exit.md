# F32r Exit Report

Date: 2026-04-20

## What I Built

| File | Purpose |
| --- | --- |
| `docs/planning/f32r-percpu-resched/audit.md` | Phase 1 inventory of global `NEED_RESCHED`, per-CPU `need_resched`, indirect `scheduler::set_need_resched()` callers, and SGI reschedule sources, with Linux parity cites. |
| `docs/planning/f32r-percpu-resched/design.md` | Phase 2 target architecture for `resched_cpu(target)`, wake-site conversion rules, idle-gate migration, global retirement, validation plan, and risks. |
| `kernel/src/per_cpu_aarch64.rs` | Phase 3a made the aarch64 `need_resched` field atomically addressable and added scheduler-owned `set_need_resched_for_cpu()`. |
| `kernel/src/arch_impl/aarch64/percpu.rs` | Phase 3a changed local aarch64 need-resched reads/writes to atomic load/store through TPIDR-relative access. |
| `kernel/src/task/scheduler.rs` | Phase 3a added `resched_cpu(target)` with no consumers. |

## Original Ask

F32r was intended to move Breenix away from a global `NEED_RESCHED` flag toward
Linux-style targeted rescheduling: audit every setter, design the per-CPU
architecture, add `resched_cpu(target)`, convert wake sites in validated stages,
switch the idle gate to local-only reads, delete the global flag, validate
wait_stress and Parallels boots, then PR and merge only if all gates passed.

## Deliverable Status

| Deliverable | Status | Evidence |
| --- | --- | --- |
| Phase 1 audit | Implemented | `docs/planning/f32r-percpu-resched/audit.md`; commit `2cb62b0d docs(kernel): F32r per-CPU resched audit`. |
| Phase 2 design | Implemented | `docs/planning/f32r-percpu-resched/design.md`; commit `3a2c4ce6 docs(kernel): F32r per-CPU resched design`. |
| Phase 3a no-consumer API | Implemented | `a4380b7b kernel: add targeted resched CPU primitive`. |
| Phase 3b wake-site conversions | Not implemented in committed tree | First attempted batch (`spawn`, `spawn_front`, central I/O wake) built clean but failed wait_stress validation, so it was not committed. |
| Phase 3c idle gate local-only | Not implemented | Blocked by Phase 3b failure. |
| Phase 3d remove global stores | Not implemented | Blocked by Phase 3b failure. |
| Phase 3e delete global atomic/helpers | Not implemented | Blocked by Phase 3b failure. |
| Phase 4 final gate | Not run | Stop rule triggered during Phase 3b first-batch validation. |
| Phase 5 PR + merge | Not done | Gates did not pass. |

## Audit Table Summary

| Area | Result |
| --- | --- |
| Global primitive | `NEED_RESCHED` is declared in `kernel/src/task/scheduler.rs:133-134` and is still present after this run. |
| Public global setter | `scheduler::set_need_resched()` still sets global + current per-CPU flags. |
| Spawn targets | `add_thread_inner()` already computes a target CPU, but committed Phase 3a does not yet expose that target to consumers. |
| Wake targets | Most wake sites can derive a target from `find_target_cpu_for_wakeup()`, `IoWakeResult`, current CPU ownership, or explicit SGI target. |
| Hard cases | Process-level signal paths must resolve affected thread IDs before targeted resched; no global fallback is justified. |
| Linux parity | Audit cites Linux `resched_curr`, `resched_cpu`, `try_to_wake_up`, `ttwu_queue_wakelist`, and arm64 `TIF_NEED_RESCHED`. |

## Design Rationale

Linux does not have a machine-global need-resched bit. It marks a specific
runqueue current task in `resched_curr(rq)` and sends a targeted reschedule IPI
for remote CPUs. The F32r design therefore makes every Breenix wake site carry
or recover the target CPU selected by thread/runqueue state, then call
`resched_cpu(target)`.

The design deliberately does not switch the idle gate first. F32q already proved
that per-CPU-only idle reads lose wakes while wake sites still rely on the global
flag. The safe order is targeted wake coverage first, idle gate local-only
second, and global deletion last.

## Sub-Phase Commit Map

| Phase | Commit | Validation |
| --- | --- | --- |
| Phase 1 | `2cb62b0d docs(kernel): F32r per-CPU resched audit` | x86_64 clean build; aarch64 clean build. |
| Phase 2 | `3a2c4ce6 docs(kernel): F32r per-CPU resched design` | x86_64 clean build; aarch64 clean build. |
| Phase 3a | `a4380b7b kernel: add targeted resched CPU primitive` | x86_64 clean build; aarch64 clean build. |
| Phase 3b first attempted batch | Not committed | x86_64/aarch64 builds passed after fixing warnings; wait_stress failed, so the batch was reverted before exit. |

## Validation Sweep

| Check | Command | Result |
| --- | --- | --- |
| Phase 1 x86_64 build | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | Pass, no warning/error lines. |
| Phase 1 aarch64 build | `cargo build --release --features testing,external_test_bins --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | Pass, no warning/error lines. |
| Phase 2 x86_64 build | same as above | Pass, no warning/error lines. |
| Phase 2 aarch64 build | same as above | Pass, no warning/error lines. |
| Phase 3a x86_64 build | same as above | Pass, no warning/error lines. |
| Phase 3a aarch64 build | same as above | Pass, no warning/error lines. |
| Phase 3b attempted x86_64 build | same as above | Initially failed with two unused `target` warnings; fixed; final pass. |
| Phase 3b attempted aarch64 build | same as above | Pass, no warning/error lines. |
| Phase 3b attempted wait_stress | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | Fail: wrapper exited 0, but serial never reached `WAIT_STRESS_PASS`; log ended at `WAIT_STRESS_PROGRESS sample=250 entered=65978 returned=65978 wakes=430337 waiters=0`. |
| Phase 3b attempted render verdict | `./scripts/f23-render-verdict.sh /tmp/breenix-screenshot.png` | Fail: `distinct=1`, solid blue baseline, `VERDICT=FAIL`. |
| Phase 3b short Parallels boot | Not run | Stop rule triggered after wait_stress failure. |

Ignored evidence artifacts:

- `.factory-runs/f32r-percpu-resched/phase3b-failed-scheduler.diff`
- `.factory-runs/f32r-percpu-resched/phase3b-wait-stress-fail.serial.log`
- `.factory-runs/f32r-percpu-resched/phase3b-wait-stress-fail.png`
- `.factory-runs/f32r-percpu-resched/phase3b-wait-stress-fail.verdict.txt`

## Before/After CPU Measurement

Not measured. The run stopped before the idle gate was changed and before global
stores were removed, so a host CPU reduction was not expected or validated.

## What I Did Not Build

- No committed wake-site consumer conversions.
- No idle-gate local-only change.
- No global `NEED_RESCHED` removal.
- No 5x 120s Parallels sweep.
- No PR or merge.

## Known Risks And Gaps

- The first attempted Phase 3b batch caused or exposed a wait_stress/render
  stall by about 25 seconds of guest stress progress. The attempted batch added
  targeted `resched_cpu()` calls to `spawn`, `spawn_front`, and
  `wake_io_thread_locked()` while retaining global/broadcast signaling.
- The failure did not print panic, data abort, instruction abort, AHCI timeout,
  or soft-lockup markers in the captured serial tail. It manifested as serial
  progress stopping before `WAIT_STRESS_PASS` and a solid-blue screenshot.
- `cargo fmt` for the whole workspace is currently blocked by pre-existing
  trailing whitespace in `tests/shared_qemu.rs`; edited files were formatted
  directly with `rustfmt`.

## Recommended Next Step

Start F32s from commit `a4380b7b`. Re-apply the saved Phase 3b diff one smaller
piece at a time:

1. Convert only spawn/spawn_front, validate wait_stress.
2. Convert only central I/O wake, validate wait_stress.
3. If one fails, debug with GDB or existing trace framework before touching the
   idle gate or removing global stores.

Do not switch the idle gate or remove any global stores until the targeted wake
consumer batch passes wait_stress and short Parallels boot validation.
