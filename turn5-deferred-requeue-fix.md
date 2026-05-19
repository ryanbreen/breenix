# Turn 5: Deferred-Requeue Handoff Fix

Status: INCONCLUSIVE

## A. Race Analysis

Turn 4 localized the remaining rescues to `READY_SITE_SCHEDULE` with `dropped + inline`. The vulnerable protocol was in `Scheduler::schedule_deferred_requeue()`: the outgoing current thread was marked `Ready` and attributed to `WAKE_SITE_SCHEDULE` before the AArch64 deferred-requeue handoff had an owner visible to other wakeup paths.

Original shape:

```rust
current.set_ready();
WAKE_SITE_SCHEDULE.fetch_add(1, Ordering::Relaxed);
record_ready_site(current_id, READY_SITE_SCHEDULE);
should_requeue_old = !is_terminated && !is_blocked && !in_queue;
```

That violated the Linux-style invariant that a runnable task must be queued, current, or covered by an unloseable handoff marker. In this kernel, the AArch64 context-switch tail cannot enqueue the outgoing thread until after the old context is saved, so the handoff marker has to exist before the thread becomes externally visible as `Ready`.

There was a second same-CPU handoff issue in `requeue_thread_after_save()`: `is_in_deferred_requeue()` checked all CPUs' `previous_thread` markers, including the marker for the CPU currently completing the save. If the same-CPU marker was set, the post-save requeue path could treat its own marker as a reason to return instead of consuming it and enqueueing the saved thread.

## B. Fix Design

The committed fix is deliberately narrow:

- In `schedule_deferred_requeue()`, compute whether the current thread will need deferred requeue before publishing `Ready`.
- If deferred requeue is required, set `cpu_state[current_cpu].previous_thread = Some(current_id)` before `current.set_ready()`.
- In the no-switch path where the same thread remains running, clear the marker and restore `Running`.
- In `requeue_thread_after_save()`, clear the same-CPU marker before checking current threads and other deferred markers.

I did not modify the rescue path. The rescue remains a regression detector.

I also rejected a second attempted fix that made `is_in_deferred_requeue()` inspect the raw AArch64 `DEFERRED_REQUEUE` atomic slots. That hid a real orphan condition instead of repairing ownership: the kernel stopped making BWM progress almost immediately with `submits=62`, `completes=65`, `fps_last_5s=0`, CPU0 timer ticks stuck at 74, and later timer watchdog panics. That patch was reverted before the final code commit.

## C. Diff

Code commit: `da923b42 fix(scheduler): close deferred-requeue race in schedule_deferred_requeue`

Only `kernel/src/task/scheduler.rs` is changed in the final code commit.

Key code changes:

- `schedule_deferred_requeue()` now publishes the same-CPU `previous_thread` marker before setting the outgoing thread `Ready`.
- The no-switch branch clears that marker if the current thread continues running.
- `requeue_thread_after_save()` consumes the same-CPU marker before applying the existing "currently running" and "still deferred elsewhere" guards.

## D. Single-Boot Evidence

Build:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
Finished `release` profile [optimized] target(s) in 4.98s
```

Final first-fix Parallels evidence:

- VM: `breenix-1779216023`
- Serial log: `turn5-artifacts/deferred-requeue-fix/serial-first-fix.log`
- BWM remained healthy through the 220s window and beyond.
- No `soft lockup`, `DATA_ABORT`, `FAR=0xccd`, or kernel panic markers in the first-fix serial log.
- Final freeze-watch sample: `uptime_ms=270520 submits=108932 completes=108934 fails=0 fps_last_5s=118`.
- Rescue attribution improved but did not clear:

```text
[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
[rescue-attrib] dropped=3 isr_lost=0 wake_no_enq=0 other=0 inline=3 timer=0 total=3
[rescue-attrib] dropped=6 isr_lost=0 wake_no_enq=0 other=0 inline=6 timer=0 total=6
[rescue-attrib] dropped=10 isr_lost=0 wake_no_enq=0 other=0 inline=10 timer=0 total=10
[rescue-attrib] dropped=10 isr_lost=0 wake_no_enq=0 other=0 inline=10 timer=0 total=10
```

Rejected second-patch evidence:

- Serial log: `turn5-artifacts/deferred-requeue-fix/serial-second-fix-regression.log`
- Early freeze-watch remained stuck at `submits=62 completes=65 fps_last_5s=0`.
- CPU0 timer ticks stayed at 74 while other CPUs advanced.
- Timer watchdog panicked at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598`.
- This confirmed the raw atomic-slot predicate masked the orphan instead of fixing the ownership race.

## E. Honesty Check

The Turn 5 success threshold was `dropped=0 inline=0 total=0` or residual `total < 5` and flat. The committed fix reduced the Turn 4 final rescue count from 28 to 10 and the count stayed flat after about 125s, but `total=10` is still above the threshold. This is not complete.

## F. Turn 6 Scope

Recommended Turn 6 scope: add a small memory-only sub-attribution around the remaining `READY_SITE_SCHEDULE` dropped events, focused on the `schedule_deferred_requeue()` to post-save handoff. The likely distinctions to capture:

- Ready published but no switch happened and the no-switch cleanup did not fully account for the marker.
- Inline scheduling path versus exception-return deferred slot path.
- `previous_thread` cleared by exception cleanup before `requeue_thread_after_save()` can enqueue.
- Requeue skipped because the thread appears current or deferred elsewhere.
- Queue membership became stale between publication and post-save requeue.

Keep the first fix. It is healthy and measurably reduces rescues. Do not restore the raw `DEFERRED_REQUEUE` slot predicate; it regressed rendering and made the real orphan harder to see.
