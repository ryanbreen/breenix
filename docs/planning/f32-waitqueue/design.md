# F32 WaitQueue Design

## Context

F32 replaces the compositor's ad-hoc waiting protocol with a scheduler-integrated
waitqueue primitive. The immediate target is `compositor_wait` in
`kernel/src/syscall/graphics.rs`, which currently publishes
`COMPOSITOR_WAITING_THREAD`, blocks with `BlockedOnTimer`, and relies on a 5 ms
fallback timer when an event wake is missed.

F31 established that F28's seqlock-style wake handshake was the wrong layer. It
sat outside the scheduler's existing `BlockedOnIO` and `isr_unblock_for_io`
machinery, so waiter publication and wake delivery disagreed under AArch64 SMP.
The replacement should look more like Linux waitqueues: the waiter is enrolled
and put into a scheduler sleep state before the condition is rechecked, and the
waker drives the scheduler's normal wake path.

## Linux Reference

Linux splits waitqueues into two structures:

- `wait_queue_head`: a spinlock plus a linked list of wait entries.
- `wait_queue_entry`: a per-waiter node containing the task pointer, flags, wake
  function, and list linkage.

The important semantics are in `prepare_to_wait`, `finish_wait`, and
`__wake_up`:

1. `prepare_to_wait` takes the waitqueue lock, adds the entry if it is not
   already queued, sets the task state, then unlocks. Linux intentionally sets
   the task state after queue insertion so the state-setting barrier pairs with
   wake-side checks.
2. The waiter checks the condition after `prepare_to_wait`.
3. If the condition is still false, the waiter calls `schedule`.
4. `wake_up` takes the same queue lock and calls each waiter's wake function.
   The default wake function routes through the scheduler's task wake path.
5. `finish_wait` sets the current task running and removes the wait entry if it
   remains queued.

The race closure is structural: a wake that lands after queue insertion but
before the condition check sets the task runnable. The later `schedule` does not
sleep a runnable task.

## Breenix Existing Primitives

### Thread States

`kernel/src/task/thread.rs` currently has:

- `Running`
- `Ready`
- `Blocked`
- `BlockedOnSignal`
- `BlockedOnChildExit`
- `BlockedOnTimer`
- `BlockedOnIO`
- `Terminated`

For F32, waitqueues should initially use `BlockedOnIO` for event-driven
syscall waits. This is not semantically perfect for compositor events, but it is
the only state today with the correct F16 wake delivery path:

- `Scheduler::block_current_for_io_with_timeout(None)` marks the current thread
  `BlockedOnIO`, sets `blocked_in_syscall = true`, clears `wake_time_ns`, and
  removes it from ready queues.
- `Scheduler::unblock_for_io(tid)` transitions `BlockedOnIO` to `Ready`, keeps
  `blocked_in_syscall` set for the waiter to clear after resuming, avoids
  double-scheduling if the thread is still current on a CPU or in deferred
  requeue, sends a reschedule IPI on AArch64, and sets `need_resched`.
- `isr_unblock_for_io(tid)` is the lock-free ISR entry point. It enqueues the
  TID into per-CPU atomic wake buffers that the scheduler drains before making a
  scheduling decision.

### Completion

`kernel/src/task/completion.rs` already implements a one-waiter special case
that resembles a waitqueue:

- A `done` token is published before wake.
- A `waiter` TID is stored before sleeping.
- The syscall sleep path uses `block_current_for_io_with_timeout`.
- `complete()` wakes with `isr_unblock_for_io(tid)`.

Completion should not be migrated in the first F32 implementation unless the
waitqueue primitive proves stable under the compositor migration. It carries
timeout and early-boot spin paths that would expand the blast radius.

### Current Compositor Wait

The compositor path has two separate waits:

- Client-side frame pacing in op 15 (`mark_window_dirty`) blocks the client
  until BWM consumes the frame. It currently stores `waiting_thread_id` in the
  `WindowBuffer` and sleeps with `block_current_for_compositor(timeout_ns)`,
  which is a `BlockedOnTimer` fallback path.
- BWM-side op 23 (`compositor_wait`) blocks until a dirty window, mouse input,
  keyboard/input latch, or registry change. It currently uses
  `COMPOSITOR_WAITING_THREAD`, `COMPOSITOR_DIRTY_WAKE`, and
  `block_current_for_compositor(timeout_ns)`.

F32's required migration target is the BWM-side `compositor_wait` and its wake
producers. The client frame-pacing path is adjacent and still timer-backed; it
should remain out of scope unless the migration explicitly expands after the
required validation passes.

## Proposed API

Add `kernel/src/task/waitqueue.rs`:

