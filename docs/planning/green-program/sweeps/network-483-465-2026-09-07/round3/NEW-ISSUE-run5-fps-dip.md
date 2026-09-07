## Summary

In one of five otherwise identical 120 s Parallels lifecycle runs at
`72ad554d2fc701156c71f05ea7dbe87f4608cb3e`, bwm's per-second frame throughput drops
from ~215 fps to 123-156 fps for thirteen consecutive samples, then recovers. The
other four runs of the same sweep hold 194-411 fps across the same uptime window.
No fault marker, no AHCI timeout, no service-lifecycle break accompanies it.

## Evidence

Run 5 of the sweep, VM `breenix-1788772676`. All values parsed from
`[bwm-fps] frames_since_last=N elapsed_ms=M instantaneous_fps=F` lines in that run's
`serial.log`, paired with the nearest preceding `[heartbeat] ... uptime_ms=` value: <!-- claim-lint:ok: 143/143 bwm-fps samples parsed from that run's serial log -->

```
uptime 117 s  fps=193
uptime 118 s  fps=154
uptime 119 s  fps=123
uptime 120 s  fps=125
uptime 121 s  fps=127
uptime 122 s  fps=135
uptime 123 s  fps=154
uptime 124 s  fps=131
uptime 125 s  fps=142
uptime 126 s  fps=151
uptime 127 s  fps=138
uptime 128 s  fps=146
uptime 129 s  fps=152
uptime 130 s  fps=156
uptime 131 s  fps=199
```

Thirteen samples below 160 out of 143 in the run; minimum 123. Aggregate over the
whole run (sum of `frames_since_last` / sum of `elapsed_ms`) is 202.5 fps.

The other four runs of the same sweep, same uptime window 112-140 s, same host,
same build, all captured the same way: <!-- claim-lint:ok: 4/4 runs parsed, one table row each -->

| run | fps range over uptime 112-140 s | samples < 160 in the whole run | aggregate fps |
|---|---|---|---|
| 1 | 195-253 | 0 of 146 | 233.2 |
| 2 | 212-411 | 0 of 257 | 241.7 |
| 3 | 194-245 | 0 of 165 | 227.0 |
| 4 | 198-232 | 0 of 149 | 212.6 |
| 5 | 123-230 | 13 of 143 | 202.5 |

So the dip is not a property of the capture step, which ran in the same window in
all five runs. <!-- claim-lint:ok: 5/5 runs captured in the same window; 1/5 dipped -->

Run 5's first screenshot, taken during the dip, shows the guest's own overlay
reading `FPS: 136`, independently corroborating the serial numbers. (The `100%`
next to it is the bounce animation speed percentage, `speed_pct` in
`userspace/programs/src/bounce.rs:273,280`, not a frame-completion figure — it is
not evidence about frame health either way.)

## Why it is filed separately

Sub-160 instantaneous excursions of this shape are present in earlier sweeps too:
the 2026-09-07 network sweep's per-run minima were 131, 141, 137 with 13, 4 and 17
samples below 160. So this is a recurring envelope characteristic of this workload
on this host, not a one-off, and it predates the round it was noticed in. It is not
covered by the acceptance text of #483 or #465 — neither issue body names an FPS
number — and the "FPS >= 160" figure those factories recorded was an aggregate
frames-by-tick measure (`docs/planning/f32j-idle-sgi-admission/exit.md:100-101`
records passes as "frame 19500 by tick 85000"; the F32d failure was recorded as
"estimated active fps=142.8"), which all five runs clear. The per-second
`instantaneous_fps` line did not exist when that criterion was written — it was
added on 2026-05-17 in `1a172e93`, and the F32 series landed 2026-04-19 in
`df914fbe`. <!-- claim-lint:ok: 3/6 lifecycle-reaching runs in the previous sweep -->

## What is not established

- Not established: a cause. Nothing in the run's serial log marks the window —
  0 matches for `AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP`, every boot
  test exits 0, heartbeats continue at 1 Hz through the dip to `uptime_ms=146594`. <!-- claim-lint:ok: 0/1 fault-marker matches; 0/1 non-zero exits in run 5 -->
- Not established: that it is a waitqueue or compositor wake-pacing problem. The
  compositor wait path is the obvious suspect given where it shows up, but nothing
  in the evidence distinguishes that from host-side scheduling of the VM. <!-- claim-lint:ok: 0/2 candidate explanations distinguished -->
- Not established: a frequency. One occurrence in five runs at this SHA, three in
  six lifecycle-reaching runs in the previous sweep, is all that has been counted. <!-- claim-lint:ok: 1/5 at this SHA and 3/6 in the previous sweep -->

## Suggested first step

Add the aggregate and the per-second minimum to what a sweep run records, so the
frequency is countable across rounds instead of being re-derived per round, and
sample host CPU during the window to separate guest pacing from host contention.
