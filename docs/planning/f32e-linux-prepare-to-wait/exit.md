# F32e Exit - Linux `prepare_to_wait_event` Pattern

Date: 2026-04-18

## Verdict

FAIL. F32e completed the audit and implemented a closer Linux-style waitqueue
prepare path, and `/bin/wait_stress` still passed for 60 seconds with 0 stalls.
The required Parallels gate did not pass, so no PR was opened and no merge was
attempted.

Per the F32e contract, validation stopped after the first normal 120s Parallels
run failed to reach the required bsshd/bounce/render/FPS milestones.

## What I Built

| File | Purpose |
| --- | --- |
| `docs/planning/f32e-linux-prepare-to-wait/audit.md` | Side-by-side Linux vs Breenix audit for waitqueue lock scope, state-publish barrier, same-lock wake path, and scheduler state re-check. |
| `kernel/src/task/scheduler.rs` | Added `publish_current_io_wait_state()` so waitqueues can publish `BlockedOnIO` with a full fence without running the heavier queue-pruning block path. |
| `kernel/src/task/waitqueue.rs` | Changed `prepare_to_wait` to enqueue + publish state under the waitqueue lock, then unlock before scheduling; moved lock-free wake publication under the same waitqueue lock. |

## Linux Cites Used

| Decision | Linux v6.8 evidence |
| --- | --- |
| Hold waitqueue lock only across enqueue + state publish, not schedule | `/tmp/linux-v6.8/kernel/sched/wait.c:270-300` (`prepare_to_wait_event`) and `/tmp/linux-v6.8/kernel/sched/wait.c:233-238` (`prepare_to_wait`). |
| State publish needs full barrier | `/tmp/linux-v6.8/include/linux/sched.h:184-231`; `set_current_state()` is `smp_store_mb(current->__state, state)`. |
| Wake serializes on same waitqueue lock | `/tmp/linux-v6.8/kernel/sched/wait.c:99-108`; `__wake_up_common_lock` takes `wq_head->lock` around wake traversal. |
| Schedule must re-check state | `/tmp/linux-v6.8/kernel/sched/core.c:6653-6681`; `__schedule` reads `prev->__state` and does not deactivate `TASK_RUNNING`. |
| Wake barrier pairs with state publish | `/tmp/linux-v6.8/kernel/sched/core.c:4247-4255`; `try_to_wake_up` executes a full barrier before checking task state. |

## Audit Findings

The Phase 1 audit found that F32c had closed the original lost-wake reproducer by
holding the waitqueue lock across `block_current_for_io()`, but that differed
from Linux because the critical section included Breenix-specific scheduler work
instead of only wait-list insertion plus state publication.

The audit also found that Breenix already had a wrapper-level no-op check in
`schedule_current_wait()` (`kernel/src/task/waitqueue.rs`) before entering the
architecture scheduler. The missing split was narrower than the initial expected
finding: Breenix needed a publish-only wait-state primitive so the caller could
release the waitqueue lock before scheduling.

Validation shows the audit is still incomplete for the broader Parallels failure.
The waitqueue reproducer remains fixed, but bwm/bsshd/bounce still do not reach
the full acceptance gate. One likely area for the next audit pass is the remaining
difference from Linux's immediate `try_to_wake_up` state transition: Breenix
`wake_up` still publishes to the lock-free ISR wake buffer rather than directly
setting the task runnable under the waitqueue wake traversal. I did not change
that further because the contract forbids workarounds and says to return to the
audit when Parallels fails.

## Commits

| Commit | Summary |
| --- | --- |
| `e29d5abd` | `docs(kernel): F32e audit Linux waitqueue parity` |
| `3d0acbd3` | `fix(kernel): F32e shrink waitqueue critical section to Linux pattern` |

## Validation

| Gate | Command | Result | Evidence |
| --- | --- | --- | --- |
| Standard release build | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS | `/tmp/f32e-build.log`; no `warning` or `error` lines. |
| aarch64 kernel build | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS | `/tmp/f32e-aarch64-build.log`; no `warning` or `error` lines. |
| wait_stress 60s | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | PASS for wait_stress | `WAIT_STRESS_PROGRESS sample=600 entered=147525 returned=147525 wakes=6800321 waiters=0`; `WAIT_STRESS_PASS`; no `WAIT_STRESS_STALL`. |
| post-stress boot | Same as above | FAIL after stress | After `WAIT_STRESS_PASS`, `/bin/bwm` hit `AHCI Port 1 TIMEOUT`; bsshd and bounce failed to start. |
| normal Parallels run 1 | `./run.sh --parallels --test 120` | FAIL | Serial reached `/bin/bwm`, `TELNETD_LISTENING`, and `[init] Boot script completed`, then ended at `[spawn] path='/bin/bsshd'`; no bsshd success, no bounce, no render verdict, no FPS sample. |

Artifacts:

```text
.factory-runs/f32e-linux-prepare-to-wait-20260418T203720Z/wait-stress.log
.factory-runs/f32e-linux-prepare-to-wait-20260418T203720Z/wait-stress.serial.log
.factory-runs/f32e-linux-prepare-to-wait-20260418T203720Z/parallels-run1.log
.factory-runs/f32e-linux-prepare-to-wait-20260418T203720Z/parallels-run1.serial.log
```

## Sweep Table

| Run | Result | bsshd | bounce | CPU0 tick_count > 1000 | FPS >= 160 | Strict render | AHCI TIMEOUT | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| wait_stress | PASS for stress, FAIL after stress | No | No | Yes in timeout dump (`tick_count[0]=2148`) | No sample | No verdict | Yes | Stress itself passed with 6.8M wakes and 0 stalls; post-stress `/bin/bwm` and later service spawns hit AHCI timeouts. |
| 1 | FAIL | No success line | Not reached | Not established | No sample | No verdict | No timeout observed in 120s serial | Serial ended at `[spawn] path='/bin/bsshd'`. |
| 2 | Not run | - | - | - | - | - | - | Stopped after run 1 failed, per contract. |
| 3 | Not run | - | - | - | - | - | - | Stopped after run 1 failed, per contract. |
| 4 | Not run | - | - | - | - | - | - | Stopped after run 1 failed, per contract. |
| 5 | Not run | - | - | - | - | - | - | Stopped after run 1 failed, per contract. |

## Self-Audit

- No timer-driven wake fallback was added.
- No arbitrary timeout was added.
- No CPU0 EL0 routing workaround was added.
- No Tier 1 prohibited files were modified.
- No F1-F30 or F32c commits were reverted.
- The code builds cleanly with zero warnings in the commands listed above.

## What I Did Not Build

I did not open a PR, merge, or continue the Parallels sweep. The contract requires
5/5 successful normal Parallels runs before PR/merge, and run 1 failed.

## PR

N/A. No PR was opened.

## Recommended Next Step

Return to Phase 1 with the new evidence. In particular, compare Linux's
`try_to_wake_up` immediate state transition against Breenix's deferred
`isr_unblock_for_io` wake buffer in the waitqueue path, and verify whether the
architecture scheduler's state re-check after draining deferred wakeups is truly
equivalent under the bwm/bsshd spawn workload.
