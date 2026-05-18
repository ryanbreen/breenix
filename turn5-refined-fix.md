# Turn 5 Refined Fix

## A. Source Diff

Attempted change from the Turn 4 baseline:

```rust
186 #[cfg(target_arch = "aarch64")]
187 fn compositor_has_pending_window_frame() -> bool {
188     let reg = WINDOW_REGISTRY.lock();
189     reg.buffers.iter().any(|slot| {
190         if let Some(buf) = slot.as_ref() {
191             // WHY: H2 fix. Wake while registered window pixels are read but
192             // not yet uploaded; the Turn 3 wedge had generation=25944 and
193             // last_uploaded_gen=0. This is upload-pending only, not waiter-
194             // present, to avoid the Turn 4 CPU0 timer starvation loop.
195             buf.registered
196                 && buf.width > 0
197                 && buf.height > 0
198                 && buf.generation > buf.last_uploaded_gen
199         } else {
200             false
201         }
202     })
203 }
...
217     // WHY: H2 fix. The dirty-wake latch is consumable; persistent window
218     // registry state must also keep BWM awake while upload work remains.
219     // See turn3-wedge-snapshot.md for the captured lost-readiness wedge.
220     if compositor_has_pending_window_frame() {
221         ready |= 1;
222     }
```

The key difference from Turn 4 was removing `|| buf.waiting_thread_id.is_some()`. The attempted source patch was reverted after Run 1 failed; it is not committed as a fix.

## B. Build Matrix

| Gate | Command | Outcome |
| --- | --- | --- |
| aarch64 kernel | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS, zero warnings/errors |
| qemu-uefi | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS, zero warnings/errors |
| diff whitespace | `git diff --check` | PASS |
| warning grep | `grep -E "^(warning|error)" /tmp/breenix-turn5-kernel-build.log /tmp/breenix-turn5-qemu-uefi-build.log` | no output |

## C. Validation Table

Run 1 used `turn5-artifacts/run_bwm_softlock_capture.sh mode=build run=reproduce-run1`.

| Run | Classification | max_frame | max_uptime_ms | stuck_tid13_count | softlock_count | far_0xccd_count | panic_count | scheduler_lock | process_lock | gpu_pci_lock | ahci_irq |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | `no-softlock-within-window` | 2 | 50341 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 |

Run 1 reached the active-window timeout without the original `stuck_tid=13` softlock, but it failed the pass criteria because `panic_count=2`, max frame stayed at 2, and BWM did not progress healthily. Per directive, no stress runs were started.

Failure marker:

```text
CPU0 timer regression: tick_count=739 but peer max=30000;
read docs/planning/cpu0-user-guard-autopsy/README.md before touching anything
```

Endpoint GDB state:

```text
COMPOSITOR_DIRTY_WAKE.byte=1
COMPOSITOR_FRAME_WQ.logical_waiters=[]
COMPOSITOR_FRAME_WQ.raw_ring=[13, 0, 0, 0]
CLIENT_FRAME_WQ.logical_waiters=[16]
window[00] generation=70 last_uploaded_gen=0 last_read_gen=70 pending=True waiting_thread_id=Some(16)
tid_location tid=13 current_cpus=[0] previous_cpus=[] ready_queues=[]
cpu_is_idle=[0, 1, 1, 1, 1, 1, 1, 1]
```

## D. Verdict

**E3: Run 1 FAIL.**

Dropping `waiting_thread_id.is_some()` did not eliminate the CPU0 timer-regression failure. The original softlock did not appear, but BWM still stopped making visible frame progress after frame 2 while TID 13 remained current on CPU0 and CPU0's timer stopped advancing.

The upload-pending predicate is still too broad for the active BWM path. The endpoint state explains why: `last_uploaded_gen` is still 0 while `last_read_gen` has caught up to `generation`. That suggests this workload is not advancing `last_uploaded_gen` through `handle_composite_windows`; the direct compositor path reads the window and presents through the full-frame path instead. A predicate based on `generation > last_uploaded_gen` therefore remains permanently true for this mode.

## E. Comparison To Turn 4

| Field | Turn 4 broad predicate | Turn 5 upload-pending predicate |
| --- | --- | --- |
| Predicate | `generation > last_uploaded_gen || waiting_thread_id.is_some()` | `generation > last_uploaded_gen` |
| Classification | `non-softlock-failure-marker` | `no-softlock-within-window` with panic markers |
| CPU0 panic | yes, `tick_count=610`, peer max `30000` | yes, `tick_count=739`, peer max `30000` |
| max_frame | 2 | 2 |
| `stuck_tid13_count` | 0 | 0 |
| `COMPOSITOR_DIRTY_WAKE.byte` at GDB | 0 | 1 |
| Window state | `generation=53 last_uploaded_gen=0 last_read_gen=52 waiter=Some(16)` | `generation=70 last_uploaded_gen=0 last_read_gen=70 waiter=Some(16)` |
| TID 13 | current on CPU0 | current on CPU0 |

Dropping the waiter disjunction changed the exact endpoint state but did not remove the starvation mechanism. The upload-pending hypothesis was incomplete because `last_uploaded_gen` does not appear to be the right consumer-progress field for this direct-compositor workload.

## F. Proposed Next Step

Turn 6 should pivot from another predicate tweak to source-level path tracing of BWM's actual syscall sequence. The key question is which kernel-side state is the canonical "this window frame has been presented" marker for the direct compositor path:

- `read_window_buffer` / `check_window_dirty` advance `last_read_gen`;
- `handle_composite_windows` advances `last_uploaded_gen`, but the GDB counters and serial suggest this path is not active here;
- `virgl_composite_frame` calls `wake_presented_client_frames()`, which clears waiters when `last_read_gen == generation`.

The next fix likely belongs either in the direct present path's bookkeeping or in the wait predicate keyed to `last_read_gen`, not `last_uploaded_gen`. Do not rerun stress until that path model is corrected.
