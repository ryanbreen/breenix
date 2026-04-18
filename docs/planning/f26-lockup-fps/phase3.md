# F26 Phase 3 - Lockup Fix

Date: 2026-04-18

## Root Cause

Two independent issues were exposed after removing the F25 per-frame bounce sleep:

1. Bounce used Breengel's default `Window::new`, which loads `/etc/fonts.conf` and font files during window construction and polls `/etc/fonts.conf` every 20 `poll_events()` calls. In a high-FPS animation this created post-bwm AHCI filesystem reads under GUI load and reproduced AHCI timeout behavior.
2. `mark_window_dirty` woke bwm before marking the client blocked for frame back-pressure. If bwm consumed the frame immediately, `sched.unblock(client)` saw a still-running bounce thread and did nothing. Bounce then blocked afterward and waited for the 50 ms fallback, capping frame cadence around 20-25 Hz.

## Fix

- Added `Window::new_without_fonts()` in Breengel for continuous animation demos that must avoid post-compositor filesystem I/O.
- Switched bounce to `Window::new_without_fonts()`.
- Moved the compositor wake in `kernel/src/syscall/graphics.rs` until after `block_current_for_compositor()`.
- Changed the missed-wake fallback from 50 ms to the existing 5 ms compositor frame interval.
- Updated `run.sh --parallels --test` to fall back to `prlctl capture` when the window-based screenshot helper cannot find a Parallels window.

## Validation

Final validation artifacts:

```text
logs/f26/backpressure-5ms/serial.log
logs/f26/backpressure-5ms/capture-60s.png
logs/f26/backpressure-5ms/capture-90s.png
logs/f26/backpressure-5ms/capture-110s.png
```

Fault marker grep was empty:

```text
SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC
```

Strict F23 verdicts passed at 60, 90, and 110 seconds.

Image diffs showed visible animation continued:

```text
capture-60s.png -> capture-90s.png: changed_pixels=28732
capture-90s.png -> capture-110s.png: changed_pixels=26378
```
