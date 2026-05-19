# Turn 4: Environment Fix and Re-baseline

## A. Root Cause

The scheduler investigation worktree did not have a usable userspace build environment.

- `rust-fork` is tracked as a symlink, not an initialized git submodule in this checkout:
  - `rust-fork -> /Users/wrb/fun/code/breenix-parallels/rust-fork`
  - the target is missing, so `rust-fork/library` does not exist.
- `userspace/programs/build.sh --arch aarch64` therefore fails with `ERROR: rust-fork/library not found`.
- The copied/generated `.elf` files are ignored artifacts, and this worktree initially had zero `userspace/programs/aarch64/*.elf` files.
- Regenerating ext2 in that state creates a disk without the real userspace binaries. That explains the prior no-render/soft-lock signature: the kernel booted with an empty runtime image rather than a meaningful BWM workload.

`git submodule update --init --recursive` was not sufficient because the checked-out `rust-fork` path is a tracked symlink.

## B. Fix Applied

I used directive Approach B: restore the known-good userspace runtime artifacts from the main repo.

- Source: `/Users/wrb/fun/code/breenix/userspace/programs/aarch64/`
- Destination: `userspace/programs/aarch64/`
- Copied artifacts: 154 `.elf` files.
- Required binaries verified present:
  - `init.elf`
  - `bwm.elf`
  - `bsh.elf`
  - `busybox.elf`
- Hashes recorded in `turn4-artifacts/userspace/copied-artifact-sha256.txt`.

After copying the artifacts, `scripts/create_ext2_disk.sh --arch aarch64` produced a populated ext2 image:

- BusyBox installed with hardlinks.
- 50 binaries installed in `/bin`.
- 3 binaries installed in `/sbin`.
- 5 C binaries installed in `/usr/local/cbin`.
- 95 test binaries installed in `/usr/local/test/bin`.

Important caveat: this restores the runtime artifact environment. It does not fix the missing `rust-fork` source-build prerequisite. `run.sh --parallels` still prints the userspace build warning, then proceeds to create a populated ext2 disk from the restored `.elf` artifacts.

## C. Fixed Clean-Main Baseline

Baseline branch: temporary `experiment/turn4-baseline-fixed` from clean `main` at `fb9d81ef`.

Artifacts:

- `turn4-artifacts/baseline-fixed/kernel-build.txt`
- `turn4-artifacts/baseline-fixed/run.out`
- `turn4-artifacts/baseline-fixed/serial.log`

Result: healthy. The VM rendered through the full evidence window and was stopped/deleted.

Final captured `freeze-watch`:

```text
uptime_ms=240483 submits=130808 completes=130811 fails=0 last_completion_ms=240480 fps_last_5s=181
```

Other baseline checks:

- `SOFT LOCKUP` / panic markers: 0
- `[SCHED] queue_empty rescue_tid=` markers: 5
- BWM and VirGL compositor frames present.

## D. Instrumented Rerun

Instrumented branch: `investigation/scheduler-wake-atomic` at `c1f54839`, including attribution instrumentation commit `86c881d7`.

Artifacts:

- `turn4-artifacts/instrumented-rerun/kernel-build.txt`
- `turn4-artifacts/instrumented-rerun/run.out`
- `turn4-artifacts/instrumented-rerun/serial.log`
- `turn4-artifacts/instrumented-rerun/vm-name.txt`

Result: healthy and attribution data produced. The VM rendered past the required window and was stopped/deleted.

Final captured `freeze-watch`:

```text
uptime_ms=270470 submits=145085 completes=145087 fails=0 last_completion_ms=270470 fps_last_5s=179
```

Other instrumented checks:

- `SOFT LOCKUP` / panic markers: 0
- `[SCHED] queue_empty rescue_tid=` log markers: 5
- Final rescue attribution counter:

```text
[rescue-attrib] dropped=28 isr_lost=0 wake_no_enq=0 other=0 inline=28 timer=0 total=28
```

Final wake/enqueue attribution counters:

```text
[wake-attrib] schedule=108907 unblock=0 isr_unblock=131620 wake_io=184865 signal=0 child=0 timer=19767
[enqueue-attrib] same_lock=72587 deferred=122450 isr_buf=131620 deferred_drained=114189 isr_buf_drained=118502 already_queued=8169 isr_buf_full=0
```

Final GPU lock attribution:

```text
[gpu-pci-lock-attrib] max_hold_ms=14 max_hold_holder_tid=13 rescues=28
```

The visible rescue log only prints the first few queue-empty rescues by design. The memory counter is the authoritative total for this run.

## E. Status and Turn 5 Scope

Status: `COMPLETE`.

Turn 4 restored a healthy runtime environment and invalidated the empty-ext2 baseline. With userspace fixed, the Turn 2 attribution instrumentation is not a render-killing perturbation.

The primary orphan classification is `dropped=28`, which maps to `READY_SITE_SCHEDULE`: a runnable outgoing thread is published `Ready` by the scheduler, but the deferred requeue path does not always make it reachable from a per-CPU ready queue before the queue-empty scan finds it. The rescue was inline (`inline=28`, `timer=0`), and the recurring rescued thread in the visible markers was `tid=13`.

Turn 5 should focus on the deferred requeue handoff for runnable outgoing threads:

- `schedule_deferred_requeue()` publishes the outgoing thread `Ready` and records `should_requeue_old`.
- The post-save path must call `requeue_thread_after_save()` exactly once for that thread after `commit_cpu_state_after_save()`.
- The fix should eliminate the `READY_SITE_SCHEDULE` orphan rather than broadening the rescue path.
