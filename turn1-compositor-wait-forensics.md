# Turn 1 Compositor-Wait Forensics

## A. Branch State

- Working branch: `fix/compositor-wait-softlock`.
- Starting HEAD: `980f12c3 Merge pull request #340 from ryanbreen/fix/virgl-send-command-deref`.
- Preflight working tree was clean before this deliverable was added.
- No kernel source edits were made in this turn.
- No new boot, QEMU, or VM run was performed.
- A release AArch64 kernel build was performed for disassembly only:
  `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`.
  The build completed cleanly with no observed warnings.

## B. Compositor-Wait Call Chain

The compositor wait path is in `kernel/src/syscall/graphics.rs`.

- `COMPOSITOR_FRAME_WQ` is the static wait queue used by the BWM compositor wait operation (`graphics.rs:44-47`).
- `CLIENT_FRAME_WQ` is the static wait queue used by client frame pacing after `mark_window_dirty` (`graphics.rs:51-52`).
- `COMPOSITOR_DIRTY_WAKE` is a single `AtomicBool` latch set by `mark_window_dirty` and consumed by `compositor_ready_bits` (`graphics.rs:81-85`, `graphics.rs:187-209`).
- `wake_compositor_if_waiting()` calls `COMPOSITOR_FRAME_WQ.wake_up()` (`graphics.rs:87-93`).

`handle_compositor_wait` is implemented as a loop (`graphics.rs:1303-1358`):

1. Compute `ready = compositor_ready_bits(last_registry_gen, prev_mouse)`.
2. If `ready != 0`, store the current mouse state and return the packed result to userspace.
3. Otherwise, call `COMPOSITOR_FRAME_WQ.prepare_to_wait(ThreadState::BlockedOnIO)`.
4. Re-check `compositor_ready_bits` after publishing the blocked state. If ready, call `finish_wait()` and return.
5. If still not ready, call `waitqueue::schedule_current_wait()`.
6. After scheduling returns, call `COMPOSITOR_FRAME_WQ.finish_wait()`, restore the current address space, and loop.

The readiness bits are:

- Bit 0: `COMPOSITOR_DIRTY_WAKE.swap(false, Ordering::Relaxed)` returned true.
- Bit 1: mouse state or pending input changed.
- Bit 2: window registry generation changed.

The notable limitation is that compositor readiness does not directly scan the window registry for a pending dirty frame. Dirty-window readiness is represented by a consumable one-bit latch.

The related client-side producer/waiter is `mark_window_dirty` (`graphics.rs:1002-1055`). It:

- Bumps the target buffer generation.
- Records `waiting_thread_id = Some(thread_id)`.
- Sets `COMPOSITOR_DIRTY_WAKE.store(true, Ordering::Relaxed)`.
- Wakes `COMPOSITOR_FRAME_WQ`.
- Then waits on `CLIENT_FRAME_WQ` while `window_frame_pending(buffer_id, thread_id)` remains true.

The compositor clears that client wait by compositing dirty windows in `handle_composite_windows` (`graphics.rs:1476-1601`):

- It collects windows whose generation needs upload.
- It records `last_uploaded_gen = generation`.
- It takes `waiting_thread_id` into a local wake list.
- After the VirGL work, it wakes `CLIENT_FRAME_WQ` if any client waiters were collected.

## C. Waitqueue API Contract

The waitqueue implementation is in `kernel/src/task/waitqueue.rs`.

`WaitQueueHead::prepare_to_wait` (`waitqueue.rs:52-81`) does two things under the waitqueue lock:

- Adds the current TID to the waitqueue if it is not already present.
- Calls `scheduler::with_scheduler(|sched| sched.publish_current_io_wait_state())`.

If publishing the blocked state fails, it removes the waiter again and returns `None`. Only `ThreadState::BlockedOnIO` is supported.

`WaitQueueHead::finish_wait` (`waitqueue.rs:87-103`) removes the current thread from the waitqueue. If the current scheduler state is still `BlockedOnIO`, it marks the thread ready and clears `wake_time_ns`; it always clears `blocked_in_syscall`.

`WaitQueueHead::wake_up` drains the waiter list and calls `wake_waiter` for each TID (`waitqueue.rs:105-112`). In task context, `wake_waiter` calls `scheduler::wake_waitqueue_thread(tid)` (`waitqueue.rs:172-178`).

`schedule_current_wait` (`waitqueue.rs:197-225`) is the key scheduler loop:

- It enables preemption before sleeping.
- It repeatedly asks the scheduler whether the current thread is still `BlockedOnIO`.
- If the scheduler is unavailable or the current thread is no longer `BlockedOnIO`, it exits the loop.
- On AArch64, while still blocked, it calls `schedule_from_kernel()`.
- It disables preemption before returning to the syscall path.

