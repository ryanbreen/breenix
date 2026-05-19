# Turn 8: Split Sub-Attribution

Status: COMPLETE

## A. Counter Design

Turn 8 replaces the two coarse Turn 6 buckets, `marker_cleared` and `ownership_skip`, with five finer memory-only buckets:

- `marker_cleared_exception_cleanup`: scheduler-owned exception cleanup cleared a `previous_thread` marker for a schedule-published Ready thread.
- `marker_cleared_caller_pre`: scheduler-owned no-switch/caller-side pre-clear cleared the marker before post-save requeue evidence could consume it.
- `marker_cleared_missing_at_entry`: `requeue_thread_after_save()` observed the marker already missing, with no scheduler-owned clear source recorded.
- `ownership_skip_current`: `requeue_thread_after_save()` skipped because the thread still appeared current on a CPU.
- `ownership_skip_other_deferred`: `requeue_thread_after_save()` skipped because another CPU still held the thread in `previous_thread`.

The unchanged buckets are `no_switch`, `inline_sched`, `exc_return`, `stale_queue`, and `other`, for 10 buckets total.

Code commit:

```text
8aef35c4 feat(scheduler): split marker_cleared and ownership_skip into sub-buckets
 kernel/src/task/scheduler.rs | 103 ++++++++++++++++++++++++++++++++++---------
```

The implementation keeps the existing per-thread schedule-detail bitset and adds `WAKE_MARKER_CLEAR_SOURCE`, a per-thread atomic source table. The source table is reset when a new Ready site/detail is recorded and is written only at scheduler-owned marker clear sites. Gold-master context-switch pre-clears are not touched; if they dominate later, they fall into `marker_cleared_missing_at_entry`.

## B. Code Paths

Instrumentation only; no ready-queue behavior changed.

- `record_schedule_ready_detail()` now resets per-thread marker-clear source.
- `record_ready_site()` clears schedule detail and marker-clear source when the Ready site is not `READY_SITE_SCHEDULE`.
- `classify_schedule_dropped_detail()` now increments exactly one of the 10 buckets.
- `emit_wake_attribution_counters()` prints the 10-bucket line and still emits `[rescue-detail-mismatch]` if the bucket sum diverges from the inline schedule-site dropped total.
- `requeue_thread_after_save()` marks `ownership_skip_current` when any CPU still has the thread as current, and `ownership_skip_other_deferred` when another `previous_thread` marker still owns it.
- `fix_exception_cleanup_cpu_state()` and `switch_to_idle_best_effort()` record `marker_cleared_exception_cleanup` before clearing scheduler-owned `previous_thread`.
- The no-switch same-thread branch records caller-pre source before clearing, but `no_switch` remains the higher-priority bucket if that path ever rescues later.

## C. Boot Data

Artifacts:

- Build log: `turn8-artifacts/split-attribution/kernel-build.txt`
- Run log: `turn8-artifacts/split-attribution/run.out`
- Serial log: `turn8-artifacts/split-attribution/serial.log`
- VM name: `turn8-artifacts/split-attribution/vm-name.txt`

Build was clean:

```text
Finished `release` profile [optimized] target(s) in 4.67s
```

Parallels VM: `breenix-1779220006`

Final health sample:

```text
[freeze-watch] uptime_ms=285469 submits=156419 completes=156422 fails=0 last_completion_ms=285469 fps_last_5s=184
```

Final attribution sample:

```text
[rescue-attrib] dropped=30 isr_lost=0 wake_no_enq=0 other=0 inline=30 timer=0 total=30
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=25 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=5 ownership_skip_other_deferred=0 stale_queue=0 other=0
```

No `PC_ALIGN`, `panic`, `soft lockup`, `DATA_ABORT`, `FAR=0xccd`, `timer_interrupt.rs:598`, or `[rescue-detail-mismatch]` lines appeared.

Counter evolution:

```text
dropped=0  inline=0  total=0   exception_cleanup=0  ownership_current=0
dropped=2  inline=2  total=2   exception_cleanup=2  ownership_current=0
dropped=4  inline=4  total=4   exception_cleanup=3  ownership_current=1
dropped=8  inline=8  total=8   exception_cleanup=6  ownership_current=2
dropped=10 inline=10 total=10  exception_cleanup=8  ownership_current=2
dropped=16 inline=16 total=16  exception_cleanup=14 ownership_current=2
dropped=19 inline=19 total=19  exception_cleanup=17 ownership_current=2
dropped=23 inline=23 total=23  exception_cleanup=20 ownership_current=3
dropped=26 inline=26 total=26  exception_cleanup=23 ownership_current=3
dropped=30 inline=30 total=30  exception_cleanup=25 ownership_current=5
```

At every snapshot the split bucket sum matched `dropped`, `inline`, and `total`.

## D. Distribution Interpretation

The distribution is clean:

- `marker_cleared_exception_cleanup`: 25/30, about 83%.
- `ownership_skip_current`: 5/30, about 17%.
- `marker_cleared_caller_pre`: 0.
- `marker_cleared_missing_at_entry`: 0.
- `ownership_skip_other_deferred`: 0.
- Previously-zero buckets also stayed zero: `no_switch`, `inline_sched`, `exc_return`, `stale_queue`, `other`.

The dominant race is scheduler-owned exception cleanup clearing the `previous_thread` marker while the outgoing thread is still a schedule-published Ready thread that later becomes unreachable. This is a true marker-lifecycle loss, not a caller-side pre-clear and not an unknown gold-master pre-clear.

The secondary `ownership_skip_current` bucket must be left alone as a protection case. Turn 7 proved that force-enqueueing a thread that still appears current can corrupt return context (`PC_ALIGN` at `ELR=0x100000001`).

## E. Turn 9 Fix Proposal

Target only `marker_cleared_exception_cleanup`.

Narrow shape for Turn 9:

1. In scheduler-owned exception cleanup, do not simply clear a `previous_thread` marker for a schedule-published Ready thread.
2. If the previous thread is Ready, not idle, not queued, not current on any CPU, and not owned by another deferred marker, enqueue it or preserve enough marker ownership for the normal post-save path.
3. If the previous thread still appears current, do not enqueue it. That is the protected `ownership_skip_current` class.
4. Do not touch `ownership_skip_current`, raw `DEFERRED_REQUEUE` predicates, rescue behavior, or gold-master context-switch/exception files.

Buckets to leave alone unless future evidence changes:

- `ownership_skip_current`
- `marker_cleared_caller_pre`
- `marker_cleared_missing_at_entry`
- `ownership_skip_other_deferred`

The latter three were zero in this run, so they are not the Turn 9 target.

## F. Status

COMPLETE.

The 10-bucket sum-check passed, the boot stayed healthy through 285s, the distribution is dominated by `marker_cleared_exception_cleanup`, and the unsafe protection case is clearly isolated as `ownership_skip_current`.
