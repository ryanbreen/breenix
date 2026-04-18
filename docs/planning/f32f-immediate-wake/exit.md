# F32f Exit - Immediate Wake Under Waitqueue Lock

## Status

Stopped after Phase 3 Run 1 failed the Parallels boot gate. Do not merge this branch as a completed F32f fix.

Implemented work remains useful evidence:

- Audit committed: `acbe68d7 docs(kernel): F32f audit Linux immediate wake path`
- Immediate task-context waitqueue wake committed: `7674d3c5 fix(kernel): F32f immediate waitqueue wake from task context`

## Original Ask

F32f tested whether Breenix's remaining bwm/bsshd Parallels stall was caused by a Linux-parity gap in the waitqueue wake path. Linux calls `try_to_wake_up` from waitqueue traversal under `wq_head->lock`, while Breenix F32e deferred waitqueue wakes through the ISR wake ring. The requested change was to wake task-context waitqueue callers inline and keep the deferred ring only for genuine interrupt-context wakers.

## Audit Findings

Linux `__wake_up_common_lock` holds `wq_head->lock`, calls `__wake_up_common`, then releases the lock (`/tmp/linux-v6.8/kernel/sched/wait.c:99-108`). `__wake_up_common` invokes each waiter's wake function while that lock is held (`/tmp/linux-v6.8/kernel/sched/wait.c:73-96`).

Linux `autoremove_wake_function` calls `default_wake_function` and removes the wait-list entry only after a successful wake (`/tmp/linux-v6.8/kernel/sched/wait.c:382-389`). `try_to_wake_up` documents the contract: if the requested state matches, set the task to running and enqueue it if needed (`/tmp/linux-v6.8/kernel/sched/core.c:4186-4197`).

Linux takes scheduler locks inside the waitqueue traversal, not the other way around: `try_to_wake_up` uses `p->pi_lock`, state matching, optional `TASK_WAKING`, and `ttwu_queue` while the waitqueue wake function is still executing (`/tmp/linux-v6.8/kernel/sched/core.c:4247-4369`). Remote delivery uses wake-list/IPI machinery as delivery support after the state decision (`/tmp/linux-v6.8/kernel/sched/core.c:3930-3944`), while local enqueue takes the runqueue lock directly (`/tmp/linux-v6.8/kernel/sched/core.c:4038-4049`).

Breenix F32e was not equivalent for task-context waitqueue callers: `WaitQueueHead::wake_up` removed waiters and pushed their TIDs into `isr_unblock_for_io`; the real `BlockedOnIO -> Ready` transition occurred only later when a scheduler drain called `unblock_for_io`.

## Implementation

`WaitQueueHead::wake_up` and `wake_up_one` now pop waiters while holding the waitqueue lock and call `wake_waiter` inline (`kernel/src/task/waitqueue.rs:105-120`).

`wake_waiter` checks interrupt context. Interrupt-context callers still use `scheduler::isr_unblock_for_io`; task-context callers use `scheduler::wake_waitqueue_thread` (`kernel/src/task/waitqueue.rs:172-188`).

`Scheduler::wake_waitqueue_thread` reuses the common I/O wake transition path, which sets `BlockedOnIO` threads ready, clears `wake_time_ns`, queues them when their old CPU no longer owns the thread, sets `need_resched`, and sends a targeted AArch64 reschedule IPI for remote targets (`kernel/src/task/scheduler.rs:1713-1772`, `kernel/src/task/scheduler.rs:1346-1362`, `kernel/src/task/scheduler.rs:2467-2473`).

The ISR wake path remains in place for AHCI/completion-style hard IRQ callers (`kernel/src/task/scheduler.rs:2626-2645`).

## Validation

Build gates passed clean:

- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`

Formatting note: full-workspace `cargo fmt` is blocked by pre-existing trailing whitespace in unrelated test files. The changed kernel files were formatted with targeted `rustfmt`, and `git diff --check` passed.

Wait-stress passed:

- Command: `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150`
- Evidence: `WAIT_STRESS_PASS entered=596936 returned=596935 wakes=700683 waiters=0`
- No `WAIT_STRESS_STALL`
- No AHCI timeout markers
- Strict screenshot verdict: `PASS`

Parallels normal boot failed on the first 120s run:

- Command: `./run.sh --parallels --test 120`
- bsshd launched and listened
- Serial stopped at `[spawn] path='/bin/bounce'`
- Missing `[init] bounce started`
- Missing `[bounce] Window mode`
- Missing sustained `Frame #...` cadence for FPS validation
- Missing CPU0 tick evidence
- No AHCI timeout markers
- Strict screenshot verdict: `PASS`

## Sweep Table

| Gate | Result | Evidence |
| --- | --- | --- |
| Standard x86_64 build | PASS | `/tmp/f32f-build-clean.log` |
| AArch64 kernel build | PASS | `/tmp/f32f-aarch64-build-clean.log` |
| wait_stress 60s+ | PASS | `.factory-runs/f32f-immediate-wake/wait-stress.serial.log:417` |
| Parallels 120s run 1 | FAIL | `.factory-runs/f32f-immediate-wake/parallels-run1.serial.log:387-390` |
| Parallels runs 2-5 | NOT RUN | Stopped per F32f instruction after run 1 failed |

## Decision

The immediate-wake Linux-parity gap existed and was closed, but the required Parallels boot sweep did not pass. Since wait-stress passed and the normal Parallels boot still failed, the remaining issue is not waitqueue wake latency. Per the run instructions, stop here and do not iterate another mechanism change.

## PR

No PR opened. The 5/5 Parallels requirement was not met.