The scheduler side is in `kernel/src/task/scheduler.rs`.

- `with_scheduler` locks the scheduler with interrupts disabled and returns `Option<R>` (`scheduler.rs:2433-2471`).
- `publish_current_io_wait_state_inner` sets the current thread state to `BlockedOnIO`, sets `wake_time_ns`, sets `blocked_in_syscall = true`, and issues a `SeqCst` fence (`scheduler.rs:1686-1705`).
- `wake_waitqueue_thread` calls `wake_io_thread_locked` (`scheduler.rs:1769-1782`).
- `wake_io_thread_locked` marks a `BlockedOnIO` thread ready and queues it unless it is still the current thread on a CPU; it also has a branch for `Ready && blocked_in_syscall` (`scheduler.rs:1784-1828`).

The intended contract is therefore:

- Waiter publishes `BlockedOnIO`.
- Waker changes it to `Ready`.
- `schedule_current_wait` exits once it observes any state other than `BlockedOnIO`.
- The syscall-specific loop then calls `finish_wait` and rechecks the domain predicate.

## D. Disassembly Cross-Reference

The post-PR-340 AArch64 binary was disassembled from `target/aarch64-breenix/release/kernel-aarch64`.

`rust-nm -C` showed:

- `kernel::syscall::graphics::handle_virgl_op` at `ffff0000400e5550`.
- Multiple monomorphized `scheduler::with_scheduler` instances, including `ffff0000400a3d94` for the `schedule_current_wait` predicate.

The compositor wait loop in `handle_virgl_op` contains this inlined `schedule_current_wait` sequence:

```text
ffff0000400e66ec: bl  preempt_enable
ffff0000400e66f0: bl  with_scheduler
ffff0000400e66f4: and w8, w0, #0xff
ffff0000400e66f8: cmp w8, #0x2
ffff0000400e66fc: b.eq 0xffff0000400e6718
ffff0000400e6700: tbz w8, #0x0, 0xffff0000400e6718
ffff0000400e6704: bl  schedule_from_kernel
ffff0000400e6708: bl  with_scheduler
ffff0000400e670c: and w8, w0, #0xff
ffff0000400e6710: cmp w8, #0x2
ffff0000400e6714: b.ne 0xffff0000400e6700
ffff0000400e6718: bl  preempt_disable
ffff0000400e6724: bl  WaitQueueHead::finish_wait
```

This is not comparing a thread-state enum to `2`. It is checking the niche encoding of `Option<bool>` returned by `with_scheduler`:

- `Some(true) = 1`: current thread is still `BlockedOnIO`; keep scheduling.
- `Some(false) = 0`: current thread is no longer `BlockedOnIO`; exit.
- `None = 2`: scheduler unavailable; exit.

The `with_scheduler` monomorph confirms this. It loads the actual Rust enum byte for the current thread state and compares it to `#0x6`, the compiled discriminant for `BlockedOnIO`; it returns `1` only for that case and returns `2` for `None`.

The client-side `mark_window_dirty` wait loop maps to a later sequence in `handle_virgl_op`:

```text
ffff0000400e7590: bl  WaitQueueHead::prepare_to_wait
...
ffff0000400e75f4: bl  preempt_enable
ffff0000400e75f8: bl  with_scheduler
...
ffff0000400e760c: bl  schedule_from_kernel
ffff0000400e7610: bl  with_scheduler
...
ffff0000400e761c: b.ne 0xffff0000400e7608
```

That saved LR (`0xffff0000400e7610`) is the client frame-pacing wait path, not the compositor wait path.

## E. Reproduction Artifact Deep-Read

Artifacts were read from:
`/Users/wrb/Downloads/Ralph/breenix-virgl-send-command-deref-1779130148/turn3-artifacts/reproduce-run1/`.

The useful files were:

- `window.serial.log`: concise freeze-watch, frame, and softlock timeline.
- `run.serial.log`: full serial log including per-thread softlock dumps and trace records.
- `signals.log` and `full-signals.log`: filtered freeze-watch/softlock signals only; they do not contain the detailed per-thread dump.
- `gdb_freeze_state.out`: lock and CPU-idle snapshot, but no waitqueue-head or compositor-domain state.

Softlock thread dump interpretation:

