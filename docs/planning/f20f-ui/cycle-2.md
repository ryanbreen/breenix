# F20f Cycle 2: Start compositor before background services

## Hypothesis

Cycle 1 showed that routing JavaScript `spawn()` through the ARM64 spawn syscall
hangs at the first service launch, so the cycle reverted to the original
fork+exec implementation.

The remaining observation was that the generated init script started system
services before the window compositor. If an early background service consumed
the first reliable fork/exec path or blocked later script progress, launching
`/bin/bwm` first should produce at least a visible compositor frame before
services run.

## Patch Summary

- `userspace/programs/src/bsh.rs`: restored JavaScript `spawn()` to the
  fork+exec path after the failed Cycle 1 syscall experiment.
- `scripts/create_ext2_disk.sh`: reordered generated `/etc/init.js` so
  `/bin/bwm` starts before GUI apps and background services.

## Captures

Baseline:

- `logs/f20f-baseline/red.png`
- Content crop: `(255, 0, 0)` at `100.0000%`

Cycle 2 patched run:

- `logs/f20f-cycle-2/screen.png`
- Content crop: `(255, 0, 0)` at `100.0000%`
- Unique colors: `1`
- Serial: `logs/f20f-cycle-2/serial.log`

## Serial Evidence

The boot reached init and printed the shell banner, but did not reach the
compositor:

```text
[init] Breenix init starting (PID 1)
Welcome to Breenix OS
```

The serial log contains no `[bwm]` lines and no `[init] Boot script completed`.

## Verdict

Failed. The captured display remained exactly the red baseline, and launch order
did not get `bwm` to visible execution. Cycle 3 should bypass JavaScript
background spawning and directly test whether `/bin/bwm` can render when it is
`exec`'d by init.
