# F28 Phase 3 - Wake Race Fix

Date: 2026-04-18

## Root Cause

F26 fixed the client-side ordering in `mark_window_dirty`: the client blocks before it wakes bwm. F28 found the symmetric compositor-side race in `compositor_wait`:

1. `compositor_wait` checked `COMPOSITOR_DIRTY_WAKE` and found no work.
2. It stored its thread ID in `COMPOSITOR_WAITING_THREAD` while it was still running.
3. `mark_window_dirty` set `COMPOSITOR_DIRTY_WAKE`, saw the published compositor TID, and called `unblock()`.
4. Because the compositor thread was not actually blocked yet, `unblock()` had no state transition to perform.
5. `compositor_wait` then blocked and slept until its timeout even though the dirty wake had already arrived.

That explains the Phase 2 counter result: bwm did not reliably wake from the client dirty signal, so clients resumed via the 5 ms back-pressure fallback.

## Fix

`kernel/src/syscall/graphics.rs` now:

- Calls `block_current_for_compositor()` before publishing `COMPOSITOR_WAITING_THREAD`.
- Publishes the waiter only after the scheduler marks the compositor thread blocked.
- Immediately re-checks dirty, mouse, and registry readiness after publishing the waiter.
- If the re-check finds pending work, it clears the waiter, unblocks the still-running syscall thread back to `Ready`, updates wake bookkeeping, and returns without waiting for the timeout.

This closes both windows:

- Dirty wake before waiter publication: the post-publish re-check observes the pending dirty bit.
- Dirty wake after waiter publication: the thread is already blocked, so `unblock()` can transition it to ready.

## Validation

Command:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f28-m3-kernel-build.log
grep -E '^(warning|error)' /tmp/f28-m3-kernel-build.log
```

Result: build exited 0, and the warning/error grep produced no output.

After the first final validation run showed the GUI and FPS were healthy but emitted no `[gfx-wake]` lines, the instrumentation was tightened to dump every 1000 counted wake completions. Follow-up command:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f28-m3b-kernel-build.log
grep -E '^(warning|error)' /tmp/f28-m3b-kernel-build.log
```

Result: build exited 0, and the warning/error grep produced no output.