```rust
pub struct WaitQueueHead {
    waiters: SpinLock<WaitQueueState>,
}

struct WaitQueueState {
    waiters: VecDeque<Waiter>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiter {
    tid: u64,
}
```

The user request names `IntrusiveList<Waiter>`. Breenix does not currently have
a general intrusive list primitive in `kernel/src`, and a waitqueue entry whose
lifetime lives on the caller's stack is awkward in safe Rust. The first
implementation should use a bounded, duplicate-free queue of TIDs behind a
spinlock. That preserves the Linux semantics that matter for Breenix:

- one logical wait entry per waiting thread per queue;
- queue mutation is serialized;
- wake routes to the scheduler;
- finish removes a stale waiter.

If waitqueue contention or allocation becomes a concern, the internals can later
be replaced with an intrusive node type without changing callers.

Public methods:

```rust
impl WaitQueueHead {
    pub const fn new() -> Self;
    pub fn prepare_to_wait(&self, state: ThreadState) -> Option<u64>;
    pub fn finish_wait(&self);
    pub fn wake_up(&self);
    pub fn wake_up_one(&self);
    pub fn has_waiters(&self) -> bool;
}
```

`prepare_to_wait` returns the current TID on success so callers can keep local
diagnostics if needed. It accepts a `ThreadState` because Linux accepts a task
state, but F32 should only support `ThreadState::BlockedOnIO` initially. Other
states can return `None` or fall back to a private scheduler method only when
there is a concrete caller. Supporting every blocked state on day one would
duplicate scheduler policy.

## Prepare, Schedule, Finish Semantics

Breenix callers should follow this pattern:

```rust
loop {
    waitq.prepare_to_wait(ThreadState::BlockedOnIO);
    if condition_is_ready() {
        break;
    }
    waitqueue::schedule_current_wait();
}
waitq.finish_wait();
```

`prepare_to_wait(BlockedOnIO)` must perform the queue insertion and scheduler
state transition without a missed-wake gap:

1. Get `current_thread_id`.
2. Take the waitqueue lock and insert the TID if it is not already present.
3. While still in the same high-level prepare operation, call
   `scheduler::with_scheduler` and set the current thread to `BlockedOnIO`.
4. The caller checks the condition after prepare.

There are two lock-order options:

- Waitqueue lock then scheduler lock.
- Scheduler lock then waitqueue lock.

F32 should use waitqueue lock then scheduler lock and document that scheduler
code must not call back into waitqueue while holding `SCHEDULER`. Wakers do not
take the scheduler lock directly when using `isr_unblock_for_io`; they only push
TIDs into F16's lock-free ISR buffer. That keeps wake paths compatible with IRQ
context and avoids adding a scheduler-lock dependency to input handlers.

After the condition check, `schedule_current_wait()` should enable preemption
for syscall callers and enter the same sleep path used by completion:

- On AArch64, use `schedule_from_kernel()` for a true scheduler switch where
  possible.
- On non-AArch64, use `halt_with_interrupts()`.

The existing compositor code currently loops around `yield_current()` and
`arch_halt_with_interrupts()`. That path specifically needs validation because
F31 called out `Aarch64Cpu::halt_with_interrupts()` as a failure path for long
syscall waits. The waitqueue primitive should provide a shared helper so the
compositor does not hand-roll another wait loop.

`finish_wait` should:

1. Remove the current TID from the queue if still present.
2. If the current thread is still in the wait state, mark it `Ready`.
3. Clear `blocked_in_syscall` only after the caller has resumed and is ready to
   return from the syscall.

The final state normalization mirrors Linux `finish_wait`, but in Breenix the
`blocked_in_syscall` flag is deliberately owned by the sleeping syscall, not by
the wake side.

## Wake-Up Integration With F16

`wake_up` and `wake_up_one` should be implemented as queue operations plus F16
wake delivery:

1. Take the waitqueue lock.
2. Pop all waiters for `wake_up`, or one waiter for `wake_up_one`.
3. Drop the waitqueue lock.
4. For each TID, call `scheduler::isr_unblock_for_io(tid)`.

Using `isr_unblock_for_io` even from non-ISR contexts is intentional for the
first version:

- It is lock-free and therefore safe for input interrupt producers.
- It centralizes wake delivery in the scheduler drain path.
- It avoids calling `Scheduler::unblock` from graphics/input code while holding
  unrelated locks.
- It preserves F16's deferred-requeue and current-on-CPU safety checks.

The tradeoff is wake latency: the TID becomes runnable when the scheduler next
drains the ISR wake buffers. Existing F16 paths already depend on that behavior,
and `isr_unblock_for_io` requests rescheduling on AArch64.

## Compositor Migration Plan

Add:

```rust
#[cfg(target_arch = "aarch64")]
static COMPOSITOR_FRAME_WQ: WaitQueueHead = WaitQueueHead::new();
```

