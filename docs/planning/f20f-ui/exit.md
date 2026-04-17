# F20f Exit: Parallels display capture and UI rendering investigation

## Terminal State

F20f completed Phase 0 and exhausted all four UI-fix cycles without meeting the
acceptance criterion.

No captured PNG showed rendered desktop content. The final capture remained a
single solid red color:

- Final captured PNG: `logs/f20f-cycle-4/screen.png`
- Final content crop: `(255, 0, 0)` at `100.0000%`
- Unique colors: `1`
- PR URL: none; no winning fix was found.

## Phase 0 Result

Reliable screenshot capture is working through `prlctl capture`:

- Tool: `scripts/parallels/capture-display.sh`
- Baseline capture: `logs/f20f-baseline/red.png`
- Baseline content crop: `(255, 0, 0)` at `100.0000%`

The helper writes PNG bytes to stdout, diagnostics to stderr, retries captures,
and rejects all-black captures.

## Cycle Summary

| Cycle | Hypothesis | Screenshot | Result | Verdict |
| --- | --- | --- | --- | --- |
| 1 | Route `bsh` JavaScript `spawn()` through ARM64 `SPAWN` syscall to avoid fork/COW issues. | `logs/f20f-cycle-1-patched/screen.png` | `(255, 0, 0)` at `100.0000%`; spawn syscall hung at first service. | Failed |
| 2 | Start `bwm` first in `/etc/init.js` before services. | `logs/f20f-cycle-2/screen.png` | `(255, 0, 0)` at `100.0000%`; no `[bwm]`, no boot-script completion. | Failed |
| 3 | Start `bwm` directly from PID 1 before the JavaScript boot script. | `logs/f20f-cycle-3/screen.png` | `(255, 0, 0)` at `100.0000%`; `bwm` and `bsh` exec failed with `EIO`. | Failed |
| 4 | Serialize direct `bwm` startup with a one-second sleep before starting the boot script. | `logs/f20f-cycle-4/screen.png` | `(255, 0, 0)` at `100.0000%`; boot stopped at `/sbin/init` preload. | Failed |

## Strongest F21 Hypothesis

F21 should move below userland and debug the Parallels VirGL scanout path.

Every F20f capture stayed solid red even though the kernel logs a successful
VirGL initialization sequence before userspace starts:

- `SUBMIT_3D OK`
- `VirGL CLEAR (cornflower blue)`
- `SET_SCANOUT + RESOURCE_FLUSH done`
- later full pipeline draw batch also returns `SUBMIT_3D OK`

If those commands actually updated the visible scanout, the Phase 0 baseline and
all cycle screenshots should not remain pure red. The likely fault is therefore
in the virtio-gpu/VirGL display update path: scanout resource selection,
flush/fence ordering, resource backing/format, or a missing host-visible update
step after the 3D render target is drawn.

## Self-Audit

- No polling was added.
- No prohibited interrupt/syscall files were edited.
- F1-F20e commits were not reverted.
- Aarch64 build logs for the cycle runs had no compile-stage warnings or
  errors.
- QEMU cleanup remains required before handoff.
