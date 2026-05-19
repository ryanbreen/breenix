# Turn 10 Stress Gate and Cleanup

## A. 5-boot gate results

Turn 10 first re-ran the Turn 9 code (`3850fef9` fix plus `5748ae8c` docs) through five fresh epoch-named Parallels boots before deleting anything.

Artifacts:

```text
turn10-artifacts/stress-gate/aggregate-result.txt
turn10-artifacts/stress-gate/boot-1/
turn10-artifacts/stress-gate/boot-2/
turn10-artifacts/stress-gate/boot-3/
turn10-artifacts/stress-gate/boot-4/
turn10-artifacts/stress-gate/boot-5/
```

Aggregate:

| Boot | Result | Rescue Max | Final Uptime | Final FPS |
|---|---:|---:|---:|---:|
| 1 | pass | 0 | 285488ms | 149 |
| 2 | pass | 0 | 285513ms | 151 |
| 3 | pass | 0 | 285529ms | 144 |
| 4 | pass | 0 | 285525ms | 132 |
| 5 | pass | 0 | 290527ms | 141 |

Overall: pass.

Each boot passed the strict gate:

- `[rescue-attrib] total=0` at every emitted snapshot.
- All `[rescue-detail]` buckets were 0 at every emitted snapshot.
- No `[rescue-detail-mismatch]` lines.
- No `queue_empty rescue` / `SCHED_RESCUE` serial markers.
- No `panic`, `PC_ALIGN`, soft-lockup, `DATA_ABORT`, `FAR=0xccd`, or `timer_interrupt.rs:598` markers.
- BWM rendering stayed active beyond 220s with final FPS above 60.
- Scoped `breenix-*` VM cleanup completed after each boot.

## B. Deletion diff

Rescue infrastructure deletion:

```text
cea8ef6a feat(scheduler): delete rescue infrastructure (race closed in 3850fef9)

kernel/src/arch_impl/aarch64/timer_interrupt.rs |  18 --
kernel/src/drivers/virtio/gpu_pci.rs            |   5 +-
kernel/src/task/scheduler.rs                    | 264 +-----------------------
3 files changed, 4 insertions(+), 283 deletions(-)
```

Removed from live code:

- Inline queue-empty rescue in `schedule_deferred_requeue()`.
- Timer rescue implementation and wrapper.
- CPU0 timer safety-net call to the rescue wrapper.
- `READY_THREAD_RESCUE_COUNT` and `ready_thread_rescue_count()`.
- `RESCUE_PATH_*`, `RESCUE_REASON_*`, `RESCUE_INLINE_COUNT`, `RESCUE_TIMER_COUNT`.
- `[rescue-attrib]` emission and `rescues=` freeze-watch field.

Sub-attribution deletion:

```text
717d44d8 feat(scheduler): delete sub-attribution counters (no longer needed)

kernel/src/task/scheduler.rs | 200 -------------------------------------------
1 file changed, 200 deletions(-)
```

Removed from live code:

- `WAKE_SCHEDULE_READY_DETAIL` per-thread table.
- `WAKE_MARKER_CLEAR_SOURCE` per-thread table.
- `READY_SCHED_DETAIL_*` flags.
- `MARKER_CLEAR_SOURCE_*` constants.
- `READY_SITE_SCHEDULE_DROPPED_*` buckets and inline total.
- Schedule-detail recording helpers and call sites.
- `[rescue-detail]` and `[rescue-detail-mismatch]` emissions.

Kept intentionally:

- General `WAKE_SITE_*` counters.
- General `WAKE_LAST_READY_SITE` table.
- General enqueue counters.
- Turn 5 deferred `previous_thread` handoff.
- Turn 9 `resolve_exception_cleanup_previous_thread()` fix.

Validation after deletion:

```text
rg -n "READY_THREAD_RESCUE|ready_thread_rescue_count|rescue_stuck_ready_threads|RESCUE_|rescue-attrib|rescue-detail|rescue-detail-mismatch|SCHED_RESCUE|queue_empty rescue|rescue_tid|READY_SITE_SCHEDULE_DROPPED|WAKE_SCHEDULE_READY_DETAIL|WAKE_MARKER_CLEAR_SOURCE|MARKER_CLEAR_SOURCE|READY_SCHED_DETAIL|record_schedule_ready_detail|mark_schedule_ready_detail" kernel/src
```

The only match was a historical AHCI comment saying the normal sleep path had eliminated old `SCHED_RESCUE` reports; no rescue symbol, caller, counter, or runtime marker remains.

Build verification:

```text
turn10-artifacts/post-deletion/kernel-build.txt
```

Result:

```text
Compiling kernel v0.1.0 (/Users/wrb/fun/code/breenix.worktrees/scheduler-wake-atomic/kernel)
Finished `release` profile [optimized] target(s) in 4.78s
```

Warning/error scan returned no output.

## C. Post-deletion verification boot

Artifact directory:

```text
turn10-artifacts/post-deletion/
```

VM:

```text
breenix-1779223828
```

Result:

```text
post-deletion: pass final_uptime=285459ms final_fps=187
final submits=158846 completes=158849 last_completion_ms=285458 timer_ticks_cpu0=188734
```

Selected tail evidence:

```text
[freeze-watch] uptime_ms=220418 submits=122996 completes=122999 fails=0 last_completion_ms=220417 fps_last_5s=182 ... timer_ticks_cpu0=145066 timer_ticks_cpu1=179140 ...
[freeze-watch] uptime_ms=245434 submits=136862 completes=136865 fails=0 last_completion_ms=245434 fps_last_5s=186 ... timer_ticks_cpu0=161877 timer_ticks_cpu1=199582 ...
[freeze-watch] uptime_ms=275452 submits=153221 completes=153224 fails=0 last_completion_ms=275452 fps_last_5s=181 ... timer_ticks_cpu0=181984 timer_ticks_cpu1=224121 ...
[freeze-watch] uptime_ms=285459 submits=158846 completes=158849 fails=0 last_completion_ms=285458 fps_last_5s=187 ... timer_ticks_cpu0=188734 timer_ticks_cpu1=232303 ...
```

Runtime marker scan returned no output for:

```text
rescue-attrib|rescue-detail|rescue-detail-mismatch|SCHED_RESCUE|queue_empty rescue|rescue_tid=
PC_ALIGN|panic|soft lockup|SOFT LOCKUP|DATA_ABORT|Data Abort|FAR=0xccd|timer_interrupt.rs:598
```

## D. PR URL

https://github.com/ryanbreen/breenix/pull/344

## E. Status

COMPLETE.

The five pre-deletion stress boots proved Turn 9 held under repeated sustained virtio-gpu load. The rescue infrastructure and temporary sub-attribution diagnostics were then deleted, the kernel built with zero warnings, and the post-deletion Parallels boot stayed healthy for 285s with deleted rescue markers absent.