Replace `COMPOSITOR_WAITING_THREAD` as the BWM-side waiter registry:

| Current path | F32 path |
| --- | --- |
| `wake_compositor_if_waiting()` loads `COMPOSITOR_WAITING_THREAD` and calls `sched.unblock(tid)` | call `COMPOSITOR_FRAME_WQ.wake_up()` |
| op 12 registry change manually unblocks BWM | bump `REGISTRY_GENERATION`, call `COMPOSITOR_FRAME_WQ.wake_up()` |
| op 15 dirty wake stores `COMPOSITOR_DIRTY_WAKE` and manually unblocks BWM | store `COMPOSITOR_DIRTY_WAKE`, call `COMPOSITOR_FRAME_WQ.wake_up()` |
| `cleanup_windows_for_pid` wakes via `wake_compositor_if_waiting()` | call the same waitqueue-backed helper |
| op 23 publishes `COMPOSITOR_WAITING_THREAD`, blocks with `block_current_for_compositor(timeout_ns)`, waits for timeout or wake | prepare on `COMPOSITOR_FRAME_WQ`, recheck dirty/input/registry, schedule only if still not ready, finish |

The 5 ms fallback timer must be removed from op 23. `timeout_ms` can remain an
ABI argument, but the migrated implementation should not convert it into a
fallback timer for the compositor wait. The syscall should block until an actual
dirty/input/registry event wakes the waitqueue, then recheck and return the
ready bitmask.

The existing `COMPOSITOR_LAST_WAKE_NS` / `MIN_FRAME_INTERVAL_NS` pacing sleep is
separate from the missed-wake fallback. It is CPU/FPS pacing, not event
delivery. For the strictest reading of "remove the 5ms fallback timer entirely,"
F32 should remove only the event wait fallback and keep pacing only if it does
not block event wake delivery. If validation shows pacing sleep can mask event
wakes, it should also move to a waitqueue-aware sleep or be removed.

## Race Closure

For `compositor_wait`, the race-safe sequence is:

1. Compute current condition bits.
2. If nonzero, return.
3. `COMPOSITOR_FRAME_WQ.prepare_to_wait(BlockedOnIO)`.
4. Recompute condition bits.
5. If nonzero, `finish_wait` and return.
6. Enable preemption and schedule/halt until the scheduler wake path changes
   this thread back to `Ready`.
7. Disable preemption, restore address space, `finish_wait`.
8. Recompute condition bits and return.

If a producer wakes between steps 3 and 4, the queue contains this TID and the
thread is already `BlockedOnIO`. `wake_up` removes the TID and pushes it through
`isr_unblock_for_io`; when drained, `unblock_for_io` marks the thread `Ready`.
The post-prepare condition check sees the event. If the caller still reaches the
schedule helper before the scheduler drain, the scheduler drain executes before
the next scheduling decision and makes the thread runnable.

## Tests

Unit-level coverage should focus on queue semantics that can be checked without
booting QEMU:

- duplicate `prepare_to_wait` for the same TID does not duplicate queue entries;
- `wake_up_one` removes one waiter;
- `wake_up` drains all waiters;
- `finish_wait` removes the current waiter if still queued.

The true race closure is a system property involving scheduler state,
preemption, and AArch64 wake delivery. It must be validated by the Phase 4 boot
sweep rather than treated as proven by unit tests.

## Migration Candidates Beyond Compositor

| Path | Candidate? | Notes |
| --- | --- | --- |
| BWM `compositor_wait` | Yes, Phase 3 target | Direct event wait with current missed-wake fallback. |
| Compositor input/registry/dirty producers | Yes, Phase 3 target | Wakers should use `wake_up`. |
| `Completion` | Maybe, Phase 5 only | Already uses `BlockedOnIO` and `isr_unblock_for_io`, but includes token, timeout, signal, and early-boot spin behavior. |
| Socket accept/recv waits | Later | Good fit after waitqueue survives compositor validation. |
| Pipes/stdin waiters | Later | Likely fit, but may need wake-one semantics and signal interruption. |
| Child exit and signal waits | Later | Need state-specific semantics; do not fold into first waitqueue patch. |

## Validation Requirements

Phase 4 must prove that F32 did not repeat F28:

- clean AArch64 build;
- five 120-second Parallels runs, all passing the requested serial and FPS
  criteria;
- CPU0 `tick_count > 1000` in the end-state audit;
- no AHCI timeout;
- `scripts/f23-render-verdict.sh` passes;
- a long syscall-wait smoke that exercises `Aarch64Cpu::halt_with_interrupts()`
  while bounce is rendering.

If any sweep run fails, the branch should stop before PR/merge and record the
failure signature.
