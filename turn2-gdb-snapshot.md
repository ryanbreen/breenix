# Turn 2 GDB Snapshot

## A. Branch + Commit State

- Branch: `fix/compositor-wait-softlock`.
- Starting commit for the turn: `103d880f docs: add compositor-wait turn1 forensics`.
- Kernel source was not edited.
- One Parallels boot was run through `turn2-artifacts/run_bwm_softlock_capture.sh mode=build`.
- The build stage in `run.out` completed cleanly; no Rust compile warnings or errors were present.

## B. Harness Summary

Artifacts:

- `turn2-artifacts/run_bwm_softlock_capture.sh`
- `turn2-artifacts/gdb_softlock_capture.gdb`
- `turn2-artifacts/reproduce-run1/result.txt`
- `turn2-artifacts/reproduce-run1/run.out`
- `turn2-artifacts/reproduce-run1/gdb_softlock_state.out`
- Ignored local logs retained on disk: `turn2-artifacts/reproduce-run1/run.serial.log`, `window.serial.log`, `harness.log`, `tail160.log`

The harness was copied from the prior Parallels BWM window harness and narrowed to:

1. Build and boot with `./run.sh --parallels`.
2. Wait for BWM progress.
3. Stop at the first `[SCHED] queue_empty stuck_tid=13 count=` marker.
4. Attach Parallels `guest-debugger`.
5. Run `gdb_softlock_capture.gdb` and save the decoded state.

The GDB script uses fixed symbol addresses from `nm -nC target/aarch64-breenix/release/kernel-aarch64`. It decodes:

- `COMPOSITOR_DIRTY_WAKE`
- `COMPOSITOR_FRAME_WQ`
- `CLIENT_FRAME_WQ`
- `WINDOW_REGISTRY`
- `SCHEDULER` cpu state and per-CPU runqueues
- AArch64 deferred-requeue and inline-schedule state
- Lock/liveness bytes for scheduler, process manager, AHCI, and virtio-gpu

The GDB script succeeded. The shell harness had a classification bug: several grep patterns were double-escaped, so it failed to recognize BWM progress even though the serial log reached frame 27500. I fixed the harness file after the run, but did not rerun because the directive allowed one boot maximum.

## C. Reproduction Outcome

Status: `BLOCKED` for the requested softlock capture. The one boot did not produce a valid "first queue-empty" GDB snapshot.

Observed run result:

```text
classification=no-bwm-progress-within-window
run_rc=still-running-at-capture
gdb_rc=0
vm=breenix-1779138625
max_frame=27500
max_uptime_ms=172605
stuck_tid13_count=0
softlock_count=0
far_0xccd_count=0
panic_count=0
data_abort_count=0
scheduler_lock_byte=0
process_manager_lock_byte=0
gpu_pci_lock_byte=0
ahci_irq=0
```

The `classification=no-bwm-progress-within-window` line is incorrect. Post-run parsing of `run.serial.log` found:

- First compositor frame: line 365, `Frame #0`.
- First BWM FPS marker: line 425, `frames_since_last=123`.
- Last compositor frame before capture: line 886, `Frame #27500`.
- Last freeze-watch before capture: uptime `170379 ms`, `submits=82719`, `completes=82721`, `fps_last_5s=53`.
- `stuck_tid=13` lines: 0.
- `SOFT LOCKUP` lines: 0.

Because the harness stopped at 180 seconds after VM detection instead of 220 seconds after BWM progress, this run cannot satisfy the directive's `INCONCLUSIVE` criterion exactly. It is a harness-blocked turn with a useful non-softlock GDB snapshot.

## D. Compositor-Domain State Dump

GDB stopped in `idle_loop_arm64`, not in the softlock path. The compositor-domain decode was:

```text
COMPOSITOR_DIRTY_WAKE.byte=0

COMPOSITOR_FRAME_WQ.lock_byte=0 cap=4 ptr=0xffff0000502e1e80 head=0 len=0
COMPOSITOR_FRAME_WQ.logical_waiters=[]
COMPOSITOR_FRAME_WQ.raw_ring=[13, 13, 13, 13]

CLIENT_FRAME_WQ.lock_byte=0 cap=4 ptr=0xffff000053213f40 head=0 len=1
CLIENT_FRAME_WQ.logical_waiters=[16]
CLIENT_FRAME_WQ.raw_ring=[16, 16, 16, 16]
```

The raw ring entries for `COMPOSITOR_FRAME_WQ` are stale storage because `len=0`; they are not active waiters.

The decoded window registry had one active/nonempty slot:

```text
window[00] slot=0xffff00004022d2e0 tag=1 id=1 owner=6 size=480000
mapped=0x7ffffdf86000 width=400 height=300 x=32 y=72 z=0
registered=1 resource=0 virgl_init=0
generation=27562 last_uploaded_gen=0 last_read_gen=27562 pending=True
waiting_thread_id=Some(16) input_head=0 input_tail=0
window_registry.active_or_nonempty_slots=1
window_registry.next_id_candidate=2
```

This is the H2 predicate shape: `COMPOSITOR_DIRTY_WAKE=false` while a window has `generation > last_uploaded_gen` and `waiting_thread_id=Some(16)`. Because this was not a softlock snapshot and BWM was still running, it is not by itself proof that the predicate shape causes the wedge.

