# Turn 3 perturbation diagnosis

Status: COMPLETE.

Turn 3 tested whether the Turn 2 attribution counters perturbed the scheduler
by booting clean `main` (`fb9d81ef`) in a temporary branch with no Turn 2 code.
The baseline boot failed with the same no-render soft-lock signature as the
instrumented Turn 2 boot, so the decision tree lands on Branch B:
environmental regression.

Artifacts:

- `turn3-artifacts/baseline-boot/kernel-build.txt`
- `turn3-artifacts/baseline-boot/run.out`
- `turn3-artifacts/baseline-boot/serial.log`
- Comparison source: `turn2-artifacts/parallels-boot/serial.log`

## A. Baseline boot result

Baseline checkout:

- Branch: temporary `experiment/turn3-baseline`
- Commit: `fb9d81ef Make virtio-gpu completion interrupt-driven (#343)`
- Build: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- Build result: clean kernel build, `Finished release profile` only in `turn3-artifacts/baseline-boot/kernel-build.txt`
- Parallels VM: `breenix-1779213465`
- Cleanup: `prlctl stop breenix-1779213465 --kill`; `prlctl delete breenix-1779213465`

The baseline did not reach active rendering:

| Metric | Baseline value |
| --- | ---: |
| Max captured uptime | `235550 ms` |
| Final `fps_last_5s` | `0` |
| Positive FPS samples | `0` |
| Final `submits` | `62` |
| Final `completes` | `65` |
| Soft-lock markers | `64` |
| Rescue markers | `0` |

Raw evidence:

```text
turn3-artifacts/baseline-boot/serial.log:339:!!! SOFT LOCKUP DETECTED !!!
turn3-artifacts/baseline-boot/serial.log:1638:[freeze-watch] uptime_ms=1734 submits=62 completes=65 fails=0 last_completion_ms=75 fps_last_5s=0 ...
turn3-artifacts/baseline-boot/serial.log:82503:[freeze-watch] uptime_ms=235550 submits=62 completes=65 fails=0 last_completion_ms=75 fps_last_5s=0 ...
```

The run harness also regenerated the userspace/ext2 image and reported:

```text
turn3-artifacts/baseline-boot/run.out:23:  ERROR: rust-fork/library not found
turn3-artifacts/baseline-boot/run.out:25:  WARNING: Userspace build failed (rust-fork may not be set up)
turn3-artifacts/baseline-boot/run.out:34:  WARNING: busybox.elf not found, skipping coreutils
```

Those warnings are not kernel compiler warnings, but they are material
environmental evidence because the standard Parallels harness recreated the
data disk with no userspace binaries during the failing baseline.

## B. Turn 2 instrumented boot comparison

Turn 2 boot:

- Commit: `86c881d7` plus docs/evidence commit `b69fdcfc`
- Artifact: `turn2-artifacts/parallels-boot/serial.log`
- Final freeze-watch sample reached `uptime_ms=240568`

| Metric | Turn 2 instrumented value |
| --- | ---: |
| Max captured uptime | `240568 ms` |
| Final `fps_last_5s` | `0` |
| Positive FPS samples | `0` |
| Final `submits` | `62` |
| Final `completes` | `65` |
| Soft-lock markers | `65` |
| Rescue markers | `0` |
| Final `rescue_total` counter | `0` |

Raw evidence:

```text
turn2-artifacts/parallels-boot/serial.log:339:!!! SOFT LOCKUP DETECTED !!!
turn2-artifacts/parallels-boot/serial.log:1620:[freeze-watch] uptime_ms=1740 submits=62 completes=65 fails=0 last_completion_ms=76 fps_last_5s=0 ...
turn2-artifacts/parallels-boot/serial.log:82792:[freeze-watch] uptime_ms=240568 submits=62 completes=65 fails=0 last_completion_ms=76 fps_last_5s=0 ...
turn2-artifacts/parallels-boot/serial.log:76343:[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
```

The baseline and instrumented runs match on the important symptoms:

- Both freeze at the same early GPU command counts: `submits=62`, `completes=65`.
- Both report `fps_last_5s=0` from the first freeze-watch sample through the end.
- Both emit repeated soft-lock dumps for the full window.
- Neither shows rescue markers.

## C. Branch identification

Branch B: baseline also soft-locks.

This falsifies the simple PROBE_PERTURBS explanation for the Turn 2 failure.
The Turn 2 counters may still have overhead, but they are not required to
produce the no-render soft-lock condition. Clean `main` at `fb9d81ef`, the
commit that previously passed the virtio-gpu 5-boot gate, now fails in the same
way under the current harness/environment.

The leading environmental clue is that `./run.sh --parallels` rebuilt the ext2
disk while userspace build support was unavailable:

- `rust-fork/library not found`
- `Userspace build failed`
- `busybox.elf not found`
- run output says `Installed 0 binaries in /bin`, `0` in `/sbin`, and `0` test binaries

That does not prove the ext2 image is the root cause, but it is the first
concrete divergence from the premise that the current environment matches the
earlier healthy 5-boot gate.

## D. Named Turn 4 scope

Turn 4 should investigate the environment/regression path, not scheduler
counter perturbation:

1. Preserve the current failing artifacts and compare the generated
   `target/ext2-aarch64.img` / `testdata/ext2-aarch64.img` hash and contents
   against the artifact used by the earlier healthy virtio-gpu 5-boot gate, if
   available.
2. Restore or reproduce the healthy userspace build environment
   (`rust-fork/library`, BusyBox/coreutils artifacts), then rerun clean
   `fb9d81ef` with `./run.sh --parallels`.
3. If clean `fb9d81ef` becomes healthy again, rerun the Turn 2 instrumented
   branch with the same restored disk/userspace image.
4. If clean `fb9d81ef` still fails after restoring userspace/disk artifacts,
   inspect Parallels host/config state and compare the exact run harness inputs
   from the earlier healthy 5-boot gate.

Do not reduce or remove the Turn 2 scheduler counters yet. The baseline failure
means there is not enough evidence to label them as perturbing the scheduler.
