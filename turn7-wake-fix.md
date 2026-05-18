# Turn 7 Wake-Side Fix Attempt

## A. Source Diff

Attempted change in `kernel/src/syscall/graphics.rs`, inside
`handle_compositor_wait` before the first `compositor_ready_bits()` check:

```rust
    loop {
        // WHY: repair the read-but-not-released wedge captured in
        // turn3-wedge-snapshot.md before BWM sleeps on the compositor latch.
        wake_presented_client_frames();

        let (ready, cur_reg_gen, mouse_packed) =
            compositor_ready_bits(last_registry_gen, prev_mouse);
```

The patch added one executable line plus a two-line WHY comment. It added no
logging, no new helper, and no public API. The patch was reverted after Run 1
failed, so this branch does not contain the failed kernel change.

## B. Build Matrix

| Gate | Command | Outcome |
| --- | --- | --- |
| aarch64 kernel | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS, zero warnings/errors |
| qemu-uefi | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS, zero warnings/errors |
| warning grep | `grep -E "^(warning|error)" /tmp/breenix-turn7-kernel-build.log /tmp/breenix-turn7-qemu-uefi-build.log` | no output |
| diff whitespace | `git diff --check` | PASS |

## C. Validation Table

Run 1 used `turn7-artifacts/run_bwm_softlock_capture.sh mode=build
run=reproduce-run1`.

| Run | Classification | max_frame | max_uptime_ms | stuck_tid13_count | softlock_count | far_0xccd_count | panic_count | scheduler_lock | process_lock | gpu_pci_lock | ahci_irq |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | `softlock-leading-edge` | 47500 | 288601 | 5 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |

Run 1 reached healthy BWM progress for a long window, including
`[virgl-composite] Frame #47500`, then hit the pass-failing leading-edge marker:

```text
[SCHED] queue_empty stuck_tid=13 count=0
[SCHED] queue_empty stuck_tid=13 count=1
[SCHED] queue_empty stuck_tid=13 count=2
[SCHED] queue_empty stuck_tid=13 count=3
[SCHED] queue_empty stuck_tid=13 count=4
```

There was no `SOFT LOCKUP DETECTED`, no CPU0 timer regression panic, no
`FAR=0xccd`, and no data abort before capture. Per directive, the first
`stuck_tid=13` marker was a Run 1 failure, so no stress runs were started.

Endpoint GDB state:

```text
pc=kernel::drivers::virtio::gpu_pci::send_command
COMPOSITOR_DIRTY_WAKE.byte=0
COMPOSITOR_FRAME_WQ.logical_waiters=[]
CLIENT_FRAME_WQ.logical_waiters=[16]
window[00] generation=47793 last_uploaded_gen=0 last_read_gen=47793
pending=True waiting_thread_id=Some(16)
cpu_state[0] current=Some(13)
ready queues all empty
scheduler_lock_byte=0 process_manager_lock_byte=0 gpu_pci_lock_byte=1 ahci_irq=0
```

## D. Verdict

**E3: Run 1 FAIL.**

The wake-side repair did not satisfy the Run 1 pass criteria because the
original `stuck_tid=13` leading-edge marker returned. The patch avoided the
Turn 4/5 CPU0 timer-regression failure mode, but it did not clear the underlying
client-frame wait shape: endpoint state still had `last_read_gen == generation`
and `waiting_thread_id=Some(16)` with TID 16 on `CLIENT_FRAME_WQ`.

One important nuance: the endpoint PC was `send_command`, not the
`handle_compositor_wait` loop. At the capture moment BWM was current on CPU0 and
holding the GPU lock in an in-flight direct present, while the window registry
already had the familiar read-but-waiting state. That means the Turn 6 repair
point was not sufficient: the stale waiter can exist while BWM is still inside
op10/send_command, before it has an opportunity to re-enter op23 and run the new
repair call.

## E. Comparison To Turn 4/5 Failure

| Field | Turn 4 broad predicate | Turn 5 upload predicate | Turn 7 wake-side call |
| --- | --- | --- | --- |
| Source strategy | persistent ready on upload-or-waiter | persistent ready on upload-pending | repair read waiters at op23 wait entry |
| Classification | `non-softlock-failure-marker` | `no-softlock-within-window` with panic markers | `softlock-leading-edge` |
| CPU0 timer regression | yes | yes | no |
| max_frame | 2 | 2 | 47500 |
| stuck_tid13_count | 0 | 0 | 5 |
| softlock_count | 0 | 0 | 0 |
| panic_count | 14 | 2 | 0 |
| Endpoint window state | `generation=53 last_read_gen=52 waiter=Some(16)` | `generation=70 last_read_gen=70 waiter=Some(16)` | `generation=47793 last_read_gen=47793 waiter=Some(16)` |
| BWM endpoint | current on CPU0 | current on CPU0 | current on CPU0 in `send_command` |

Turn 7 is materially better than Turn 4/5 for scheduler/timer health, but it
does not fix the wedge. The result weakens the hypothesis that repairing only at
op23 wait entry is enough; the failing state can be observed while BWM is still
inside the direct present path.

## F. Proposed Next Step

The next turn should pivot from op23 wait-entry repair to the direct op10
completion path. The source fact to test is whether the client waiter should be
cleared before or during the direct present, rather than only after
`virgl_composite_frame()` returns `Ok(())`.

The most focused next investigation is GDB-based, not another blind patch:

- Break or instrument nonintrusively around `wake_presented_client_frames()` and
  op10's `virgl_composite_frame()` return path.
- Capture whether BWM is stuck inside `send_command` for a single direct-present
  command while `last_read_gen == generation` and TID 16 is blocked.
- If confirmed, the candidate fix is probably in the op10 direct-present path:
  clear/release consumed client waiters before the potentially long
  TRANSFER/FLUSH sequence, or split "client buffer consumed" from "GPU present
  completed" with a separate state transition.

Do not re-apply the Turn 7 op23-only patch without additional evidence; it did
not execute early enough to repair the captured failure.