| TID | State | Meaning | Symbolized site |
| --- | --- | --- | --- |
| 2 | `B` | blocked kernel thread | `kernel::task::kthread::kthread_park` |
| 3 | `T bis wt` | timer-sleeping freeze watchdog | `kernel::drivers::virtio::gpu_pci::freeze_watchdog_thread` |
| 11 | `C bis user pid=1` | blocked in waitpid | `kernel::syscall::wait::sys_waitpid` |
| 12 | `T bis wt user pid=2` | nanosleep | `kernel::syscall::time::sys_nanosleep` |
| 13 | `R bis inl user pid=3` | BWM, saved in compositor wait | saved LR `0xffff0000400e6708` in `handle_virgl_op` |
| 14 | `B bis user pid=4` | socket accept | `kernel::syscall::socket::sys_accept` |
| 15 | `B bis user pid=5` | socket accept | `kernel::syscall::socket::sys_accept` |
| 16 | `? bis inl user pid=6` | client frame wait | saved LR `0xffff0000400e7610` in `handle_virgl_op` |

The `state=?` entry is diagnosable: `try_dump_state` maps `ThreadState::BlockedOnIO` to external code `7` (`scheduler.rs:373-384`), while the AArch64 softlock printer only renders codes `0..6` and prints anything else as `?` (`timer_interrupt.rs:1128-1143`). Therefore TID 16 is best read as `BlockedOnIO`, not an unknown runtime state.

Representative softlock state:

- TID 13, BWM, is `Ready` but still saved inside the compositor wait scheduling path at `0xffff0000400e6708`.
- TID 16, a user client, is `BlockedOnIO` inside the `mark_window_dirty`/`CLIENT_FRAME_WQ` wait path at `0xffff0000400e7610`.
- Ready queues are empty while the detector reports `stuck_tid=13`.

The concise timeline from `window.serial.log`:

- Last pre-onset freeze-watch sample:
  `uptime_ms=205408 submits=97049 completes=97052 fails=0 last_completion_ms=205406 fps_last_5s=155 ... total_threads=16 blocked_threads=6 sched_lock=ok procmgr_lock=ok gpu_pci_lock=ok`.
- Last frame marker before the first softlock:
  `[virgl-composite] Frame #32500 ...`.
- Then the scheduler reports:
  `[SCHED] queue_empty stuck_tid=13 count=0`, `1`, `2`, `3`, `4`, then later `1000`.
- The softlock banner follows.

The detailed trace in `run.serial.log` shows a successful BWM composite immediately before the wait:

```text
BWM_COMPOSITE_FRAME_ENTER
VIRTGPU_Q_COMPLETE cmd_type=131589
VIRTGPU_WAIT_COMPLETION_EXIT cmd_type=131589
VIRTGPU_Q_COMPLETE cmd_type=131331
VIRTGPU_WAIT_COMPLETION_EXIT cmd_type=131331
VIRTGPU_Q_COMPLETE cmd_type=131332
VIRTGPU_WAIT_COMPLETION_EXIT cmd_type=131332
BWM_COMPOSITE_FRAME_EXIT status=0
```

The next traced BWM-side syscall activity enters the wait path. The trace timestamp delta from the last successful VirGL completion/exit at `ts=5054061015` to the next captured stuck-loop syscall entry at `ts=5054061711` is 696 trace ticks. The artifacts do not print an exact `uptime_ms` for the first `queue_empty` line, so the millisecond gap to the first softlock cannot be derived exactly from serial alone. The nearest serial constraints are `last_completion_ms=205406`, the final pre-onset freeze-watch at `uptime_ms=205408`, and the subsequent queue-empty sequence.

`gdb_freeze_state.out` shows no scheduler or process-manager lock leak:

```text
scheduler_lock_byte=0 scheduler_word=0x0
process_manager_lock_byte=0 process_owner_cpu=0xffffffffffffffff process_owner_tid=0xffffffffffffffff
gpu_pci_lock_byte=1 gpu_pci_word=0x1
need_resched_byte=0 context_switch_count=247624
cpu_is_idle: ... 0x01 0x01 0x00 0x01 0x01 0x01 0x01 0x01
```

The GPU PCI lock byte is set in that snapshot, but earlier freeze-watch lines at the onset report `gpu_pci_lock=ok`. The snapshot is useful for ruling out scheduler/process-manager lock ownership; it is not enough to prove a GPU lock root cause.

One important nuance: the full `run.serial.log` later shows additional frames and freeze-watch output after the first softlock marker. This artifact demonstrates a repeated stall/softlock class, not necessarily a single permanent VM death at the first banner.

## F. Hypothesis Table

