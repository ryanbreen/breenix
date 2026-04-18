# Scratchpad - F28 Eliminate the 5ms Wake Fallback

## 2026-04-18T10:53:11Z - Setup

Starting from `main` at `ab351efe` on branch `fix/f28-eliminate-wake-fallback`.

Created and claimed Beads issue `breenix-39z`.

Using `factory-orchestration` because this is an explicit multi-phase factory run. Read the stable runbook, F26 exit notes, `graphics.rs`, Breengel `Window::present()`, and the bwm event loop.

About to start M1: add low-frequency wake counter instrumentation in `kernel/src/syscall/graphics.rs` only. The event counter should increment when `handle_composite_windows()` consumes a dirty window generation and wakes the waiting client. The fallback counter should increment when `mark_window_dirty` resumes and still owns its `waiting_thread_id`, meaning the compositor did not consume and wake that wait before the timeout.


## 2026-04-18T10:58:00Z - M1 Instrumentation Edit

Added frame wake counters in graphics.rs. Workspace cargo fmt --check failed on pre-existing unrelated rustfmt/trailing-whitespace issues, so I will format only graphics.rs and use the aarch64 kernel build as the milestone gate.

M1 validation passed: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` exited 0 and `/tmp/f28-m1-kernel-build.log` had no `warning` or `error` lines.

About to commit M1 instrumentation before running the 120 second baseline workload.

Committed M1 on the F28 branch as `4133be00`. When unrelated dirty changes appeared in the original worktree, I created clean worktree `/Users/wrb/fun/code/breenix-f28-clean` from that commit and continued there to keep validation independent of other factories.

M2 result: final baseline counter line was `[gfx-wake] event=0 fallback=2191 total=2191 fallback_ppm=1000000`, so fallback ratio was 100%. Fault-marker grep was empty. The serial stream stopped after `Frame #2000`, so I am not claiming a full-run baseline FPS.

M3 edit: fixed compositor waiter publication ordering. `compositor_wait` now marks itself blocked before publishing `COMPOSITOR_WAITING_THREAD`, then re-checks dirty/mouse/registry before entering WFI.

M3 validation passed: aarch64 kernel build exited 0 and `/tmp/f28-m3-kernel-build.log` had no `warning` or `error` lines.

First M4 run returned 0, strict render verdict passed, fault-marker grep was empty, and FPS was healthy (`Frame #500` near `ticks=5000` through `Frame #13500` near `ticks=105000`, about 130 Hz over that interval). It did not emit `[gfx-wake]` lines after the fix, so the instrumentation was not sufficient for final counter validation.

Added total-count bucket dumps every 1000 client wake completions. Follow-up aarch64 kernel build passed with no `warning` or `error` lines in `/tmp/f28-m3b-kernel-build.log`.
