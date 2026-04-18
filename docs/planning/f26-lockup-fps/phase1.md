# F26 Phase 1 - Reproduce and Bound

Date: 2026-04-18

Branch: `fix/f26-post-boot-lockup-fps`

Base: `9218aabb`

## Command

Primary run:

```bash
./run.sh --parallels --test 120
```

The first capture wrapper latched onto stale VM `breenix-1776505344`, which `run.sh` then deleted while creating the fresh test VM. The actual test VM was `breenix-1776505744`; captures were recovered manually against that VM at 60, 90, and 110 seconds based on the timestamped VM name.

The stale capture wrapper delayed cleanup, so the VM continued beyond the nominal 120 second test window. That gave extra serial evidence: timer ticks and compositor frames continued through roughly tick 160000.

Serial log:

```text
logs/f26/reproduce/serial.log
```

Captures:

```text
logs/f26/reproduce/capture-60s.png
logs/f26/reproduce/capture-90s.png
logs/f26/reproduce/capture-110s.png
```

## Strict Render Verdicts

```text
=== 60s ===
distinct=2194 dominant=(10, 10, 25) dom_frac=0.0894
big_color_buckets=12 blue_baseline=False red_baseline=False
VERDICT=PASS

=== 90s ===
distinct=2118 dominant=(10, 10, 25) dom_frac=0.0731
big_color_buckets=12 blue_baseline=False red_baseline=False
VERDICT=PASS

=== 110s ===
distinct=1945 dominant=(10, 10, 25) dom_frac=0.0742
big_color_buckets=12 blue_baseline=False red_baseline=False
VERDICT=PASS
```

Image diff checks also showed visible changes between captures:

```text
capture-60s.png -> capture-90s.png: changed_pixels=60575
capture-90s.png -> capture-110s.png: changed_pixels=17284
```

## Lockup Verdict

The reported post-boot hard lockup did not reproduce in this Phase 1 run.

Evidence:

```text
[timer] cpu0 ticks=145000
[virgl-composite] Frame #10000: 1280x960 -> 1280x960 display
[timer] cpu0 ticks=150000
[virgl-composite] Frame #10500: 1280x960 -> 1280x960 display
[timer] cpu0 ticks=155000
[virgl-composite] Frame #11000: 1280x960 -> 1280x960 display
[timer] cpu0 ticks=160000
```

There were no matches for:

```text
SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC
```

Kernel-vs-userspace verdict: neither a kernel lockup nor a userspace compositor lockup reproduced. Timer ticks continued, and bwm continued driving the VirGL compositor.

## Last Frame

Last emitted compositor frame:

```text
[virgl-composite] Frame #11000
```

Nearest preceding timer marker:

```text
[timer] cpu0 ticks=155000
```

The next timer marker also appeared:

```text
[timer] cpu0 ticks=160000
```

## FPS Estimate

The serial log prints compositor frames every 500 frames and timer ticks every 5000 ticks, so per-window estimates have +/- 5 second quantization. The least noisy Phase 1 estimate uses the first and last frame markers with nearby timer markers:

```text
first measured frame: Frame #500 near ticks=5000
last measured frame:  Frame #11000 near ticks=155000
elapsed frames:       10500
elapsed time:         150 seconds
estimated FPS:        10500 / 150 = 70 Hz
```

Verdict: the hard lockup did not reproduce, but the FPS regression remains. The measured compositor cadence is about 70 Hz, below the Phase 5 target of at least 100 Hz and below the historical 200 Hz target.