| Hypothesis | Mechanism | Supporting evidence | Contradicting evidence | Best next probe |
| --- | --- | --- | --- | --- |
| H1: producer/consumer circular wait between BWM and a client | BWM waits on `COMPOSITOR_FRAME_WQ`; client TID 16 waits on `CLIENT_FRAME_WQ` for BWM to composite its dirty frame. If BWM misses dirty readiness or is not resumed, both sides can wait. | TID 13 is saved in compositor wait at `0xffff0000400e6708`; TID 16 is saved in client frame wait at `0xffff0000400e7610`; ready queues are empty; TID 16 is effectively `BlockedOnIO`. | BWM is `Ready`, not `BlockedOnIO`, so the immediate issue may be scheduler requeue/resume rather than a pure domain deadlock. The run later resumes. | At softlock, inspect `COMPOSITOR_DIRTY_WAKE`, both waitqueues, and window registry `generation`, `last_uploaded_gen`, and `waiting_thread_id`. |
| H2: dirty-wake latch loses persistent frame readiness | `COMPOSITOR_DIRTY_WAKE` is consumed with `swap(false)` and compositor readiness does not scan dirty window generations. A pending client frame can exist while the compositor dirty bit is false. | TID 16 remains in the client dirty-frame wait; compositor readiness depends on a single consumable bit for dirty work; the post-prepare race check rechecks the same consumable predicate. | `mark_window_dirty` stores true before waking, and the wait pattern has a recheck. Single-consumer lost wake is less likely unless the persistent dirty condition is missing from the predicate. | GDB inspect whether a window has `waiting_thread_id=Some(16)` and `generation > last_uploaded_gen` while `COMPOSITOR_DIRTY_WAKE=false`. |
| H3: ready-but-inline waiter is not requeued or resumed | Wake side transitions BWM from `BlockedOnIO` to `Ready`, but because it is still current/inline-scheduled, it is not placed on a runqueue and does not return through `schedule_current_wait`. | TID 13 is `R bis inl` with saved LR in the wait loop; `[SCHED] queue_empty stuck_tid=13` repeats; GDB says scheduler lock is free and `need_resched_byte=0`; the loop would exit immediately if it observed `Some(false)`. | Previous scheduler work addressed an inline schedule trampoline leak; this would be another edge, not the same proven bug. We do not yet have deferred-requeue data at the exact stuck instant. | Inspect per-CPU current thread, run queues, deferred requeue list, `blocked_in_syscall`, and saved inline-schedule metadata for TID 13. |
| H4: disassembly compares the wrong enum value | The `cmp #2` sequence might be interpreted as checking the wrong `ThreadState`, causing the loop to continue incorrectly. | The initial symptom looked like a suspicious compare, and the diagnostic state printer has a separate enum mapping bug. | Refuted by disassembly: the caller checks `Option<bool>` encoding, and the monomorph compares the actual thread state byte against `#0x6` for `BlockedOnIO`. `Some(false)` and `None` both exit. | No runtime probe needed; keep this as a documentation correction. |
| H5: preemption or need-resched state is wrong around `schedule_current_wait` | `schedule_current_wait` enables preemption, calls into scheduler/inline schedule, then disables preemption. A stale preempt or resched state could leave the thread ready but not runnable. | TID 13 is ready, inline, and not on a ready queue; most CPUs are idle; `need_resched_byte=0`; the saved site is inside the wait scheduling loop. | Timer ticks continue, locks are free, and the artifact later makes progress. No preempt-depth snapshot is present. | GDB inspect per-CPU preempt depth, IRQ state, current thread, and need-resched across CPUs at softlock. |

## G. Best-Supported Hypothesis

The strongest conclusion from turn 1 is that the runtime loop itself is not stuck because the AArch64 code compares a thread-state enum to the wrong value. The `cmp #2` sequence is the expected `Option<bool>` encoding for `with_scheduler`.

The best-supported failure shape is a ready-but-not-running compositor waiter:

- BWM TID 13 has been moved out of `BlockedOnIO` into `Ready`.
- It is still saved inside the compositor wait `schedule_current_wait` return path.
- Ready queues are empty and the detector repeatedly reports `stuck_tid=13`.
- A client TID 16 is simultaneously blocked in the `mark_window_dirty` client wait path, which means there is likely a frame-pacing dependency waiting for BWM to composite and wake `CLIENT_FRAME_WQ`.

That points to the scheduler/waitqueue handoff for an inline-scheduled waitqueue thread as the first place to inspect, with the compositor dirty-latch state as the domain-specific companion probe. A good turn 2 would be a non-intrusive GDB reproduction snapshot that captures:

- `COMPOSITOR_DIRTY_WAKE`.
- `COMPOSITOR_FRAME_WQ` and `CLIENT_FRAME_WQ` waiter lists.
- Window registry `generation`, `last_uploaded_gen`, and `waiting_thread_id`.
- Scheduler state for TID 13 and TID 16, including per-CPU current thread, runqueue/deferred-requeue membership, `blocked_in_syscall`, and inline-schedule saved state.
