# Turn 2 wake-state/enqueue source map + attribution

Status: INCONCLUSIVE.

Turn 2 added memory-only scheduler attribution counters and emitted them from
the existing freeze-watch path. The Parallels boot ran past the requested 220 s
window, but it did not reproduce the orphan-Ready rescue condition:
`rescue_total` stayed `0`, and the I/O wake paths were not exercised
(`isr_unblock=0`, `wake_io=0`). The run is therefore useful as a source map and
counter sanity check, but it does not identify a primary wake-site fix target.

Artifacts:

- `turn2-artifacts/parallels-boot/run.out`
- `turn2-artifacts/parallels-boot/serial.log`
- `turn2-artifacts/parallels-boot/kernel-build.txt`

Code commit: `86c881d7 feat(scheduler): attribute wake/enqueue/rescue with memory-only counters`.

## A. Wake-state/enqueue source map

| Site | File:Line | Caller context | Ready transition | Enqueue path | Lock held | Race window |
| --- | --- | --- | --- | --- | --- | --- |
| `schedule()` ISR-buffer drain | `kernel/src/task/scheduler.rs:867` | scheduler under `SCHEDULER` lock | none directly; drains IDs into `unblock_for_io_from_isr_buffer()` | `wake_io_thread_locked()` | yes | If ISR buffer is never drained, no Ready transition happens. |
| `schedule()` outgoing current | `kernel/src/task/scheduler.rs:850`, `:900`, `:923` | normal scheduler path | running current thread -> Ready at `:900` | same function pushes to current CPU queue at `:925` | yes | No Ready-without-queue window when `will_add` is true; `already_queued` is counted separately. |
| `schedule_deferred_requeue()` ISR-buffer drain | `kernel/src/task/scheduler.rs:1117`, `:1126`, `:1135` | aarch64 context-switch tail scheduler path | none directly; drains IDs into `unblock_for_io_from_isr_buffer()` | `wake_io_thread_locked()` | yes | Same as `schedule()` drain, but only runs when this path is entered. |
| `schedule_deferred_requeue()` outgoing current | `kernel/src/task/scheduler.rs:1117`, `:1158`, `:1172` | aarch64 context-switch tail before old context is saved | running current thread -> Ready at `:1158` | records `should_requeue_old`; actual queue insert later | yes | Yes. The thread is Ready while the old CPU still owns its unsaved context; queue insertion is intentionally deferred. |
| `requeue_thread_after_save()` | `kernel/src/task/scheduler.rs:1401`, `:1426`, `:1435` | context-switch code after saving old context | none; requires thread already Ready | pushes to current CPU queue at `:1435` | yes | If this step is skipped or returns early after Ready publication, the rescue predicate can observe an orphan Ready thread. |
| `unblock()` | `kernel/src/task/scheduler.rs:1458`, `:1469`, `:1506` | general scheduler wake path | blocked/thread wait state -> Ready at `:1469` | same lock queues target CPU at `:1507`, or records deferred/current/queued | yes | Yes when the thread is current on some CPU or in deferred requeue. State is Ready but queue insertion is skipped for later owner-side handling. |
| `unblock_for_signal()` | `kernel/src/task/scheduler.rs:1664`, `:1687`, `:1710` | signal delivery wake path | `BlockedOnSignal` -> Ready at `:1687` | same lock queues target CPU at `:1711`, or records deferred/current/queued | yes | Same current/deferred window as `unblock()`. |
| `unblock_for_child_exit()` | `kernel/src/task/scheduler.rs:1796`, `:1799`, `:1819` | child-exit wake path | `BlockedOnChildExit` -> Ready at `:1799` | same lock queues target CPU at `:1820`, or records deferred/current/queued | yes | Same current/deferred window as `unblock()`. |
| `unblock_for_io()` / `wake_waitqueue_thread()` | `kernel/src/task/scheduler.rs:1942`, `:1963`, `:1973`, `:1978`, `:2022` | task-context I/O completion or waitqueue wake | `BlockedOnIO` -> Ready at `:1978` | same lock queues target CPU at `:2023`, or records deferred/current/queued | yes | Yes when the waiter is still current or in deferred requeue. |
| `isr_unblock_for_io()` | `kernel/src/task/scheduler.rs:2915`, `:2918`, `:2919` | hard IRQ completion path | no Ready transition | lock-free per-CPU ISR wake buffer | no scheduler lock | If the buffer is full or no later scheduler path drains it, the wake intent can be lost before Ready publication. |
| `wake_expired_timers()` current thread | `kernel/src/task/scheduler.rs:2080`, `:2128` | scheduler/timer expiry processing | timed wait -> Ready at `:2128` | no queue because the thread is still current; records deferred | yes | Yes, but intentional: the running thread should observe Ready itself, or deferred requeue should later queue it. |
| `wake_expired_timers()` non-current thread | `kernel/src/task/scheduler.rs:2080`, `:2153`, `:2165` | scheduler/timer expiry processing | timed wait -> Ready at `:2153` | same lock queues target CPU at `:2167`, or records deferred/queued | yes | Same deferred/queued window as `unblock()`. |
| `WaitQueue::finish_wait()` | `kernel/src/task/waitqueue.rs:87`, `:94`, `:97` | current thread leaving a waitqueue without sleeping | `BlockedOnIO` -> Ready at `:97` | no queue; this is the current thread normalizing state | waitqueue removal first, then scheduler lock | Not a ready-queue producer. The current thread continues executing and clears `blocked_in_syscall`. |
| `ThreadState::Ready` constructors | `kernel/src/task/thread.rs:570`, `:631`, `:679`, `:726`, `:786`, `:841` | thread construction/tests | initial state, not a wake transition | inserted by caller/add-thread path | caller-dependent | Not part of a blocked/running -> Ready wake race. |

