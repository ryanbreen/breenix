# F32d Exit - bwm Spawn AHCI Stall After F32c Waitqueue Fix

## Verdict

FAIL. F32d fixed the bwm-spawn AHCI timeout signature seen after F32c's
waitqueue ordering change, but the first normal 120s Parallels validation run
missed the requested FPS gate. Per the factory prompt, no PR was opened and no
merge was attempted.

## Phase 1 Diagnosis

F32c's waitqueue race remained closed. The final-form `wait_stress` run reached:

```text
WAIT_STRESS_PASS entered=124804 returned=124804 wakes=5264236 waiters=0
```

The subsequent timeout initially looked like stale CPU0 ownership of tid 14, but
the CPU0 trace showed that `cpu0_dispatch tid=14` was a candidate dispatch, not
an actual userspace return: the prepared frame ELR was `idle_loop_arm64`, and
the trace repeatedly recorded `DISPATCH_REDIRECT reason=6`, the existing CPU0
EL0 guard.

The concrete bug was in the guard interaction. The guard comment said the EL0
candidate was requeued to a non-CPU0 queue, but it called
`requeue_thread_after_save()`, which appends to the current CPU queue. CPU0 then
kept selecting the same EL0 candidates, redirecting them to idle, and eventually
entered the known Parallels CPU0 WFI/PPI-pending failure mode.

## Phase 2 Fix Rationale

The waitqueue change keeps Linux's ordering without holding the waitqueue lock
across the scheduler call:

- Linux v6.8 `kernel/sched/wait.c:233-238` holds `wq_head->lock` across wait-list
  insertion and `set_current_state()`.
- Linux v6.8 `include/linux/sched.h:227-231` implements `set_current_state()` as
  a state store with `smp_store_mb()`.
- Breenix now mirrors that by enqueueing the waiter and publishing
  `BlockedOnIO` via `Scheduler::publish_current_io_wait_state()` while the
  waitqueue lock is held, then scheduling only after the waitqueue lock is
  released.

The CPU0 guard fix makes the existing Parallels/HVF policy internally
consistent:

- newly spawned ARM64 userspace threads are queued away from CPU0 when SMP is
  online;
- CPU0's scheduler selection moves EL0 candidates to a non-CPU0 queue before
  dispatch, instead of selecting and then redirecting them;
- the late CPU0 EL0 guard fallback now calls
  `requeue_user_el0_away_from_cpu0()` instead of requeueing onto CPU0.

No `BlockedOnTimer` fallback, timer polling, arbitrary timeout, or Tier 1 file
change was added. `kernel/src/arch_impl/aarch64/context_switch.rs` is Tier 2 and
was changed only to correct the existing CPU0 guard requeue target.

## Phase 3 Gates

| Gate | Command / Artifact | Result | Notes |
| --- | --- | --- | --- |
| Clean aarch64 build | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS | No warnings. |
| Formatter check | `cargo fmt` | BLOCKED | Pre-existing trailing whitespace in `tests/shared_qemu.rs`; touched files were formatted directly with `rustfmt --edition 2021`. |
| Waitqueue reproducer + post-stress boot | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | PASS | `WAIT_STRESS_PASS entered=113219 returned=113218 wakes=4503721 waiters=0`; bwm, bsshd, bounce reached; no AHCI timeout; render verdict PASS. |
| Normal Parallels run 1 | `./run.sh --parallels --test 120` | FAIL | No AHCI timeout; bsshd and bounce reached; render verdict PASS; CPU0 ticks 105000; frame #15000 gives an estimated active FPS below 160. |
| Normal Parallels runs 2-5 | Not run to completion | STOPPED | Prompt requires stopping on <5/5 rather than merging a partial fix. |

## Artifacts

```text
.factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/wait-stress-postfix.serial.log
.factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/wait-stress-postfix.png
.factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/boots/run1.serial.log
.factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/boots/run1.png
.factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/boots/run1.render.txt
```

## PR

N/A. Validation did not reach 5/5, so no PR was opened.
