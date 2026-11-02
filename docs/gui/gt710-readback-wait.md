# Native readback wait diagnosis (25.1-u8)

This unit observes the existing backend-6 path. It changes no completion policy,
clock, scheduler, timeout, suite case or threshold. The 11:37 EST coordinator
ruling in the E25 ledger requests split timing after confirmation
`20261101T161810Z-metal` at `38954218`.

## Completion path

- `kernel/src/gpu/mod.rs:5489` (`read_back`) creates the optional native trace.
  The target-acquisition wait at `:5606` waits for `r.writers == 0` and
  `!r.reading`, then pins the resource for the observation. This is a wait for
  accepted render work/other readers, before either CE poison pass.
- `kernel/src/gpu/mod.rs:5923` (`observe`) poisons the backing, sends each CE
  transfer through `send_sync`, and waits through `await_sync_traced` at `:6075`.
  The two passes preserve the existing complementary-poison comparison.
- `kernel/src/gpu/mod.rs:2114` (`wait_for_traced`) calls `Core::poll` on each
  iteration. `Core::poll` at `:1285` reaps and refills native slots. Native
  `adapter.rs:261` (`Device::reap`) reads the channel's engine receipt at
  `:310` and accepts only `identity.complete(sample)` at `:319`; enqueueing or
  a caller fence alone is not completion.
