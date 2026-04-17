# F20f Cycle 3: Start compositor directly from init

## Hypothesis

Cycles 1 and 2 showed that the JavaScript boot-script path did not get `bwm` to
visible execution. The compositor may need to start before the JavaScript shell
has initialized enough heap/runtime state to make fork+exec fragile.

PID 1 is a small Rust process and can own startup of core services directly,
while `/etc/init.js` remains responsible for GUI clients and background services.

## Patch Summary

- `userspace/programs/src/init.rs`: fork+exec `/bin/bwm` directly before
  running the boot script.
- `scripts/create_ext2_disk.sh`: remove `/bin/bwm` from generated
  `/etc/init.js` so the script does not start a duplicate compositor.

## Captures

Baseline:

- `logs/f20f-baseline/red.png`
- Content crop: `(255, 0, 0)` at `100.0000%`

Cycle 3 patched run:

- `logs/f20f-cycle-3/screen.png`
- Content crop: `(255, 0, 0)` at `100.0000%`
- Unique colors: `1`
- Serial: `logs/f20f-cycle-3/serial.log`

## Serial Evidence

The run started the compositor child but the early exec path failed:

```text
[init] Breenix init starting (PID 1)
[init] bwm started (PID 2)
[init] Failed to exec bwm: EIO
[init] Boot script completed
[init] bsshd started (PID 4)
[init] Process 2 exited (code 127)
[init] Failed to exec bsh: EIO
[init] Process 3 exited (code 127)
```

The end-of-run soft lockup dump reported `EXEC_TOTAL: 0`, confirming no
userspace exec completed successfully in this cycle.

## Verdict

Failed. The captured display remained exactly the red baseline.

The new evidence supports a narrower Cycle 4 hypothesis: direct compositor
startup from init is still plausible, but Cycle 3 launched the boot-script child
immediately after forking bwm. That overlaps two early executable reads from the
AHCI-backed ext2 root. Existing init code already delays `bsshd` until after the
boot script for the same class of risk, so Cycle 4 should serialize bwm startup
with a bounded sleep, not polling.
