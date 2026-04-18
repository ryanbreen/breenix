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
