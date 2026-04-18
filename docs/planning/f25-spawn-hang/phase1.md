# F25 Phase 1 — hello_raw Narrowing

Date: 2026-04-18

## Setup

- Branch: `fix/f25-post-bwm-spawn-hang`
- Base: `97b441823473b81de7ba3ba89404c217903e0cd5`
- Test edit: ARM64 init service list changed to:
  1. `/bin/bwm`
  2. `/bin/hello_raw`
  3. `/sbin/telnetd`
- Spawn pacing: 75ms `nanosleep` after each service spawn.

## Build

`./userspace/programs/build.sh --arch aarch64` passed.

`./scripts/create_ext2_disk.sh --arch aarch64` passed under Docker. The generated ext2 image contains `/bin/hello_raw` at 278000 bytes.

## Parallels Run

Command:

```bash
> /tmp/breenix-parallels-serial.log
./run.sh --parallels --test 90
```

The run reached the screenshot step but failed to capture because no Parallels window matched the generated VM name. Serial was still usable for the Phase 1 verdict.

Relevant serial:

```text
[init] Breenix init starting (PID 1)
[spawn] path='/bin/bwm'
[spawn] Created child PID 2 for parent PID 1
[spawn] Success: child PID 2 scheduled
[bwm] Breenix Window Manager starting... (v2-chromeless-skip)
[bwm] GPU compositing mode (VirGL), display: 1280x960
[bwm] Direct compositor mapping: 1280x960 at 0x7ffffdb4e000
[bwm] using bitmap font fallback for early boot
[bwm] hotkeys: using built-in defaults for early boot
[spawn] path='/bin/hello_raw'
```

There was no `[spawn] Created child PID ...` for `hello_raw`, no `[hello_raw] start`, and no `[syscall] exit(42)`.

## Verdict

`hello_raw` also hangs when spawned after `bwm`. This is not bounce-specific. The bug is architectural: spawning any binary after `bwm` stalls before child process creation.

Next step: add low-overhead breadcrumbs around the ARM64 spawn path and process creation path to identify the last completed phase.
