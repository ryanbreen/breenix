# Turn 6 Path Trace

## A. BWM workload syscall sequence

The ARM64 BWM workload does not use the op16 multi-window upload path. The
documented guardrail is explicit: on ARM64, BWM intentionally uses
`graphics::virgl_composite(composite_buf, ...)`, which is op10, because the
normal op16 VirGL compositor path times out on Parallels
(`docs/planning/f24-apps-on-desktop/phase1-constraints.md:19,28`).

The direct workload sequence is:

1. BWM discovers windows with op13 and maps each client buffer with op21.
   `discover_windows()` calls `graphics::map_window_buffer(info.buffer_id)` and
   stores the mapped pointer in each `Window`
   (`userspace/programs/src/bwm.rs:1138-1155`).
2. The client writes pixels and calls op15 `mark_window_dirty`.
   `handle_virgl_op` op15 increments `generation`, stores
   `waiting_thread_id`, sets `COMPOSITOR_DIRTY_WAKE`, and wakes
   `COMPOSITOR_FRAME_WQ` (`kernel/src/syscall/graphics.rs:1018-1037`).
3. BWM blocks in op23 `compositor_wait` when it has no local work. The wait
   path returns bit 0 when `COMPOSITOR_DIRTY_WAKE.swap(false, ...)` succeeds
   (`kernel/src/syscall/graphics.rs:187-197`, `1303-1358`).
4. BWM converts bit 0 into local `windows_dirty = true`
   (`userspace/programs/src/bwm.rs:2160-2162`).
5. In the ARM64 present block, BWM calls `blit_window_contents()`. That helper
   calls op22 `check_window_dirty` for each mapped window, ignores the boolean
   return, then copies from the mapped client pages into `composite_buf`
   (`userspace/programs/src/bwm.rs:1188-1224`, `2201-2215`).
6. op22 advances `last_read_gen` when it observes a new `generation`; it does
   not wake the client and does not touch `last_uploaded_gen`
   (`kernel/src/syscall/graphics.rs:1213-1229`).
7. BWM presents the CPU-composited buffer with op10
   `graphics::virgl_composite(composite_buf, ...)`
   (`userspace/programs/src/bwm.rs:2224`).
8. On ARM64 VirGL success, op10 calls
   `virtio::gpu_pci::virgl_composite_frame(...)`; after that returns `Ok(())`,
   it calls `wake_presented_client_frames()`
   (`kernel/src/syscall/graphics.rs:807-815`). The driver helper copies the
   BWM buffer into the 3D framebuffer backing, sends TRANSFER_TO_HOST_3D, then
   SET_SCANOUT and RESOURCE_FLUSH (`kernel/src/drivers/virtio/gpu_pci.rs:4588-4704`).

The step-3 wake for blocked clients is `wake_presented_client_frames()`. It
scans window buffers and clears `waiting_thread_id` only when
`last_read_gen == generation`, then wakes `CLIENT_FRAME_WQ`
(`kernel/src/syscall/graphics.rs:165-183`).

The direct path therefore has two separate progress markers:

- BWM consumed the client's mapped pixels: op22 advanced `last_read_gen`.
- BWM completed a direct present and released client back-pressure: op10 success
  called `wake_presented_client_frames()`, which cleared `waiting_thread_id`.

There is no direct-path setter for `last_uploaded_gen`.

## B. Window-state field semantics

`generation`

- Initialized to `1` on buffer allocation
  (`kernel/src/syscall/graphics.rs:450`).
- Incremented by op15 `mark_window_dirty`
  (`kernel/src/syscall/graphics.rs:1023`).
- Incremented by resize op24 when the backing pages change
  (`kernel/src/syscall/graphics.rs:1993-2006`).

`last_read_gen`

- Initialized to `0` on buffer allocation
  (`kernel/src/syscall/graphics.rs:452`).
- Advanced by op14 `read_window_buffer` after it decides a copy is needed
  (`kernel/src/syscall/graphics.rs:970-978`).
- Advanced by op22 `check_window_dirty` in the MAP_SHARED direct path
  (`kernel/src/syscall/graphics.rs:1217-1223`).
- This is the direct path's "BWM has consumed this client generation" marker.

`last_uploaded_gen`

- Initialized to `0` on buffer allocation
  (`kernel/src/syscall/graphics.rs:451`).
- Set only inside op16 `handle_composite_windows` when that path decides a
  window is dirty (`kernel/src/syscall/graphics.rs:1492`, `1541-1545`).
- Not set by op10, op21, op22, or `virgl_composite_frame`.

`pending`

- There is no `pending` field in `WindowBuffer`
  (`kernel/src/syscall/graphics.rs:321-368`).
- The Turn 3 GDB `pending=True` label was a derived diagnostic, effectively
  `generation > last_uploaded_gen`. That is valid for op16 upload-pending
  analysis, but it is misleading for the ARM64 direct path because
  `last_uploaded_gen` is not maintained there.

`waiting_thread_id`

- Initialized to `None` on buffer allocation
  (`kernel/src/syscall/graphics.rs:454`).
- Set by op15 to the current client thread before the compositor is woken
  (`kernel/src/syscall/graphics.rs:1012-1025`).
- Used by `window_frame_pending()` as the client's blocking condition
  (`kernel/src/syscall/graphics.rs:137-142`, `1040-1055`).
- Cleared by op16 with `buf.waiting_thread_id.take()` when
  `generation > last_uploaded_gen` (`kernel/src/syscall/graphics.rs:1541-1545`).
- Cleared by `wake_presented_client_frames()` in the direct op10 path when
  `waiting_thread_id.is_some() && last_read_gen == generation`
  (`kernel/src/syscall/graphics.rs:165-183`).

