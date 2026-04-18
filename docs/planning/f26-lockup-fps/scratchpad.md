# Scratchpad - F26 Post-Boot Lockup + Bounce FPS Regression

## 2026-04-18 - Setup

Starting from `main` at `9218aabb` on branch `fix/f26-post-boot-lockup-fps`.

Created Beads issue `breenix-53k` and claimed it.

About to start M1: reproduce and bound the Parallels lockup using a 120 second test, screenshots at 60/90/110 seconds, strict F23 verdict, serial cadence checks, and FPS computation.

## 2026-04-18 - M1 Reproduction Run

About to run `./run.sh --parallels --test 120` and capture the live VM display at 60, 90, and 110 seconds after the VM appears in `prlctl list`. I will copy `/tmp/breenix-parallels-serial.log` to `logs/f26/reproduce/serial.log` after the run and classify the lockup from frame cadence plus fault markers.

Result: initial wrapper latched onto stale VM `breenix-1776505344`, but recovered by manually capturing actual VM `breenix-1776505744` at 60/90/110 seconds. All strict render verdicts passed. No lockup reproduced: timer ticks continued through `ticks=160000` and compositor reached `Frame #11000`. Fault marker grep was empty. FPS estimate from `Frame #500` near `ticks=5000` to `Frame #11000` near `ticks=155000` is `10500 / 150 = 70 Hz`.

## 2026-04-18 - M2 Diagnosis

About to document the Phase 2 diagnosis as "lockup not reproduced under sustained 120s+ Parallels GUI load." This means there is no honest first-stuck component to fix yet. Evidence rules out a deterministic kernel timer stop, AHCI timeout, or bwm compositor stop in the measured run.

## 2026-04-18 - M4 FPS Change

Confirmed `7e7744c9` added exactly one sleep to the window-buffer path: `let _ = time::sleep_ms(1);` after `win.present()` in `run_window_loop`. About to remove that line only. The mmap fallback sleep existed outside the F25 diff and is not part of this window-mode regression.

No-sleep validation failed: strict captures passed, but serial reported repeated `[ahci] Port 1 TIMEOUT (5s)` after bounce started. Timer reached `ticks=120000`, so this was not a dead CPU. Only `Frame #500` appeared near `ticks=110000`, so bwm progress was effectively wedged/log-flooded. The likely trigger is Breengel `Window::poll_events()` invoking `FontWatcher::poll()` every 20 frames, which reads `/etc/fonts.conf`; uncapping bounce turns font hot-reload polling into post-bounce AHCI reads under GUI load. About to disable font polling for bounce after the initial font load instead of restoring the 1 ms sleep.

Follow-up validation with post-construction polling disabled still failed earlier: serial stopped after `[window] Created buffer id=1` and before `[bounce] Window mode`, so the initial `FontWatcher::new()` load inside `Window::new()` is also part of the post-bwm filesystem I/O hazard. About to add `Window::new_without_fonts()` for animation demos and switch bounce to it so window creation performs no font config/font file reads.

No-font validation progressed through `[bounce] Window mode`, bwm discovery, and `ticks=5000`, with no AHCI timeout, then serial stopped and 60/90/110 captures were identical. So the remaining lockup is the fully tight present loop itself, not font I/O. About to replace the F25 `sleep_ms(1)` with `sched_yield()` after `present()` to keep cooperative scheduling without adding a 1 ms delay.

Yield validation emitted frames quickly (`Frame #1500`) but caused Parallels capture/list calls to hang, so it is not a valid fix. About to try bounded pacing: keep no-font bounce, remove the per-frame sleep, and sleep 1 ms every fourth presented frame. This preserves scheduler relief while aiming for >100 Hz average instead of the F25 ~70 Hz per-frame sleep.

Bounded pacing validated captures, but FPS fell to 25 Hz (`Frame #500` near `ticks=20000`, `Frame #2000` near `ticks=80000`). Root cause found in kernel graphics op15: `mark_window_dirty` woke bwm before putting the client into `BlockedOnTimer`. If bwm consumed the frame immediately, `sched.unblock(client)` saw a still-running thread and did nothing; the client then blocked and waited for the 50 ms fallback. About to move the compositor wake after `block_current_for_compositor()` and remove userspace pacing.

Ordering-only kernel back-pressure still produced `Frame #500` near `ticks=20000`, so missed wakes were still falling back to the 50 ms timeout. About to change the fallback timeout to the existing `MIN_FRAME_INTERVAL_NS` (5 ms) so even a missed compositor wake degrades to the intended ~200 Hz pacing instead of 20-25 Hz.

5 ms fallback validation succeeded at the guest level. Captures at 60/90/110 passed strict F23 and changed between captures. Fault grep was empty. FPS estimate: `Frame #1000` near `ticks=5000` to `Frame #13000` near `ticks=85000` = `12000 / 80 = 150 Hz`. `run.sh --parallels --test` still returned 1 because `scripts/parallels/screenshot-vm.sh` could not find the Parallels window, while direct `prlctl capture` succeeded. Added a fallback in `run.sh`.

Final `./run.sh --parallels --test 120 --no-build` returned 0 after the screenshot fallback. `/tmp/breenix-screenshot.png` passed F23 strict verdict. Fault grep was empty. FPS estimate improved to `Frame #1000` near `ticks=5000` through `Frame #17000` near `ticks=105000` = `16000 / 100 = 160 Hz`.