- `kernel/src/gpu/mod.rs:99-109` supplies three different intervals:
  `SPIN_POLL_NS=1,000,000`, `POLL_NS=250,000`, `EXAMINE_NS=250,000,000`.
  The caller spins in 64-pause batches only while work is outstanding, no
  native continuation is queued for `kgpu`, and its 1 ms window remains.
  Otherwise it joins `WAITERS` through `wait_event` at `:2196`, with a 250 ms
  re-examination deadline (or the caller's earlier explicit deadline).
- `kernel/src/gpu/mod.rs:1850` (`next_deadline`) explicitly states there is
  no native completion interrupt here. While work is outstanding, `kgpu`
  requests a poll deadline 250 us later. Its loop at `:2569` pumps the queue,
  then `reaper_block` at `:2552` publishes `BlockedOnIO` with that deadline,
  calls `yield_current`, and calls `arch_halt_while_blocked`.
- `kernel/src/syscall/blocking_io.rs:16-69` (`wait_prepared`) checks signals,
  releases its preemption level, yields and parks while blocked. A completion
  taken by the caller or `kgpu` reaches `notify` (`gpu/mod.rs:205`), increments
  the event sequence and wakes queued waiters; `wake_reaper` at `:222` wakes
  the worker. No native IRQ supplies the completion notification.
- `kernel/src/task/scheduler.rs:10106` (`yield_current`) sets need-resched;
  it does not switch contexts by itself. `kernel/src/lib.rs:628-695`
  (`arch_halt_while_blocked`) can enter the x86 software reschedule gate for
  a migratable blocked thread, then uses `enable_and_hlt` if still blocked.
- `kernel/src/time/timer.rs:19-38,70-104` programs x86 LAPIC (or PIT fallback)
  at **200 Hz, 5 ms per tick**. `:114-122` advances the shared tick on timer
  entry using elapsed TSC periods. `kernel/src/interrupts/timer.rs:42-77`
  requests scheduling when a timer deadline is due or a CPU is idle;
  `TIME_QUANTUM=10` is a separate 50 ms quantum, not the requested 250 us poll.
  `kernel/src/task/scheduler.rs:89-107` claims due timers; expiration and wake
  occur in scheduler context (`:6685` and `kernel/src/task/scheduler/rq.rs:1017,1109`). Thus a
  250 us worker timer does not establish 250 us completion detection: timer
  delivery and dispatch can defer it to a 5 ms tick, plus scheduling delay.

No Tier-1 or Tier-2 file is edited. These references describe the source path,
not a measurement of engine execution time.

## Retained diagnostic schema

Intervals measure wall time and can include preemption; they do not measure CPU
utilization. The existing digest-bound `GT710_TIMING_V1` bulk record retains its ordinary
five-word rows. When native readback timing is enabled it also carries:

- `readbacks`: `[owner, draw, start_id_ns, total_ns, wait_ns, copy_ns,
  poll_iterations, spin_batches]`. The timestamp identifies one readback even
  when several readbacks share a draw. Wait time includes polling and scheduler
  delay inside display-restore, writer and CE-completion waits. Copy time covers
  CPU staging read/compare, channel canonicalization and publication to userspace.
  Poisoning, request construction, locks outside those intervals and metadata
  remain in `total - wait - copy`; that residual is not device time.
- `blocked`: `[readback_start_id, stage, reason, start_ns, elapsed_ns,
  tick_before, tick_after]`. Stages: 1 resource writers/readers, 2 CE completion,
  3 console restore. Reasons: 1 outstanding device work after the spin budget;
  2 no caller spin (queued continuation, stalled channel or no outstanding work).
  These label the condition at entry, not an invented IRQ or claimed wake source.
  The interval is the `wait_event` call including publication/cleanup, does not establish
  that the whole interval was spent asleep. Tick endpoints read the existing
  shared delivered-tick counter; no timer-handler instrument is added.

The ordinary-row bound grows from 16,384 to 32,768. The prior boot retained
16,384 and dropped 7,451, totaling 23,835; 32,768 adds 37% headroom. Readback and
blocked records each have an independent 8,192-row bound. Bound and allocation refusals still
increment `dropped`. The host check covers 29,794 ordinary rows (25% above the
prior count), 3,000 split records and 3,000 blocked records, with representative
40-second timestamps, below the 2 MiB slot minus 128 KiB for other final evidence.
Allocation failure preserves records for a later export. The boot's actual
`dropped` count and export length remain the acceptance of this case's retention.

Hardware results are retained in the E25 ledger under the diagnostic boot ID.

## Fence-driven native readback (25.1-u9)

The retained u8 split found 2,389 case readbacks: 6,040.968 ms waiting,
13.257 ms copying and 270.060 ms residual. 748 of 748 readbacks above 5 ms
entered blocking and crossed a delivered tick. Their return phases were spread,
so the capture does not measure GPU-complete-to-wake latency or establish a single
phase lock. It does expose the queued-continuation handoff described above:
`Core::poll` refills native slots, but a remaining backlog makes the caller
immediately hand the queue to the tick-driven worker.

Native resource-writer and CE readback waits now select fence-driven polling.
Each turn reaps real engine receipts and refills the bounded native queue, then
releases the core lock before the next turn. A queued continuation no longer
forces a worker handoff, and a readback receipt beyond the 1 ms general spin
window is still consumed directly. Signals, caller deadlines and the existing
watchdog run in the wait loop. A stalled device or a wait with no outstanding
native work still blocks for worker recovery. Console restoration and ordinary
fence waits retain the existing blocking policy; the suite's wait-CPU bound is
unchanged. The policy is independent of whether timing recording is enabled. Native
poll pauses now enable IRQs and release the syscall preemption hold outside
the core lock, restoring both before the next poll.
The native policy arguments and expressions compile only on x86; ARM execution
retains its original path.

Reference: MIT `nouveau_fence_wait_busy` / `nouveau_fence_wait` in
`nouveau/nouveau_fence.c` at Linux v6.8 commit
`e8f897f4afef0031fe618a8e94127a0934896aba`, whose file header was checked
before use. The consuming source retains its full notice and is listed in
`THIRD_PARTY_NOTICES`. Breenix supplies its own completion validation, signals
and watchdog; no Linux scheduler or DMA-fence implementation is imported.

The host reproduction executes the production wait against two queued engine
receipts at 80 us and 1,100 us per stage, at three phases of a 5 ms worker tick,
with timing recording both present and absent. Completion must follow the
receipts without any worker sleep. The old queued handoff slept twice even for
the 80 us receipts. Deadline, signal and virtual/recovery blocking controls
exercise the same function. This models the measured scheduling mechanism;
only the confirming card captures establish the case deadlines.
