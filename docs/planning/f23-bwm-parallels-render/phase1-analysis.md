# F23 Phase 1 Analysis

## Baseline Verdict Tightening

`scripts/f23-render-verdict.sh` rejects both known bad baselines:

- `logs/f23-prep/fresh-capture.png`: `VERDICT=FAIL`, `distinct=1`, dominant `(100, 149, 237)`.
- `logs/f22-validation/baseline/solid-red.png`: `VERDICT=FAIL`, `distinct=1`, dominant `(255, 0, 0)`.

## Reproduction

Fresh branch rebuild from `5e421ec8` was captured 75 seconds after the Parallels `--- Starting VM ---` marker.

Strict verdict:

```text
distinct=1 dominant=(100, 149, 237) dom_frac=1.0000
big_color_buckets=1 blue_baseline=True red_baseline=False
VERDICT=FAIL
```

## Serial Checkpoints

At the 75-second capture:

- Kernel VirGL initialized successfully through Step 10, leaving the cornflower-blue proof clear visible.
- Init reached userspace and started its ARM64 service sequence.
- Init was still in the first service path, `/sbin/telnetd`, with serial ending at `T3T4`.
- There was no `[spawn] path='/bin/bwm'` before capture.
- There was no `[bwm]` output before capture.
- There was no bwm `[syscall] exit`; bwm had not been created yet.

Last checkpoint before failure: init blocked/delayed before bwm spawn because ARM64 launched telnetd first.

## Fix

Move `/bin/bwm` before `/sbin/telnetd` in the ARM64 init service list so the compositor replaces the kernel proof clear before network services can delay the validation window.
