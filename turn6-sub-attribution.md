# Turn 6: READY_SITE_SCHEDULE Sub-Attribution

Status: COMPLETE

## A. Counter Design

Turn 6 adds memory-only counters for inline rescues whose last ready publication was `READY_SITE_SCHEDULE`.

- `no_switch`: the thread hit the no-switch path after being published Ready.
- `inline_sched`: the last Ready publication came from the regular `schedule()` same-lock path.
- `exc_return`: the last Ready publication came from `schedule_deferred_requeue()` and had no more specific post-save outcome.
- `marker_cleared`: the thread had a deferred handoff marker published, but the marker was already gone when post-save requeue/cleanup observed it.
- `ownership_skip`: `requeue_thread_after_save()` skipped enqueue because the thread appeared current or deferred elsewhere.
- `stale_queue`: the thread was already queue-visible at publication or post-save requeue, then later became unreachable.
- `other`: fallback for any schedule-site inline rescue that does not fit a bucket.

The emit path prints:

```text
[rescue-detail] no_switch=N inline_sched=N exc_return=N marker_cleared=N ownership_skip=N stale_queue=N other=N
```

It also emits `[rescue-detail-mismatch]` if the detail sum diverges from the internal inline `READY_SITE_SCHEDULE` rescue count. No mismatch line appeared in the Turn 6 boot.

## B. Code Paths

Code commit: `5b263d90 feat(scheduler): sub-attribute READY_SITE_SCHEDULE dropped events`

All changes are in `kernel/src/task/scheduler.rs`.

- `record_schedule_ready_detail()` stores per-tid metadata when a thread is published Ready by `schedule()` or `schedule_deferred_requeue()`.
- `mark_schedule_ready_detail()` adds post-publication facts from scheduler-owned paths without changing behavior.
- The inline queue-empty rescue calls `classify_rescue_for_tid(stuck_tid, RESCUE_PATH_INLINE)`, which increments exactly one detail bucket for `READY_SITE_SCHEDULE`.
- The timer rescue still updates the existing broad counters, but does not feed the inline detail buckets.
- `requeue_thread_after_save()` marks `marker_cleared`, `ownership_skip`, and `stale_queue` causes based on the post-save outcome.
- `fix_exception_cleanup_cpu_state()` marks the prior thread if exception cleanup clears a handoff marker.

No rescue behavior changed. No timer, context-switch, exception, GIC, CPU0 guard, or idle-loop code was edited.

## C. Boot Data

Artifacts:

- Build log: `turn6-artifacts/sub-attribution/kernel-build.txt`
- Run log: `turn6-artifacts/sub-attribution/run.out`
- Serial log: `turn6-artifacts/sub-attribution/serial.log`
- VM name: `turn6-artifacts/sub-attribution/vm-name.txt`

Build was clean:

```text
Finished `release` profile [optimized] target(s) in 0.05s
```

Parallels VM: `breenix-1779217484`

Final health sample:

```text
[freeze-watch] uptime_ms=275449 submits=152921 completes=152924 fails=0 fps_last_5s=182
```

No `panic`, `DATA_ABORT`, `FAR=0xccd`, `soft lockup`, or `timer_interrupt.rs:598` markers were present. The only `CPU0` match in the panic scan was the normal AHCI IRQ routing line.

Counter evolution:

```text
dropped=0  inline=0  total=0   detail: marker_cleared=0  ownership_skip=0
dropped=4  inline=4  total=4   detail: marker_cleared=2  ownership_skip=2
dropped=5  inline=5  total=5   detail: marker_cleared=3  ownership_skip=2
dropped=9  inline=9  total=9   detail: marker_cleared=6  ownership_skip=3
dropped=11 inline=11 total=11  detail: marker_cleared=8  ownership_skip=3
dropped=13 inline=13 total=13  detail: marker_cleared=9  ownership_skip=4
dropped=14 inline=14 total=14  detail: marker_cleared=9  ownership_skip=5
dropped=16 inline=16 total=16  detail: marker_cleared=10 ownership_skip=6
dropped=18 inline=18 total=18  detail: marker_cleared=12 ownership_skip=6
dropped=21 inline=21 total=21  detail: marker_cleared=15 ownership_skip=6
```

Final snapshot:

```text
[rescue-attrib] dropped=21 isr_lost=0 wake_no_enq=0 other=0 inline=21 timer=0 total=21
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared=15 ownership_skip=6 stale_queue=0 other=0
```

The bucket sum is `15 + 6 = 21`, matching `dropped=21`, `inline=21`, and `total=21`.

## D. Distribution Interpretation

The residual race is not no-switch cleanup, regular `schedule()`, stale queue membership, or an unclassified path. Those buckets stayed at zero.

The signal is concentrated in two buckets:

- `marker_cleared`: 15 of 21 events, about 71%.
- `ownership_skip`: 6 of 21 events, about 29%.

Interpretation: the Turn 5 marker-before-Ready fix is pointing at the right invariant, but the handoff marker is still being cleared before the thread has a durable owner in some post-save paths. A smaller subset reaches `requeue_thread_after_save()` and then skips enqueue because the thread appears current or deferred elsewhere.

One caveat: `marker_cleared` means "the scheduler-visible `previous_thread` handoff was missing by the time scheduler-owned requeue/cleanup observed the thread." It includes explicit scheduler cleanup and caller-side pre-requeue clears. The bucket is still actionable because it identifies clear ordering as the dominant failure shape, but Turn 7 should preserve that distinction while fixing.

## E. Turn 7 Fix Proposal

Target the two dominant buckets, in this order:

1. Keep the handoff marker visible until `requeue_thread_after_save()` has either enqueued the thread, confirmed it is already queued, or confirmed another durable owner. Practically, this means delaying `previous_thread` clearing and restoring/preserving it on ownership-skip exits instead of clearing first and then discovering that enqueue cannot happen.
2. For `ownership_skip`, make the skip path prove the thread is still protected by a durable owner. If the only owner was the marker that just got cleared, preserve the marker or enqueue instead of leaving `Ready` unreachable.

Keep the fix narrow. Do not restore the raw `DEFERRED_REQUEUE` slot predicate. Do not widen the rescue path. If Claude keeps `context_switch.rs` off-limits for Turn 7, do the scheduler-owned clear ordering first and report whether caller-side pre-clears remain as a blocker.

## F. Status

COMPLETE.

The 7-bucket sum matched the inline `READY_SITE_SCHEDULE` rescue count, `other=0`, the distribution is dominated by `marker_cleared` and `ownership_skip`, and the 275s Parallels run stayed healthy.
