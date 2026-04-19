# F32 WaitQueue Factory Exit

Date: 2026-04-18
Branch: `f32-waitqueue-compositor`
Base: `dedbbb88`

## Result

F32 did not pass the Phase 4 validation gate. The branch contains the design,
waitqueue primitive, and compositor migration commits, but it must not be
merged.

Validation stopped after the first rebuilt 120-second Parallels run because the
boot reached BWM and bsshd, then stalled while spawning `/bin/bounce`. This
violates the hard lifecycle requirement for bounce and leaves no valid FPS or
CPU0 end-state audit sample.

## Commits Completed

| Phase | Commit | Status |
| --- | --- | --- |
| Phase 1 design | `f7fcc191 design(kernel): F32 waitqueue design document` | Complete |
| Phase 2 primitive | `653d7253 feat(kernel): F32 WaitQueueHead primitive` | Builds clean |
| Phase 3 migration | `8a2e9885 feat(gui): F32 migrate compositor wait to waitqueue` | Builds clean, validation failed |
| Phase 5 optional migration | none | Deferred |

## Design Excerpt

From `docs/planning/f32-waitqueue/design.md`:

> F32 replaces the compositor's ad-hoc waiting protocol with a scheduler-integrated
> waitqueue primitive. The immediate target is `compositor_wait` in
> `kernel/src/syscall/graphics.rs`, which currently publishes
> `COMPOSITOR_WAITING_THREAD`, blocks with `BlockedOnTimer`, and relies on a 5 ms
> fallback timer when an event wake is missed.

The implemented primitive followed the design direction:

- `WaitQueueHead` stores duplicate-free waiter TIDs behind a spin lock.
- `prepare_to_wait(ThreadState::BlockedOnIO)` enrolls the current thread and
  uses the scheduler's existing `BlockedOnIO` state.
- `wake_up` and `wake_up_one` route through F16's `isr_unblock_for_io`.
- `schedule_current_wait` uses the scheduler wait path instead of adding a
  polling timer.

## Migration Table

| Path | Before | F32 branch change |
| --- | --- | --- |
| BWM `compositor_wait` op 23 | `COMPOSITOR_WAITING_THREAD` plus timer-backed compositor block | `COMPOSITOR_FRAME_WQ.prepare_to_wait`, condition recheck, scheduler wait, `finish_wait` |
| Dirty-window wake | Direct thread ID signal | `COMPOSITOR_FRAME_WQ.wake_up()` |
| Registry/input/cleanup wakes | Direct compositor signal | `COMPOSITOR_FRAME_WQ.wake_up()` |
| Client frame pacing op 15 | `block_current_for_compositor` with 5 ms fallback | `CLIENT_FRAME_WQ` wait until BWM consumes/presents the frame |
| VirGL present path | No client waitqueue notification | Wakes `CLIENT_FRAME_WQ` after successful `virgl_composite_frame` when a waiting frame was sampled |
| `Completion` | Independent single-shot wait primitive | Not migrated |

## Validation

Clean builds completed before the Parallels gate:

| Command | Result |
| --- | --- |
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | Pass, no warnings/errors |
| `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | Pass, no warnings/errors |
| `userspace/programs/build.sh --arch aarch64` | Pass |

120-second Parallels sweep:

| Run | Boot marker | bsshd | bounce | CPU0 tick audit | AHCI timeout | FPS >= 160 | Render verdict | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Pass | Pass | Fail: serial stops at `spawn path='/bin/bounce'` | Missing | None seen | Missing, only frames 0-1 logged | Pass | Fail |
| 2 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 1 failure |
| 3 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 1 failure |
| 4 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 1 failure |
| 5 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 1 failure |

Artifacts:

- Serial log: `.factory-runs/f32-waitqueue-20260418-115626/rebuilt-run1.serial.log`
- Screenshot: `.factory-runs/f32-waitqueue-20260418-115626/rebuilt-run1.png`
- Render verdict: `.factory-runs/f32-waitqueue-20260418-115626/rebuilt-run1.verdict.txt`

Relevant serial tail:

```text
[init] Boot script completed
[spawn] path='/bin/bsshd'
bsshd: starting on port 2222
bsshd: listening on 0.0.0.0:2222
[init] bsshd started (PID 4)
[spawn] path='/bin/bounce'
```

There were no `SOFT LOCKUP`, `AHCI TIMEOUT`, `DATA_ABORT`, `panic`, or `PANIC`
markers in the captured serial log. The failure signature is a forward-progress
stall before bounce process creation logs begin.

## PR

No PR was opened and no merge was attempted. Phase 4 failed on run 1/5, so the
factory stop condition applied.

## Next Investigation

The next pass should start from the run 1 stall rather than from FPS or render
quality. Suggested first checks:

- Use GDB on the rebuilt branch and break around the `/bin/bounce`
  `create_process_with_argv` path to identify which CPU/thread is stuck after
  init prints `spawn path='/bin/bounce'`.
- Inspect whether BWM is sleeping in `COMPOSITOR_FRAME_WQ` while init is waiting
  on filesystem/process creation, or whether the waitqueue lock/scheduler lock
  order is blocking a later syscall path.
- Verify the client frame wait migration did not introduce a dependency where
  init can block behind a compositor/client wake that never arrives.
- Keep the no-polling constraint: do not restore the 5 ms fallback timer.

## Self-Audit

- No polling fallback was added.
- No Tier 1 prohibited files were modified.
- F16 `isr_unblock_for_io` remains the wake delivery mechanism.
- F1-F30 commits were not reverted.
- Completion migration was deferred to avoid expanding the blast radius after
  the failed gate.