`COMPOSITOR_DIRTY_WAKE`

- Set only by op15 `mark_window_dirty`
  (`kernel/src/syscall/graphics.rs:1034-1037`).
- Consumed only by `compositor_ready_bits()` with `swap(false, ...)`
  (`kernel/src/syscall/graphics.rs:187-197`).
- It is an edge latch, not a persistent "dirty work exists" field.

## C. Why Turn 4/5 fixes broke

Turn 4 made any `waiting_thread_id.is_some()` frame a persistent compositor
readiness signal. That was too broad: it can keep BWM runnable while the next
required transition is not "read another dirty bit"; the failed run showed BWM
current on CPU0 with `CLIENT_FRAME_WQ` still holding TID 16.

Turn 5 narrowed the predicate to `generation > last_uploaded_gen`, but source
tracing proves that field is permanently stale in the ARM64 direct workload.
`last_uploaded_gen` is set only in op16 (`kernel/src/syscall/graphics.rs:1542`).
BWM's ARM64 path uses op22 plus op10 (`userspace/programs/src/bwm.rs:1217`,
`2224`), and op10's `virgl_composite_frame` does not update window registry
upload state (`kernel/src/drivers/virtio/gpu_pci.rs:4588-4704`).

That explains the Turn 5 endpoint: `generation=70 last_uploaded_gen=0
last_read_gen=70 waiting_thread_id=Some(16)`. The frame was read by BWM, but
the op16 upload marker could never catch up.

The correct direct-path persistent state is not upload-pending. The Turn 3
wedge state is a read-but-not-released client frame:

```text
generation=25944
last_read_gen=25944
last_uploaded_gen=0
waiting_thread_id=Some(16)
COMPOSITOR_DIRTY_WAKE=0
CLIENT_FRAME_WQ=[16]
```

That means op22 consumed the client generation, but the wake side that should
clear `waiting_thread_id` did not run or did not take effect before BWM
re-entered op23.

## D. Recommended fix-site: Option B, wake-side repair

Recommendation: Option B. Fix the direct-path wake side, not the
`compositor_ready_bits` predicate and not the dirty latch protocol.

Candidate fix-site: `kernel/src/syscall/graphics.rs:1321`, at the top of the
`handle_compositor_wait` loop, before the readiness check can decide to sleep.

Small change:

```rust
    loop {
        // Direct-mapped BWM advances last_read_gen with op22 before it presents
        // with op10. If BWM re-enters compositor_wait with that consumed frame
        // still holding a client waiter, repair the missed direct-present wake
        // before sleeping on the edge-triggered compositor latch.
        wake_presented_client_frames();

        let (ready, cur_reg_gen, mouse_packed) =
            compositor_ready_bits(last_registry_gen, prev_mouse);
        ...
    }
```

Rationale:

- The helper's existing predicate exactly matches the Turn 3 bad state:
  `waiting_thread_id.is_some() && last_read_gen == generation`
  (`kernel/src/syscall/graphics.rs:174-175`).
- Calling it from op23 does not make BWM busy-loop. It clears the client waiter
  and wakes `CLIENT_FRAME_WQ`; if no compositor work remains, BWM can then sleep.
- It does not depend on `last_uploaded_gen`, which is op16-only state.
- It does not treat every parked client as BWM readiness, which was the Turn 4
  failure mode.
- It covers the source-level missing edge: once BWM has already consumed pixels
  with op22 and nevertheless reaches `compositor_wait`, the client release edge
  must be repaired before BWM waits on `COMPOSITOR_FRAME_WQ`.

I would not put this in `compositor_ready_bits()`. That helper currently consumes
the dirty latch, but the repair is a wake/clear side effect on
`CLIENT_FRAME_WQ`; placing it directly in `handle_compositor_wait` makes the
state transition explicit and avoids presenting it as another readiness bit.

## E. Open questions / risks

- Source tracing cannot prove why the normal op10 success wake was missed in the
  Turn 3 run. The trace buffer contained status-0 BWM composite exits, but the
  final registry state still had a read generation and a client waiter. The
  proposed fix repairs that state when BWM next enters op23; it does not explain
  which exact interleaving produced it.
- `wake_presented_client_frames()` is named around "presented", but this repair
  effectively treats "BWM already consumed the mapped client generation and is
  now trying to wait again" as sufficient to release the client. That is safe for
  the client buffer because op22 is only called inside BWM's blit path before
  copying from the mapped pages, and BWM cannot enter op23 until that userland
  block has returned. It is still a semantic shift from strict GPU-presented
  frame pacing to consumed-frame pacing on this recovery edge.
- If there is a deeper scheduler bug causing `wake_presented_client_frames()`
  itself to run but not clear the registry entry, this repair will not be
  sufficient. The captured registry state argues the helper did not clear the
  waiter after the final op22 consumption, but it does not distinguish "not
  called" from "called too early".
- If the next validation reproduces a CPU0 timer regression, the fix may need
  an additional BWM-side change around `graphics::virgl_composite(...)` result
  handling so BWM does not clear local dirty flags after a failed direct present.

## F. Proposed Turn 7 scope

Implement the Option B wake-side repair in `handle_compositor_wait`, keeping the
change local to `kernel/src/syscall/graphics.rs` and without adding logging.
Then run the normal zero-warning build gate and one Parallels reproduction. If
the original softlock is gone and no CPU0 guard fires, continue to the 5-boot
stress gate required by the goal. If the run still leaves
`last_read_gen == generation` with `waiting_thread_id=Some(..)`, capture GDB at
the first recurrence and specifically distinguish whether op10 reached
`wake_presented_client_frames()`.