The core invariant gap is still visible in the source: several Breenix paths
publish `ThreadState::Ready` before either same-lock queue insertion succeeds or
the deferred-owner path completes. Linux avoids that gap by using `TASK_WAKING`
until `activate_task()` has placed the task on the run queue, and only then
publishing runnable state.

## B. Rescue mechanism map

| Rescue site | File:Line | Trigger condition | Corrective action | Candidate producer |
| --- | --- | --- | --- | --- |
| Inline queue-empty rescue | `kernel/src/task/scheduler.rs:1216`, `:1225`, `:1245`, `:1287` | `schedule_deferred_requeue()` finds all run queues empty before idle, then finds a Ready thread that is not idle, not current, and not in deferred requeue | increments `RESCUE_INLINE_COUNT`, classifies the last Ready site, increments `READY_THREAD_RESCUE_COUNT`, and dispatches the stuck TID directly | missed deferred requeue, skipped same-lock enqueue after Ready publication, or an ISR wake intent drained into Ready without queue insertion |
| Timer rescue | `kernel/src/task/scheduler.rs:2333`, `:2340`, `:2399`, `:2403` | CPU0 periodic timer path finds Ready threads that are not idle, not queued, not current, and not pending deferred requeue | increments `RESCUE_TIMER_COUNT`, classifies the last Ready site, increments `READY_THREAD_RESCUE_COUNT`, pushes the TID onto a target ready queue, and sends resched IPI | same orphan producers as inline rescue, but detected asynchronously |
| Non-blocking wrapper | `kernel/src/task/scheduler.rs:2704`, `:2705`, `:2707` | timer calls rescue through `SCHEDULER.try_lock()` | skips if contended; otherwise runs timer rescue | not a producer; bounds rescue overhead in timer context |
| Existing rescue total | `kernel/src/task/scheduler.rs:131`, `:135` | incremented by both rescue paths | exported through `ready_thread_rescue_count()` and emitted by freeze-watch lock attribution | comparison total for new rescue buckets |

The new classifier maps the last Ready publication site to buckets:

- `schedule()` / `schedule_deferred_requeue()` -> `dropped`
- ISR-buffer-drained I/O wake -> `isr_lost`
- ordinary unblock/signal/child/timer/wake-io Ready publication -> `wake_no_enq`
- missing/unknown site -> `other`

## C. Counter diff from the boot

Verification:

- Kernel build: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- Build result: `turn2-artifacts/parallels-boot/kernel-build.txt` contains only `Finished release profile`, with no compiler warnings or errors.
- Parallels VM: `breenix-1779212644`
- Cleanup: `prlctl stop breenix-1779212644 --kill`; `prlctl delete breenix-1779212644`; only unrelated `linux-probe` remained in `prlctl list --all`.

The freeze-watch stream reached `uptime_ms=240568`, and the last attribution
emission before the requested 220 s mark was at `uptime_ms=215539`.

