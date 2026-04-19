# F32c Waitqueue Exit

Date: 2026-04-18

## Status

F32c did not meet the acceptance gate. Phase 1, Phase 2, and Phase 3 were
completed and committed, but Phase 4 failed on the first normal 120s Parallels
run. Per the F32c instruction, implementation stopped after that failure. No PR
was opened or merged.

## Commits

| Phase | Commit | Summary |
| --- | --- | --- |
| Phase 1 | `ede4a4e0` | `test(kernel): F32c wait_stress reproducer for waitqueue race` |
| Phase 2 | `eaa4fef6` | `docs(kernel): F32c waitqueue ordering audit` |
| Phase 3 | `1f6ae288` | `fix(kernel): serialize waitqueue prepare with blocked state` |

## Reproducer

Phase 1 added an opt-in userspace binary, `/bin/wait_stress`, enabled at boot by
creating `/etc/wait_stress.enabled` with `BREENIX_WAIT_STRESS=1`.

The harness forks:

- a waiter that repeatedly calls FBDRAW op 28, which prepares and schedules on a
  dedicated waitqueue with no persistent condition;
- a waker that repeatedly calls FBDRAW op 29, which wakes the same queue;
- a monitor parent that samples op 30 every 100 ms and reports a stall if wakes
  advance while wait returns stop advancing.

Before the fix, the reproducer failed deterministically:

```text
WAIT_STRESS_STALL sample=3 entered=269 returned=268 wakes=17280 waiters=0
```

Artifact: `.factory-runs/f32c-waitqueue-20260418/phase1-wait-stress-stall.serial.log`.

After the Phase 3 fix, the 60s reproducer reached the end without a stall:

```text
WAIT_STRESS_PROGRESS sample=600 entered=90983 returned=90982 wakes=3245248 waiters=0
WAIT_STRESS_PASS entered=90983 returned=90982 wakes=3245248 waiters=0
```

The `WAIT_STRESS_PASS` line in the raw serial is interleaved by concurrent serial
writers; the preceding progress line preserves the full final counter values.

Artifact: `.factory-runs/f32c-waitqueue-20260418/phase4-wait-stress-60.serial.log`.

## Ordering Audit

The audit is in `docs/planning/f32c-waitqueue/ordering-audit.md`.

| Invariant | Linux v6.8 | F32 before fix | Phase 3 fix |
| --- | --- | --- | --- |
| Enqueue and state publication are serialized by the same waitqueue lock. | `prepare_to_wait` locks `wq_head->lock`, adds the entry, calls `set_current_state(state)`, then unlocks (`/tmp/linux-v6.8/kernel/sched/wait.c:233-238`). | Breenix added the TID under the waitqueue lock, released it, then set `BlockedOnIO` (`kernel/src/task/waitqueue.rs:58-66` before `1f6ae288`). | `prepare_to_wait` now holds the waitqueue lock across duplicate-free enqueue and `sched.block_current_for_io()`. |
| Blocked-state publication has a full barrier. | `set_current_state` is `smp_store_mb(current->__state, state)` (`/tmp/linux-v6.8/include/linux/sched.h:227-231`). | No explicit barrier tied to the waitqueue critical section. | `prepare_to_wait` now executes a full `SeqCst` fence before releasing the waitqueue lock. |
| Wakeup serializes on the same waitqueue lock. | `__wake_up` takes `wq_head->lock` via `__wake_up_common_lock` (`/tmp/linux-v6.8/kernel/sched/wait.c:99-127`). | Wakeup drained the TID under the queue lock, but could do so before the waiter became `BlockedOnIO`. | A wake that drains a TID can no longer run until `BlockedOnIO` is visible. |
| If wake wins before schedule, scheduling is a no-op. | `try_to_wake_up` pairs with the waiter barrier (`/tmp/linux-v6.8/kernel/sched/core.c:4247-4255`); `__schedule` does not deactivate `TASK_RUNNING` (`core.c:6653-6681`). | `schedule_current_wait` had the no-op check, but the wake could be ignored before state publication. | The no-op check is now reachable for the original lost-wake race. |

## Phase 4 Validation

| Gate | Command | Result | Evidence |
| --- | --- | --- | --- |
| aarch64 kernel build | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS | `.factory-runs/f32c-waitqueue-20260418/phase3-aarch64-build.log`; no warnings in output |
| 60s waitqueue stress | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | PASS for waitqueue stress, then failed later in normal boot | `WAIT_STRESS_PASS`; after stress, boot hit AHCI timeouts when spawning BWM/bsshd/bounce |
| Normal 120s Parallels run | `./run.sh --parallels --test 120` | FAIL | Serial stopped at `[spawn] path='/bin/bwm'`; no bsshd, no bounce, no FPS sample |

Post-stress failure signature:

```text
[spawn] path='/bin/bwm'
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   cpu0_last_timer_elr=0xffff00004010c5e8 cpu0_breadcrumb=107 ...
[ahci]   tick_count=[43299,52636,53017,53569,53820,53854,53798,55002]
```

Normal-run failure signature:

```text
[init] Breenix init starting (PID 1)
T5[spawn] path='/bin/bwm'
T6T7T8T9T0
```

Artifact: `.factory-runs/f32c-waitqueue-20260418/phase4-normal-run1.serial.log`.

## Sweep Table

The required 5 x 120s sweep was not continued after run 1 failed.

| Run | Result | bsshd | bounce | CPU0 tick_count > 1000 | FPS >= 160 | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | FAIL | No | No | Not established | No sample | Stopped at BWM spawn |
| 2 | Not run | - | - | - | - | Stopped per F32c failure rule |
| 3 | Not run | - | - | - | - | Stopped per F32c failure rule |
| 4 | Not run | - | - | - | - | Stopped per F32c failure rule |
| 5 | Not run | - | - | - | - | Stopped per F32c failure rule |

## Fallback Audit

No timer fallback was added for the waitqueue fix. The compositor wait path
(`op=23`) still uses `COMPOSITOR_FRAME_WQ.prepare_to_wait`,
`schedule_current_wait`, and `finish_wait`. The client frame-pacing path
(`op=15`) still uses `CLIENT_FRAME_WQ` and explicitly documents that there is no
timer fallback.

`rg "BlockedOnTimer|fallback" kernel/src/syscall/graphics.rs` still finds the
older blocking window-input path (`op=19`) and unrelated userspace font/input
comments. That is not the compositor wait path, but the grep is not globally
clean in `graphics.rs`.

## PR

No PR was opened. No merge was attempted.

## Next Investigation

The waitqueue lost-wake race is fixed for the dedicated reproducer, but the
normal compositor boot still fails before the acceptance sweep. The next attempt
should start from these artifacts and determine whether `1f6ae288` exposed a
separate CPU0/scheduler bug, or whether the stricter Linux-style waitqueue
ordering introduces a lock-order/preemption interaction during BWM startup.
