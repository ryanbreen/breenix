# Turn 9 Exception-Cleanup Marker-Clear Fix

## A. Race walkthrough

Turn 8 localized the remaining rescue events to the exception-cleanup marker-clear path:

```text
[rescue-attrib] dropped=30 isr_lost=0 wake_no_enq=0 other=0 inline=30 timer=0 total=30
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=25 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=5 ownership_skip_other_deferred=0 stale_queue=0 other=0
```

The race is:

1. The scheduler publishes `cpu_state[cpu].previous_thread = T` during a context-switch save window.
2. `T` is made `Ready`, but its normal post-save enqueue has not yet made it runqueue-reachable.
3. The AArch64 exception cleanup path redirects the CPU to idle and clears `previous_thread` unconditionally.
4. The normal post-save tail no longer sees the marker, so it cannot enqueue `T`.
5. `T` remains `Ready` but absent from every ready queue until the rescue detector finds it.

That matches the Turn 8 distribution: most remaining rescues came from `marker_cleared_exception_cleanup`, while the `ownership_skip_current` bucket was a separate protection path that Turn 7 showed must not be force-enqueued.

## B. Fix shape

The fix replaces the unconditional exception-cleanup clear with `resolve_exception_cleanup_previous_thread(cpu)` in `kernel/src/task/scheduler.rs`.

For the local `previous_thread` marker, the helper checks whether the thread is:

- `Ready`
- not an idle thread
- not already on any per-CPU ready queue
- not current on any CPU
- not owned by another CPU's `previous_thread` marker

If all predicates hold, the helper enqueues the thread on the current CPU's ready queue, increments the existing deferred-drain success counter, sets `need_resched`, and then clears the local marker. If the thread is already queued, current, idle, owned by another deferred marker, or no longer `Ready`, the helper only clears the local exception-cleanup marker and preserves the existing ownership behavior. There is no enqueue-on-current change and no rescue-path edit.

The helper is used from both scheduler-owned exception cleanup call sites:

- `Scheduler::fix_exception_cleanup_cpu_state()`
- `switch_to_idle_best_effort()`

## C. Diff

Code commit:

```text
3850fef93994db2987f22bdd57f6d6180980830d fix(scheduler): close exception-cleanup marker-clear race
```

Diff stat:

```text
kernel/src/task/scheduler.rs | 48 ++++++++++++++++++++++++++++++++------------
1 file changed, 35 insertions(+), 13 deletions(-)
```

No gold-master files, rescue-path logic, sub-attribution counters, raw `DEFERRED_REQUEUE` slot predicates, or `ownership_skip_current` behavior were modified.

## D. Boot evidence

Build artifact:

```text
turn9-artifacts/exception-cleanup-fix/kernel-build.txt
```

Build completed cleanly:

```text
Compiling kernel ...
Finished `release` profile [optimized] target(s) in 4.54s
```

Warning/error scan:

```text
rg -n "^(warning|error)" turn9-artifacts/exception-cleanup-fix/kernel-build.txt
# no output
```

Parallels VM:

```text
breenix-1779220776
```

Runtime artifacts:

```text
turn9-artifacts/exception-cleanup-fix/run.out
turn9-artifacts/exception-cleanup-fix/serial.log
turn9-artifacts/exception-cleanup-fix/vm-name.txt
```

Every rescue snapshot stayed zero:

```text
428:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
429:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
538:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
539:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
624:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
625:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
710:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
711:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
797:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
798:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
883:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
884:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
969:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
970:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
1055:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
1056:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
1141:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
1142:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
1227:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
1228:[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared_exception_cleanup=0 marker_cleared_caller_pre=0 marker_cleared_missing_at_entry=0 ownership_skip_current=0 ownership_skip_other_deferred=0 stale_queue=0 other=0
```

Freeze-watch health after the 220s target:

```text
1069:[freeze-watch] uptime_ms=220429 submits=122099 completes=122102 fails=0 last_completion_ms=220425 fps_last_5s=187 ... timer_ticks_cpu0=144751 timer_ticks_cpu1=186942 timer_ticks_cpu2=185815 timer_ticks_cpu3=183840 ...
1083:[freeze-watch] uptime_ms=225432 submits=124841 completes=124844 fails=0 last_completion_ms=225432 fps_last_5s=182 ... timer_ticks_cpu0=148038 timer_ticks_cpu1=191206 timer_ticks_cpu2=190056 timer_ticks_cpu3=188037 ...
1223:[freeze-watch] uptime_ms=275459 submits=152735 completes=152737 fails=0 last_completion_ms=275458 fps_last_5s=188 ... timer_ticks_cpu0=181122 timer_ticks_cpu1=233929 timer_ticks_cpu2=232489 timer_ticks_cpu3=230026 ...
1255:[freeze-watch] uptime_ms=285465 submits=158236 completes=158239 fails=0 last_completion_ms=285465 fps_last_5s=186 ... timer_ticks_cpu0=187746 timer_ticks_cpu1=242469 timer_ticks_cpu2=240963 timer_ticks_cpu3=238438 ...
```

Alarm scan:

```text
rg -n "rescue-detail-mismatch|PC_ALIGN|panic|soft lockup|DATA_ABORT|Data Abort|FAR=0xccd|timer_interrupt.rs:598" \
  turn9-artifacts/exception-cleanup-fix/serial.log \
  turn9-artifacts/exception-cleanup-fix/run.out
# no output
```

Honesty checks:

- Runtime exceeded the 220s active-rendering target, reaching `uptime_ms=285465`.
- `submits` and `completes` advanced through the full window: final `submits=158236`, `completes=158239`.
- `fps_last_5s` stayed far above 60 in the sampled tail: final `fps_last_5s=186`.
- Final `last_completion_ms=285465`, equal to final `uptime_ms=285465`.
- CPU0 timer ticks advanced throughout the run: `144751` at 220s to `187746` at 285s.
- No PC_ALIGN, panic, soft-lock, DATA_ABORT, FAR=0xccd, or CPU0 timer regression markers were found.

## E. Honesty check

`marker_cleared_exception_cleanup` went to 0 for the single 285s Parallels boot. `ownership_skip_current` also read 0 in this boot, below the Turn 8 baseline of 5 per about 5 minutes; that is positive evidence but not enough to claim the residual ownership window is gone. A 5-boot gate is still needed before deleting the rescue path or deciding whether the ownership bucket is a real design path.

No bucket shifted to another sub-attribution category, and the sum check stayed trivially valid because both `[rescue-attrib] total` and all `[rescue-detail] buckets were 0 at every snapshot.

## F. Status

COMPLETE for Turn 9.

Recommended Turn 10 scope: run the planned 5-boot Parallels stress gate against this exact code. If all boots stay zero-rescue, proceed to delete the rescue mechanism and then remove the temporary sub-attribution counters in separate code/docs commits. If `ownership_skip_current` returns while the system stays healthy, decide whether to keep a documented ownership-window handler temporarily or redesign the wake path around a Linux-style `TASK_WAKING` intermediate state. If any other bucket fires, treat that as an INCONCLUSIVE follow-up and localize the new bucket before touching cleanup.
