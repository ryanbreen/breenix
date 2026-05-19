# Turn 3 Wedge Snapshot

## A. Branch + Reproduction Outcome

- Branch: `fix/compositor-wait-softlock`.
- Starting commit: `87fccb91 docs: add compositor-wait turn2 GDB snapshot`.
- Kernel source edits: none.
- Harness: `turn3-artifacts/run_bwm_softlock_capture.sh`.
- GDB script: `turn3-artifacts/gdb_softlock_capture_v2.gdb`.
- Run directory: `turn3-artifacts/reproduce-run1/`.

Outcome: `COMPLETE`. The wedge reproduced.

```text
classification=softlock-leading-edge
run_rc=still-running-at-capture
gdb_rc=0
vm=breenix-1779142410
max_frame=26000
max_uptime_ms=191585
stuck_tid13_count=14
softlock_count=12
far_0xccd_count=0
panic_count=0
data_abort_count=0
scheduler_lock_byte=0
process_manager_lock_byte=0
gpu_pci_lock_byte=1
ahci_irq=0
```

The first leading-edge marker in `window.serial.log` came after `Frame #13500` and after the last pre-wedge freeze-watch sample at `uptime_ms=95368`:

```text
[SCHED] queue_empty stuck_tid=13 count=0
[SCHED] queue_empty stuck_tid=13 count=1
[SCHED] queue_empty stuck_tid=13 count=2
[SCHED] queue_empty stuck_tid=13 count=3
[SCHED] queue_empty stuck_tid=13 count=4
[SCHED] queue_empty stuck_tid=13 count=1000
!!! SOFT LOCKUP DETECTED !!!
```

The GDB v2 script collected the required decoded sections, then hit a post-capture Python scope error before optional raw text dumps. The script is fixed in-tree for future use; the captured compositor, scheduler, waitqueue, window, and lock sections are intact.

## B. Side-by-Side State Diff

| Field | Turn 2 loaded/no-wedge | Turn 3 wedge |
| --- | --- | --- |
| `COMPOSITOR_DIRTY_WAKE.byte` | `0` | `0` |
| `COMPOSITOR_FRAME_WQ` | `len=0`, waiters `[]`, stale ring `[13, 13, 13, 13]` | `len=0`, waiters `[]`, stale ring `[13, 13, 13, 13]` |
| `CLIENT_FRAME_WQ` | `len=1`, waiters `[16]` | `len=1`, waiters `[16]` |
| Window slot 0 | `id=1`, `generation=27562`, `last_uploaded_gen=0`, `waiting_thread_id=Some(16)`, `pending=True` | `id=1`, `generation=25944`, `last_uploaded_gen=0`, `waiting_thread_id=Some(16)`, `pending=True` |
| Other window slots | Not active | Slots 1-15 empty/none |
| BWM TID 13 scheduler location | `current_cpus=[1]`, `ready_queues=[]`, `deferred_cpus=[]` | `current_cpus=[2]`, `ready_queues=[]`, `deferred_requeue=[0,0,0,0,0,0,0,0]` |
| BWM saved frame | Not a softlock; GDB raw thread data only | Serial dump: `state=R bis inl`, `saved_lr=0xffff0000400e6708`, `saved_sp=0xffff0000542547d0` |
| TID 16 state | `CLIENT_FRAME_WQ` waiter, not current, not queued | `CLIENT_FRAME_WQ` waiter; serial dump: `state=? bis inl`, `saved_lr=0xffff0000400e7610` |
| `scheduler_lock_byte` | `0` | `0` |
| `process_manager_lock_byte` | `0` | `0` |
| `gpu_pci_lock_byte` | `0` | `1` |
| `need_resched_byte` | `0` | `0` |
| `cpu_is_idle` | `[1,0,1,1,1,1,1,1]` | `[1,1,0,1,1,1,1,1]` |

The domain state is the same shape in both captures: one client frame is pending forever from the registry's point of view, and the compositor dirty latch is already false. The wedge-specific difference is that BWM is now in the compositor wait softlock signature while the GPU lock is held.

Turn 3 GDB compositor-domain quote:

```text
COMPOSITOR_DIRTY_WAKE.byte=0
COMPOSITOR_FRAME_WQ.lock_byte=0 cap=4 ptr=0xffff0000502e1e60 head=2 len=0
COMPOSITOR_FRAME_WQ.logical_waiters=[]
CLIENT_FRAME_WQ.lock_byte=0 cap=4 ptr=0xffff0000502e1fc8 head=2 len=1
CLIENT_FRAME_WQ.logical_waiters=[16]
window[00] ... id=1 buffer_id=1 owner=6 ... registered=1 resource=0 virgl_init=0
generation=25944 last_uploaded_gen=0 last_read_gen=25944 pending=True
waiting_thread_id=Some(16)
```