## E. Scheduler State for TID 13 + TID 16

The scheduler was unlocked and decoded cleanly:

```text
scheduler.lock_byte=0 data=0xffff00004022d008
cpu_state[0] current=Some(0) previous=None idle_tid=0
cpu_state[1] current=Some(13) previous=None idle_tid=4
cpu_state[2] current=Some(5) previous=None idle_tid=5
cpu_state[3] current=Some(6) previous=None idle_tid=6
cpu_state[4] current=Some(7) previous=None idle_tid=7
cpu_state[5] current=Some(8) previous=None idle_tid=8
cpu_state[6] current=Some(9) previous=None idle_tid=9
cpu_state[7] current=Some(10) previous=None idle_tid=10
```

Per-TID location summary:

```text
tid_location tid=13 current_cpus=[1] previous_cpus=[] ready_queues=[]
tid_location tid=16 current_cpus=[] previous_cpus=[] ready_queues=[]
tid_deferred_membership tid=13 deferred_cpus=[] inline_old_cpus=[0, 4, 6] inline_new_cpus=[1, 2, 3, 5]
tid_deferred_membership tid=16 deferred_cpus=[] inline_old_cpus=[1, 2, 3, 5, 7] inline_new_cpus=[0]
```

The inline old/new CPU lists are breadcrumb state, not active deferred ownership. The active deferred-requeue array was all zero.

## F. Per-CPU Dispatch State

Per-CPU static state:

```text
need_resched_byte=0 context_switch_count=193511
cpu_is_idle=[1, 0, 1, 1, 1, 1, 1, 1]
deferred_requeue=[0, 0, 0, 0, 0, 0, 0, 0]
```

All runqueues were empty:

```text
ready_queue[0] cap=4 ptr=0xffff0000502e0860 head=0 len=0 ids=[]
ready_queue[1] cap=4 ptr=0xffff0000502e0b70 head=2 len=0 ids=[]
ready_queue[2] cap=4 ptr=0xffff0000502e0d40 head=2 len=0 ids=[]
ready_queue[3] cap=4 ptr=0xffff0000502e1e40 head=0 len=0 ids=[]
ready_queue[4] cap=4 ptr=0xffff0000502e0d20 head=1 len=0 ids=[]
ready_queue[5] cap=4 ptr=0xffff0000502e0d70 head=3 len=0 ids=[]
ready_queue[6] cap=4 ptr=0xffff0000502e1ed0 head=1 len=0 ids=[]
ready_queue[7] cap=4 ptr=0xffff0000502e1fc8 head=2 len=0 ids=[]
```

Liveness sanity:

```text
scheduler_lock_byte=0 scheduler_word=0x0
process_manager_lock_byte=0 process_owner_cpu=0xffffffffffffffff process_owner_tid=0xffffffffffffffff
gpu_pci_lock_byte=0 gpu_pci_word=0x0
ahci_irq=0
```

This is a healthy non-softlock scheduling snapshot: BWM TID 13 is current on CPU 1, and CPU 1 is the only CPU marked non-idle.

## G. Hypothesis Verdict

| Hypothesis | Verdict from this capture | Reason |
| --- | --- | --- |
| H1: producer/consumer circular wait between BWM and a client | INCONCLUSIVE, with supporting condition present | TID 16 is in `CLIENT_FRAME_WQ`, and the window has `waiting_thread_id=Some(16)`. But BWM TID 13 is actively current on CPU 1, so this is not a circular wait snapshot. |
| H2: dirty-wake latch loses persistent frame readiness | INCONCLUSIVE, with the key predicate observed | `COMPOSITOR_DIRTY_WAKE.byte=0` while `generation=27562`, `last_uploaded_gen=0`, and `waiting_thread_id=Some(16)`. This exactly matches the predicate concern, but the system had not wedged. |
| H3: ready-but-inline waiter is not requeued or resumed | REFUTED for this capture only | TID 13 is not ready-but-lost. It is `current=Some(13)` on CPU 1, CPU 1 is non-idle, all ready queues are empty, and active deferred requeue is empty. |
| H5: preemption or need-resched state wrong around `schedule_current_wait` | INCONCLUSIVE | `need_resched_byte=0` with BWM current on a non-idle CPU is not suspicious by itself. No softlock or stuck ready state was captured. |

This capture does not decide the root cause because it missed the target condition.

## H. Best-Supported Hypothesis + Proposed Turn 3 Scope

Do not attempt a kernel fix from this turn. The right next scope is another forensics turn using the corrected harness, because the one allowed boot was consumed by a harness classification defect.

What the non-softlock snapshot did add:

- The H2 predicate is real in a live run: dirty wake can be false while a client window is pending and TID 16 is waiting on `CLIENT_FRAME_WQ`.
- That predicate alone is not sufficient to prove a wedge, because BWM was still current and making frames in this capture.
- The H3 ready-but-inline-lost shape was not present in this capture; a true softlock-edge snapshot is still needed.

Proposed Turn 3: run the corrected harness once, or an equivalent manual monitor, and attach GDB at the first real `[SCHED] queue_empty stuck_tid=13 count=` line. The same GDB script should be reused.
