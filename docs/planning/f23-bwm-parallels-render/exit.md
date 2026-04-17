# F23 Exit — bwm Parallels Render

## What Changed

- Added `scripts/f23-render-verdict.sh`, a strict PNG verdict that rejects solid cornflower blue, solid red, dominant-color frames, low distinct-color counts, and insufficient quantized color regions.
- Documented the 75-second failing checkpoint in `docs/planning/f23-bwm-parallels-render/phase1-analysis.md`.
- Moved ARM64 init's `/bin/bwm` service before `/sbin/telnetd` in `userspace/programs/src/init.rs`.

## Root Cause

At the 75-second Parallels validation point, bwm had not been spawned yet. ARM64 init launched `/sbin/telnetd` first, and the capture happened while init was still in that service path. The only visible frame was the kernel VirGL proof clear: solid `(100, 149, 237)`.

## Validation Cycles

### Known Bad Baselines

```text
logs/f23-prep/fresh-capture.png
distinct=1 dominant=(100, 149, 237) dom_frac=1.0000
big_color_buckets=1 blue_baseline=True red_baseline=False
VERDICT=FAIL
```

```text
logs/f22-validation/baseline/solid-red.png
distinct=1 dominant=(255, 0, 0) dom_frac=1.0000
big_color_buckets=1 blue_baseline=False red_baseline=True
VERDICT=FAIL
```

### Cycle 1 — FAIL

- Branch HEAD: `5e421ec8`
- Capture timing: 75 seconds after `--- Starting VM ---`
- Capture: `.factory-runs/f23-bwm-parallels-render/final/final-capture.png`

```text
distinct=1 dominant=(100, 149, 237) dom_frac=1.0000
big_color_buckets=1 blue_baseline=True red_baseline=False
VERDICT=FAIL
```

Last checkpoint: init started `/sbin/telnetd`; `/bin/bwm` was not spawned before capture.

### Cycle 2 — PASS

- Branch HEAD: `a2f58990`
- Rebuild command: `./run.sh --clean --parallels --test 90`
- Capture timing: 75 seconds after `--- Starting VM ---`
- Capture: `.factory-runs/f23-bwm-parallels-render/cycle-2/fresh-capture.png`

```text
distinct=100 dominant=(17, 19, 48) dom_frac=0.0246
big_color_buckets=10 blue_baseline=False red_baseline=False
VERDICT=PASS
```

Serial checkpoints:

```text
[spawn] path='/bin/bwm'
[bwm] Breenix Window Manager starting... (v2-chromeless-skip)
[virgl-composite] Frame #1: 1280x960 → 1280x960 display
[spawn] path='/sbin/telnetd'
```

## Quality Gates

- `userspace/programs/build.sh --arch aarch64`: pass.
- `./run.sh --clean --parallels --test 90`: clean compile stage; warning/error grep produced no output.
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: pass; warning/error grep produced no output.

## Self-Audit

- No Tier 1 prohibited files modified.
- No interrupt/syscall hot path changes.
- No reverts of F1-F22 or PRs through #315.
- No polling fallback added.
- No false-positive claim: final PASS is from a fresh clean rebuild of branch HEAD `a2f58990`, captured after the target 75-second window and evaluated by `scripts/f23-render-verdict.sh`.

## PR

https://github.com/ryanbreen/breenix/pull/316