Turn 3 scheduler quote:

```text
cpu_state[2] current=Some(13) previous=None idle_tid=5
ready_queue[0] ... len=0 ids=[]
ready_queue[1] ... len=0 ids=[]
ready_queue[2] ... len=0 ids=[]
ready_queue[3] ... len=0 ids=[]
ready_queue[4] ... len=0 ids=[]
ready_queue[5] ... len=0 ids=[]
ready_queue[6] ... len=0 ids=[]
ready_queue[7] ... len=0 ids=[]
tid_location tid=13 current_cpus=[2] previous_cpus=[] ready_queues=[]
tid_location tid=16 current_cpus=[] previous_cpus=[] ready_queues=[]
deferred_requeue=[0, 0, 0, 0, 0, 0, 0, 0]
```

Softlock serial quote:

```text
tid=13 state=R bis inl user pid=3 elr=0xffff0000401d31dc
x30=0xffff0000401d4124 sp=0xffff0000542547d0
saved_lr=0xffff0000400e6708 saved_sp=0xffff0000542547d0

tid=16 state=? bis inl user pid=6 elr=0xffff0000401d4124
x30=0xffff0000401d4124 sp=0xffff0000542877d0
saved_lr=0xffff0000400e7610 saved_sp=0xffff0000542877d0
```

## C. Hypothesis Verdict

| Hypothesis | Verdict | Decisive evidence |
| --- | --- | --- |
| H1: producer/consumer circular wait between BWM and a client | SUPPORTED | TID 16 is the sole `CLIENT_FRAME_WQ` waiter, the only active window has `waiting_thread_id=Some(16)` and `generation > last_uploaded_gen`, and BWM TID 13 is in the compositor wait softlock path. |
| H2: dirty-wake latch loses persistent frame readiness | SUPPORTED | At the wedge, `COMPOSITOR_DIRTY_WAKE.byte=0` while window slot 0 is still pending: `generation=25944`, `last_uploaded_gen=0`, `waiting_thread_id=Some(16)`. The compositor wait readiness predicate can therefore sleep despite persistent dirty work. |
| H3: ready-but-inline waiter is not requeued or resumed | REFUTED as primary cause | GDB does not show TID 13 ready-but-lost. It is `current=Some(13)` on CPU 2, all runqueues are empty, and active deferred requeue is empty. The serial still shows `state=R bis inl`, but scheduler ownership is "current", not missing from every CPU. |
| H5: preemption or need-resched state wrong around `schedule_current_wait` | INCONCLUSIVE / not primary | `need_resched_byte=0` and CPU 2 is non-idle with TID 13 current. That is compatible with the observed stuck current thread but does not identify a preempt-depth bug. |

The strongest hypothesis is H2, with H1 as the consequence: the compositor wait path uses an edge-triggered dirty latch, while client frame pacing depends on a persistent registry condition being consumed.

## D. Source Pointer

Primary source pointer: `kernel/src/syscall/graphics.rs:187-197`.

```rust
fn compositor_ready_bits(last_registry_gen: u64, prev_mouse: u64) -> (u64, u64, u64) {
    ...
    let mut ready = 0u64;
    if COMPOSITOR_DIRTY_WAKE.swap(false, Ordering::Relaxed) {
        ready |= 1;
    }
    ...
}
```

The bug is that dirty-window readiness is represented only by the consumable `COMPOSITOR_DIRTY_WAKE` latch. `compositor_ready_bits` does not also test the persistent condition in `WINDOW_REGISTRY`: a registered window with `generation > last_uploaded_gen` or a `waiting_thread_id`.

The producer side is `kernel/src/syscall/graphics.rs:1023-1037`: `mark_window_dirty` increments `generation`, records `waiting_thread_id`, sets the dirty latch, and wakes `COMPOSITOR_FRAME_WQ`.

The consumer/wake side is `kernel/src/syscall/graphics.rs:1492-1545` and `1595-1599`: `handle_composite_windows` notices `generation > last_uploaded_gen`, sets `last_uploaded_gen = generation`, takes `waiting_thread_id`, and wakes `CLIENT_FRAME_WQ` after GPU work.

The captured wedge proves the consumer side has not run for the dirty window: `last_uploaded_gen` is still `0` while `generation` is `25944`.

## E. Proposed Turn 4 Scope

Turn 4 should be a compositor readiness fix attempt, not more scheduler forensics.

Implement a persistent dirty-window predicate in `kernel/src/syscall/graphics.rs` and include it in `compositor_ready_bits`, so BWM cannot block in `handle_compositor_wait` while any registered window has pending dirty work or a client frame waiter. Keep the fix local to the syscall graphics layer. Then verify with the boot-stage/build gate and the requested Parallels stress gate in the goal contract.
