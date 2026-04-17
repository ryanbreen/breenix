# F20f Cycle 1: Route init-script spawn through ARM64 spawn syscall

## Hypothesis

The baseline boot reaches `[init] Boot script completed`, but the 120-second
serial log has no `[bwm]` lines and no post-init VirGL draw activity. That
suggested that `bsh`'s JavaScript `spawn()` was returning without GUI children
actually reaching exec/runtime.

Because ARM64 already has a dedicated `SPAWN` syscall described as avoiding
fork/COW issues with shared display mappings, Cycle 1 tried using that syscall
for `bsh` init-script spawns on ARM64 while preserving fork+exec elsewhere.

## Patch Summary

- `userspace/programs/src/bsh.rs`: on `target_arch = "aarch64"`, `spawn(cmd)`
  calls `libbreenix::process::spawn()` instead of fork+exec.

## Captures

Baseline:

- `logs/f20f-baseline/red.png`
- Content crop: `(255, 0, 0)` at `100.0000%`

Cycle 1 pre-patch observation:

- `logs/f20f-cycle-1/screen.png`
- Content crop: `(255, 0, 0)` at `100.0000%`
- Serial: `logs/f20f-cycle-1/serial.log`
- Evidence: `[init] Boot script completed`; no `[bwm]` lines.

Cycle 1 patched run:

- `logs/f20f-cycle-1-patched/screen.png`
- Content crop: `(255, 0, 0)` at `100.0000%`
- Serial: `logs/f20f-cycle-1-patched/serial.log`

## Serial Evidence

The patched run exercised the syscall path, then stopped making progress:

```text
[init] Breenix init starting (PID 1)
Welcome to Breenix OS
[spawn] path='/sbin/telnetd'
```

There was no `[spawn] Success`, no `[init] Boot script completed`, and no
`[bwm]` startup line.

## Verdict

Failed. The screen remained exactly the red baseline, and the patch regressed
boot-script progress by hanging at the first spawn syscall. The syscall-based
spawn path is not a viable Cycle 1 fix under the current constraints because
the implementation lives in prohibited ARM64 syscall code.

Cycle 2 should avoid the ARM64 spawn syscall and test whether launching the
compositor earlier/directly changes the visible display outcome.
