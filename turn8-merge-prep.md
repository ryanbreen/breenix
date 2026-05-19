# Turn 8 Merge Prep

## A. Source Diff

Fix commit: `de8a603c fix(compositor-wait): wake released client frames before sleeping in handle_compositor_wait`.

The shipped source change is the Turn 7 wake-side repair in
`kernel/src/syscall/graphics.rs::handle_compositor_wait`:

```rust
    loop {
        // WHY: repair the read-but-not-released wedge captured in
        // turn3-wedge-snapshot.md before BWM sleeps on the compositor latch.
        wake_presented_client_frames();

        let (ready, cur_reg_gen, mouse_packed) =
            compositor_ready_bits(last_registry_gen, prev_mouse);
```

This calls the existing client-release helper once per compositor wait-loop
entry, before BWM evaluates whether to sleep on `COMPOSITOR_FRAME_WQ`.

## B. Validation History

| Turn | Strategy | Classification | max_frame | max_uptime_ms | softlock_count | panic_count | stuck_tid13_count | far_0xccd_count |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3 | Baseline capture | `softlock-leading-edge` | 26000 | 191585 | 12 | 0 | 14 | 0 |
| 4 | Broad predicate: upload pending or waiter present | `non-softlock-failure-marker` | 2 | 38289 | 0 | 14 | 0 | 0 |
| 5 | Narrow predicate: upload pending only | `no-softlock-within-window` | 2 | 50341 | 0 | 2 | 0 | 0 |
| 7 | Wake-side repair before compositor sleep | `softlock-leading-edge` | 47500 | 288601 | 0 | 0 | 5 | 0 |

Turn 7 is the accepted fix validation for this bug class:

- `softlock_count`: 12 -> 0
- `panic_count`: 0 -> 0
- `max_frame`: 26000 -> 47500
- `max_uptime_ms`: 191585 -> 288601
- `stuck_tid13_count`: 14 -> 5 leading-edge transients only

The Turn 7 run did not introduce the Turn 4/5 CPU0 timer-regression failure and
kept BWM rendering through frame 47500.

## C. Commit Hashes

Investigation branch commits from `main` (`980f12c3`) through the fix:

| Commit | Purpose |
| --- | --- |
| `103d880f` | Turn 1 forensics: compositor-wait source/disassembly read |
| `87fccb91` | Turn 2 GDB snapshot |
| `a75b8439` | Turn 3 wedge snapshot and baseline capture |
| `8e49867b` | Turn 4 broad-predicate fix attempt report |
| `5ccb9387` | Turn 5 refined upload-predicate fix attempt report |
| `2e23a5aa` | Turn 6 direct-compositor path trace |
| `ac07d70e` | Turn 7 wake-side fix attempt report and artifacts |
| `de8a603c` | Shipped fix: wake released client frames before compositor wait sleep |

Merge-prep doc commit follows this table.

## D. Known Residual

Turn 7 still showed 5 leading-edge `stuck_tid=13` transients in the active
window. That is down from 14 leading-edge markers plus 12 full softlock banners
in the Turn 3 baseline, and it appeared with BWM current inside
`kernel::drivers::virtio::gpu_pci::send_command` rather than wedged in
`handle_compositor_wait`.

Current interpretation: this residual is a separate long-syscall watchdog
classification, not the original compositor-wait producer/consumer wedge. BWM
can remain legitimately busy inside direct-present `send_command` long enough
for the scheduler watchdog's leading-edge queue-empty detector to flag TID 13.

Recommended follow-up, outside this investigation:

- Tune the watchdog stuck-threshold for long graphics syscalls; or
- Add a cooperative yield/progress point inside `send_command` after the
  compositor-wait repair is merged and stable.

Do not conflate that residual with the original waitqueue bug. The original bug
left BWM in the compositor wait loop with TID 16 stuck on `CLIENT_FRAME_WQ` and
full softlock banners; the shipped fix eliminated the full softlock banners in
the Turn 7 validation window.

## E. PR Body Draft

Title:

```text
fix(compositor-wait): wake released client frames before sleeping
```

Body:

```markdown
## Summary

- Repair the direct-compositor read-but-not-released wedge in
  `handle_compositor_wait`
- Call `wake_presented_client_frames()` before BWM sleeps on the compositor wait
  latch
- Preserve the existing op10/op22 direct-compositor state model without adding a
  persistent readiness predicate

## Root Cause

ARM64 BWM uses the direct compositor path: op21 `map_window_buffer`, op22
`check_window_dirty`, then op10 `virgl_composite`. The op16
`handle_composite_windows` path is not active, so `last_uploaded_gen` is dead
state for this workload.

The captured wedge was read-but-not-released:

```text
generation=25944
last_read_gen=25944
last_uploaded_gen=0
waiting_thread_id=Some(16)
CLIENT_FRAME_WQ=[16]
COMPOSITOR_DIRTY_WAKE=0
```

BWM had consumed the client pixels via op22, but the client release wake had not
cleared `waiting_thread_id`, allowing TID 16 to remain blocked.

## Fix

Call `wake_presented_client_frames()` at the top of the
`handle_compositor_wait` loop before the readiness check can decide to sleep.
The helper is idempotent: it only clears/wakes when
`waiting_thread_id.is_some() && last_read_gen == generation`.

## Validation

Build gates:

- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- warning/error grep: no output
- `git diff --check`

Parallels validation:

| Scenario | max_frame | max_uptime_ms | softlock_count | panic_count | stuck_tid13_count |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline Turn 3 | 26000 | 191585 | 12 | 0 | 14 |
| Wake-side Turn 7 | 47500 | 288601 | 0 | 0 | 5 |

Known residual: the remaining 5 `stuck_tid=13` markers are leading-edge
transients while BWM is busy in `send_command`, not full compositor-wait
softlocks. Follow-up should tune the watchdog threshold or add cooperative
progress inside long graphics syscalls.

## Investigation Trail

See:

- `turn1-compositor-wait-forensics.md`
- `turn2-gdb-snapshot.md`
- `turn3-wedge-snapshot.md`
- `turn4-fix-attempt.md`
- `turn5-refined-fix.md`
- `turn6-path-trace.md`
- `turn7-wake-fix.md`
- `turn8-merge-prep.md`
```
