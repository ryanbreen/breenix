# F20f Phase 0: Parallels Display Capture Baseline

## Capture Mechanism

Primary capture uses `prlctl capture <vm> --file <png>`, wrapped by
`scripts/parallels/capture-display.sh` for binary-clean stdout capture.

Validation run:

- VM: `breenix-1776449657`
- Direct capture: `logs/f20f-baseline/red.png`
- Helper capture: `logs/f20f-baseline/script-capture.png`
- Helper stderr: `logs/f20f-baseline/capture-display.stderr`

Both PNGs were valid:

- Dimensions: `1280x960`
- Format: `8-bit/color RGB, non-interlaced`

## Baseline Color

The content-area crop excludes the top 8% of the image to avoid host/window
chrome. The entire remaining content region was solid red:

- Content pixels: `1,131,520`
- Dominant color: `(255, 0, 0)`
- Dominant color share: `100.0000%`

This is the baseline for F20f cycle comparisons.

## Notes

The previous `scripts/parallels/screenshot-vm.sh` Quartz-window fallback is not
reliable for this factory because it failed to find the offscreen Parallels
window during test mode. Direct `prlctl capture` succeeded without requiring a
visible Mac window.
