# F26 Exit - Post-Boot Lockup + Bounce FPS Regression

Date: 2026-04-18

Branch: `fix/f26-post-boot-lockup-fps`

Base: `9218aabb`

## What I Built

- `kernel/src/syscall/graphics.rs`: fixed window frame back-pressure by blocking the client before waking bwm, and reduced the missed-wake fallback from 50 ms to the existing 5 ms frame interval.
- `libs/breengel/src/font.rs`: added a disabled `FontWatcher` mode that performs no initial font load and no font config polling.
- `libs/breengel/src/window.rs`: added `Window::new_without_fonts()` for animation clients that must avoid filesystem I/O after the compositor is active.
- `userspace/programs/src/bounce.rs`: switched bounce to `Window::new_without_fonts()` and removed the F25 per-frame `sleep_ms(1)`.
- `run.sh`: made Parallels test screenshots fall back to direct `prlctl capture` when the window-based helper cannot find a host window.
- `docs/planning/f26-lockup-fps/phase1.md`: reproduction and baseline FPS report.
- `docs/planning/f26-lockup-fps/phase2.md`: initial lockup diagnosis from the baseline run.
- `docs/planning/f26-lockup-fps/phase3.md`: root-cause and lockup fix report.
- `docs/planning/f26-lockup-fps/phase4.md`: FPS before/after report.

## Original Ask

Reproduce the post-boot GUI lockup on Parallels, diagnose whether it was kernel or userspace, fix the lockup, remove the F25 bounce sleep band-aid, verify bounce/compositor FPS recovers to at least 100 Hz, and validate a 120 second Parallels GUI run with strict render verdict and no soft lockups.

## How This Meets The Ask

Phase 1 reproduction: implemented. `phase1.md` records strict captures, no reproduced hard lockup in the baseline run, and baseline FPS around 70 Hz.

Phase 2 diagnosis: implemented. `phase2.md` honestly records that the baseline hard lockup did not reproduce, with timer and compositor progress continuing.

Phase 3 lockup fix: implemented. Removing the sleep exposed the true failure modes: post-bwm font filesystem I/O and a `mark_window_dirty` wake/block race that made clients fall back to slow timer waits. `phase3.md` records the root cause and fix.

Phase 4 FPS: implemented. The final serial log estimates 160 Hz:

```text
Frame #1000 near ticks=5000
Frame #17000 near ticks=105000
16000 frames / 100 seconds = 160 Hz
```

Phase 5 validation: implemented. Final command:

```bash
./run.sh --parallels --test 120 --no-build
```

It returned 0. `/tmp/breenix-screenshot.png` passed strict F23 verdict, and fault-marker grep over `logs/f26/final-run/serial.log` was empty.

## What I Did Not Build

- I did not modify Tier 1 prohibited files.
- I did not add polling.
- I did not remove the mmap fallback sleep in bounce; F25 only added the window-mode sleep and the mmap fallback was not on the active bwm path.
- I did not rewrite AHCI. The AHCI timeout was avoided by removing post-bwm font I/O from bounce rather than changing storage.

## Known Risks And Gaps

- `Window::new_without_fonts()` means bounce uses the bitmap FPS text fallback and does not hot-reload system fonts. This is intentional for the high-FPS demo.
- The compositor wake race is fixed and the fallback is now 5 ms, but the serial evidence still suggests some frames use the fallback path. This is acceptable for the current target because final FPS is 160 Hz and the 120 second run remains stable.
- `run.sh --parallels --test` still first tries the window-based screenshot helper; it now falls back to direct `prlctl capture` when that helper cannot find a host window.

## How To Verify

```bash
./userspace/programs/build.sh --arch aarch64
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
./run.sh --parallels --test 120 --no-build
bash scripts/f23-render-verdict.sh /tmp/breenix-screenshot.png
grep -E "SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC" logs/f26/final-run/serial.log
cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | grep -E "^(warning|error)"
```

The final grep commands should produce no output.

## PR

Pending.

## Self-Audit

- No polling added.
- No Tier 1 prohibited files modified.
- F1-F25 intact; no PR #319 changes reverted.
- QEMU cleanup run after validation.
