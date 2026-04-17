# F20f Cycle 4: Serialize compositor startup before boot script

## Hypothesis

Cycle 3 overlapped `/bin/bwm` and `/bin/bsh` exec reads from the AHCI-backed
ext2 root. Both failed with `EIO`, which matched an existing init comment about
avoiding overlapping early exec reads during initial userspace bring-up.

Since the generated init script already uses one-second sleeps between spawned
programs, Cycle 4 added a single bounded sleep after starting bwm and before
starting the boot-script child. This is serialization, not polling.

## Patch Summary

- `userspace/programs/src/init.rs`: sleep for one second after `start_bwm()` and
  before `run_boot_script()`.

## Captures

Baseline:

- `logs/f20f-baseline/red.png`
- Content crop: `(255, 0, 0)` at `100.0000%`

Cycle 4 patched run:

- `logs/f20f-cycle-4/screen.png`
- Content crop: `(255, 0, 0)` at `100.0000%`
- Unique colors: `1`
- Serial: `logs/f20f-cycle-4/serial.log`

## Serial Evidence

The run did not reach init. The serial log stops here:

```text
[boot] Tracing subsystem initialized and enabled
[boot] Pre-loading /sbin/init from ext2 (before timer)...
```

There are no `[init]` or `[bwm]` lines in the Cycle 4 serial log.

## Verdict

Failed. The captured display remained exactly the red baseline, and the boot did
not reach the compositor.

The four-cycle budget is exhausted. The strongest remaining F21 hypothesis is
below userland: every capture stayed red even though the kernel VirGL init path
logs successful `SUBMIT_3D`, `SET_SCANOUT`, and `RESOURCE_FLUSH` operations.
F21 should debug why that acknowledged scanout/flush sequence does not replace
the red display resource in Parallels.