| uptime_ms | wake schedule | wake timer | wake unblock/isr/wake_io/signal/child | enqueue same_lock | enqueue deferred | deferred_drained | isr_buf | isr_buf_drained | already_queued | rescue dropped/isr_lost/wake_no_enq/other | rescue inline | rescue timer | rescue total |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| 1740 | 0 | 1 | all 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0/0/0/0 | 0 | 0 | 0 |
| 35307 | 2 | 24 | all 0 | 23 | 3 | 2 | 0 | 0 | 1 | 0/0/0/0 | 0 | 0 | 0 |
| 65350 | 3 | 30 | all 0 | 29 | 4 | 3 | 0 | 0 | 2 | 0/0/0/0 | 0 | 0 | 0 |
| 95391 | 3 | 36 | all 0 | 35 | 4 | 3 | 0 | 0 | 2 | 0/0/0/0 | 0 | 0 | 0 |
| 125429 | 4 | 42 | all 0 | 41 | 5 | 4 | 0 | 0 | 3 | 0/0/0/0 | 0 | 0 | 0 |
| 155462 | 4 | 48 | all 0 | 47 | 5 | 4 | 0 | 0 | 3 | 0/0/0/0 | 0 | 0 | 0 |
| 185497 | 4 | 54 | all 0 | 53 | 5 | 4 | 0 | 0 | 3 | 0/0/0/0 | 0 | 0 | 0 |
| 215539 | 4 | 60 | all 0 | 59 | 5 | 4 | 0 | 0 | 3 | 0/0/0/0 | 0 | 0 | 0 |

Representative raw lines:

```text
turn2-artifacts/parallels-boot/serial.log:76340:[gpu-pci-lock-attrib] max_hold_ms=7 max_hold_holder_tid=-1 rescues=0
turn2-artifacts/parallels-boot/serial.log:76341:[wake-attrib] schedule=4 unblock=0 isr_unblock=0 wake_io=0 signal=0 child=0 timer=60
turn2-artifacts/parallels-boot/serial.log:76342:[enqueue-attrib] same_lock=59 deferred=5 isr_buf=0 deferred_drained=4 isr_buf_drained=0 already_queued=3 isr_buf_full=0
turn2-artifacts/parallels-boot/serial.log:76343:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
```

The run did soft-lock repeatedly and BWM did not make active rendering
progress. The freeze-watch samples stayed at `submits=62`, `completes=65`,
`fps_last_5s=0` from the first sample through the end. That limits this boot's
value for the intended I/O wake attribution question.

## D. Attribution conclusion

This boot did not identify an orphan-Ready producer.

The concrete evidence is:

- `rescue_total=0` for every attribution emission, including the final sample at
  `uptime_ms=215539`.
- `RESCUE_REASON_*` buckets all stayed `0`, so no wake site can be ranked as an
  orphan producer from this run.
- The only wake counters that moved were `schedule=4` and `timer=60`.
- `isr_unblock=0`, `wake_io=0`, and `isr_buf=0`, so the suspected I/O wake
  buffer/drain path did not execute in this boot.
- The last sample accounts for active wake lifecycle counters without rescue:
  `same_lock=59`, `deferred=5`, `deferred_drained=4`, `already_queued=3`, and
  `rescue_total=0`.

So Turn 2 proves the instrumentation and emitter work, but it does not prove
which production wake path caused the prior 5-28 rescue events per boot. The
absence of I/O wake activity means a Turn 3 behavior fix would be speculative.

## E. Proposed Turn 3 fix design

Do not merge a scheduler behavior fix directly from this boot. The next turn
should first force or restore a workload that actually exercises the I/O wake
path and reproduces nonzero rescue attribution.

The fix design to review once attribution is nonzero is:

1. If `RESCUE_REASON_ISR_BUFFER_LOST` or `READY_SITE_WAKE_IO_ISR_DRAIN`
   dominates, change `isr_unblock_for_io()` / `wake_io_thread_locked()` to a
   Linux-style two-phase wake:
   - ISR records a wake intent only.
   - Scheduler drain moves the thread to a transient `Waking`/`WakePending`
     state under the scheduler lock.
   - The target ready queue insert happens while the thread is still not
     externally Ready.
   - Only after queue insertion succeeds does the scheduler publish
     `ThreadState::Ready`.
2. If `RESCUE_REASON_DEFERRED_DROPPED` dominates, apply the same two-phase
   rule to `schedule_deferred_requeue()` / `requeue_thread_after_save()`:
   keep the outgoing thread in a non-runnable `RequeuePending` state while its
   context is unsafe to dispatch, enqueue it after the save completes, then
   publish Ready.
3. If `RESCUE_REASON_WAKE_WITHOUT_ENQUEUE` dominates ordinary unblock/timer
   paths, factor those paths through one helper that performs
   state-transition-plus-enqueue as an atomic scheduler-lock operation, using a
   pending state whenever current/deferred ownership prevents immediate queue
   insertion.

That is the Linux `TASK_WAKING -> activate_task() -> TASK_RUNNING` sequence
translated to Breenix: no observer should see `ThreadState::Ready` until the
thread is either queued, current, or explicitly pending owner-side requeue.
