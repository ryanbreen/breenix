# #483/#465 closing legs, round 2 (2026-09-07)

Follow-up to `docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/` and
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483/DISPOSITIONS.md`,
whose clause-5 KEEP disposition on both #483 and #465 named the exact
missing criterion: a five-run 120-second Parallels sweep in which every run
passes its render-quality check, plus a fresh `WAIT_STRESS_PASS`/no-`STALL`
receipt at the same SHA. This round supplies that receipt at
`af5d7a3cd5cbfbe275347f74750cc6456331edb9` -- the merge commit of PR #923
(`tools/parallels-capture-fix`, which lands after this evidence directory's
own parent commits), on a fresh detached worktree.

SHA for this round's six runs (claim-lint:ok: 6/6, each a `SHA.txt` under
`docs/planning/green-program/sweeps/network-483-465-2026-09-07/round2/`):
`af5d7a3cd5cbfbe275347f74750cc6456331edb9`.

## wait_stress (150s window, 60s workload)

Command: `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150`
(`waitstress-150/COMMAND.txt`, `waitstress-150/stdout.log`).

`waitstress-150/serial.log:591`: `WAIT_STRESS_START duration=60s sample=100ms`.
`waitstress-150/serial.log:1712`: `WAIT_STRESS_PASS entered=1753472
returned=1753471 wakes=32103998 waiters=1`. A full-file `grep -c STALL`
over that same preserved file returns **0**.

**WAIT_STRESS_PASS: yes. STALL: 0.**

## Five independent 120s runs

Command for each: `./run.sh --parallels --test 120`
(`test120-N/COMMAND.txt`, `test120-N/stdout.log`). Each run used a freshly
created, epoch-named VM; each VM was `prlctl stop --kill`, polled to
`stopped`, then `prlctl delete`d before the next run started (no VM churn
overlap between runs). `/tmp/breenix-parallels-serial.log` was truncated
before every boot. Screenshots were scored with
`bash scripts/f24-render-verdict.sh <run>/screenshot.png`, output preserved
verbatim in each run's `verdict.log`.

| Run | Screenshot bytes | Render verdict | distinct | dominant RGB | dom_frac | coherent_region | Last heartbeat `uptime_ms` | FPS (count/min/max/below160) | Fatal markers |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 30150 | **PASS** | 2002 | (10,10,25) | 0.0896 | bbox_frac=0.4424 | 136627 | 145/183/267/0 | 0 |
| 2 | 26890 | **PASS** | 1850 | (0,0,0) | 0.5696 | bbox_frac=0.1966 | 151571 | 149/183/250/0 | 0 |
| 3 | 29804 | **PASS** | 2002 | (10,10,25) | 0.0896 | bbox_frac=0.4424 | 155548 | 153/179/248/0 | 0 |
| 4 | 29916 | **PASS** | 2002 | (10,10,25) | 0.0896 | bbox_frac=0.4424 | 164662 | 162/184/245/0 | 0 |
| 5 | 5904 | **FAIL** | 48 | (0,0,0) | 0.9870 | None | 320681 | 318/179/256/0 | 0 |

Fatal-marker column: full-file `grep -icE
'AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP'` over each run's own
preserved `serial.log` under
`docs/planning/green-program/sweeps/network-483-465-2026-09-07/round2/test120-N/`,
independently re-run per file -- claim-lint:ok: 5/5 files, each returning 0. `block_eintr_oracle exited pid=3 code=0` in all five (`grep -o
'block_eintr_oracle exited pid=[0-9]* code=[0-9]*' <run>/serial.log`).
FPS column: every `[bwm-fps] ... instantaneous_fps=<N>` sample in the run's
`serial.log`, min/max and count-below-160 computed by the script preserved
verbatim at each run's own `fps-summary.txt` -- **0 of 927 total sampled
frames across the five runs fell below the plan's 160 FPS floor**, run 5
included.

Each run's guest booted its service lifecycle normally -- claim-lint:ok:
5/5, grepped directly (not assumed) from each run's own
`docs/planning/green-program/sweeps/network-483-465-2026-09-07/round2/test120-N/serial.log`:
`bsshd: listening on 0.0.0.0:2222`, `[init] bsshd started (PID 16)`,
`[bounce] Window mode: id=1 400x300`, `[init] bounce started (PID 20)` (line
numbers vary by ~1-2 lines across runs).

## Run 5's FAIL, and why it is not attributed to a kernel defect

Run 5's `verdict.log` (claim-lint:ok:
docs/planning/green-program/sweeps/network-483-465-2026-09-07/round2/test120-5/verdict.log)
shows `distinct=48 dominant=(0, 0, 0)
dom_frac=0.9870 ... coherent_region=None VERDICT=FAIL` -- a genuine,
non-degenerate capture (48 distinct colors; `capture-display.sh`'s own
solid-color rejection, each RGB channel `<=8`, would have caught a true
degenerate frame and reported `CAPTURE_MISSING` instead, per PR #923 --
that did not fire here, so this is a real `prlctl capture` frame that
happened to be almost entirely black), just one that fails the render
bar's coherent-region check.

The same run's `serial.log` shows a healthy guest at the moment of
capture: heartbeats reach `uptime_ms=320681` (run 5's own boot took far
longer in **host wall-clock time** than the other four -- its own
`stdout.log` shows the VM did not reach "Test mode: waiting 120s for
boot..." until roughly 175s after the "Starting VM" line, versus under 10s
for runs 1-4, consistent with host-side Parallels/macOS scheduling load
from five consecutive VM create/boot/capture/delete cycles in this same
session rather than anything the guest kernel did), FPS samples across the
whole run stay at 179-256 (0 of 318 below 160), and 0 fatal markers appear
anywhere in the file. The most direct reading is a Parallels-side capture
timing artifact -- `prlctl capture` caught a transient near-black
compositor frame (a repaint/blank moment) on a guest that was, by every
other preserved signal, rendering normally throughout -- not a rendering
regression in the kernel or compositor. **This reading is offered as
context, not as grounds to discard the FAIL**: the plan's own acceptance
oracle is the render verdict on the actual captured frame, and this run's
verdict is FAIL, full stop.

## Overall result

- `WAIT_STRESS_PASS`, 0 `STALL`: **met**.
- Five independent 120s render verdicts, all `PASS`: **not met** -- 4 of 5
  (`run1`, `run2`, `run3`, `run4`) are `PASS`; `run5` is `FAIL`.

Per the assigned oracle (`P:1029`/`P:1141` in
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/astra/PLAN-LAYER-SWEEP.md`,
quoted in the prior round's `DISPOSITIONS.md`), the acceptance criterion is
five independent runs each passing, not four of five or a majority. This
round's task instructions were followed literally on this point: no
closing comment was posted to #483 or #465, and neither issue was closed.
This directory documents the attempt and its exact outcome regardless.

## Not claimed

- That run 5's FAIL is proven to be a capture-timing artifact rather than
  a genuine rendering defect -- the surrounding-evidence reading above is
  offered as context (heartbeat/FPS/fatal-marker health throughout that
  same run), not as an independently reproduced root cause. No repeat
  capture of the same boot was taken to confirm the frame was transient.
- That five runs sharing one SHA in one session establishes
  session-to-session or SHA-to-SHA behavioral equivalence with any other
  sweep -- this is one five-run batch, reported on its own terms.
- That the host-side slowdown observed in run 5 (uptime_ms=320681 vs.
  136627-164662 for runs 1-4) has an identified root cause -- it is
  reported as an observed timing difference between this run and the
  other four, not diagnosed further here.
