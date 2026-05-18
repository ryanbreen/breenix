# Turn 4 Fix Attempt

## A. Source Diff

Attempted change in `kernel/src/syscall/graphics.rs`:

```rust
186 #[cfg(target_arch = "aarch64")]
187 fn compositor_has_pending_window_frame() -> bool {
188     let reg = WINDOW_REGISTRY.lock();
189     reg.buffers.iter().any(|slot| {
190         if let Some(buf) = slot.as_ref() {
191             buf.registered
192                 && buf.width > 0
193                 && buf.height > 0
194                 && (buf.generation > buf.last_uploaded_gen || buf.waiting_thread_id.is_some())
195         } else {
196             false
197         }
198     })
199 }
200
201 #[cfg(target_arch = "aarch64")]
202 fn compositor_ready_bits(last_registry_gen: u64, prev_mouse: u64) -> (u64, u64, u64) {
203     use core::sync::atomic::Ordering;
204
205     let (mx, my, mb) = crate::drivers::usb::hid::mouse_state();
206     let mouse_packed = ((mx as u64) << 32) | ((my as u64) << 16) | (mb as u64);
207     let cur_reg_gen = REGISTRY_GENERATION.load(Ordering::Relaxed);
208
209     let mut ready = 0u64;
210     if COMPOSITOR_DIRTY_WAKE.swap(false, Ordering::Relaxed) {
211         ready |= 1;
212     }
213     // WHY: H2 fix. The dirty-wake latch is consumable; persistent window
214     // registry state must also keep BWM awake while client frame work remains.
215     // See turn3-wedge-snapshot.md for the captured lost-readiness wedge.
216     if compositor_has_pending_window_frame() {
217         ready |= 1;
218     }
219     if mouse_packed != prev_mouse
220         || crate::drivers::usb::hid::has_pending_press()
221         || crate::drivers::usb::hid::has_pending_scroll()
```

The attempted source patch was reverted after the E3 validation failure. The branch commit for Turn 4 records the report and artifacts only, not the failed kernel change.

Codegen check:

- `nm -nC target/aarch64-breenix/release/kernel-aarch64` showed `compositor_ready_bits` at `0xffff000040115824`.
- `rust-objdump` confirmed the new registry predicate was on the `compositor_ready_bits` fast path after the `COMPOSITOR_DIRTY_WAKE.swap(false)` sequence.
- LLVM inlined the fixed-size registry scan into `compositor_ready_bits` as a 16-slot early-exit scan under the existing `WINDOW_REGISTRY` spinlock. It unlocked before mouse/registry return handling.

## B. Build Matrix

| Gate | Command | Outcome |
| --- | --- | --- |
| aarch64 kernel | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS, zero warnings/errors |
| qemu-uefi | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS, zero warnings/errors |
| diff whitespace | `git diff --check` | PASS |
| warning grep | `grep -E "^(warning|error)" /tmp/breenix-turn4-kernel-build.log /tmp/breenix-turn4-qemu-uefi-build.log` | no output |

## C. Validation Table

Run 1 used `turn4-artifacts/run_bwm_softlock_capture.sh mode=build run=reproduce-run1`.

| Run | Classification | max_frame | max_uptime_ms | stuck_tid13_count | softlock_count | far_0xccd_count | panic_count | scheduler_lock | process_lock | gpu_pci_lock | ahci_irq |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | `non-softlock-failure-marker` | 2 | 38289 | 0 | 0 | 0 | 14 | 0 | 0 | 0 | 0 |

Run 1 failed early, so the directive required stopping immediately without 5-boot stress.

Failure marker:

```text
panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:
CPU0 timer regression: tick_count=610 but peer max=30000;
read docs/planning/cpu0-user-guard-autopsy/README.md before touching anything
```

Endpoint GDB state at failure:

```text
COMPOSITOR_DIRTY_WAKE.byte=0
COMPOSITOR_FRAME_WQ.logical_waiters=[]
COMPOSITOR_FRAME_WQ.raw_ring=[13, 0, 0, 0]
CLIENT_FRAME_WQ.logical_waiters=[16]
window[00] generation=53 last_uploaded_gen=0 last_read_gen=52 pending=True waiting_thread_id=Some(16)
tid_location tid=13 current_cpus=[0] previous_cpus=[] ready_queues=[]
tid_location tid=16 current_cpus=[] previous_cpus=[] ready_queues=[]
cpu_is_idle=[0, 1, 1, 1, 1, 1, 1, 1]
```

## D. Verdict

**E3: Run 1 FAIL.**

The attempted H2 predicate did prevent the original validation signature from appearing in this run: there were zero `stuck_tid=13` markers and zero softlock markers. But it introduced or exposed a different liveness failure: BWM/TID 13 stayed current on CPU0 while CPU0's timer count stopped at 610 and peer CPUs reached 30000 ticks, tripping the CPU0 timer regression guard.

The patch did not land. It is not safe to merge as written.

## E. Comparison To Turn 3 Wedge

| Field | Turn 3 wedge | Turn 4 failed run |
| --- | --- | --- |
| Classification | `softlock-leading-edge` | `non-softlock-failure-marker` |
| `stuck_tid13_count` | 14 | 0 |
| `softlock_count` | 12 | 0 |
| `panic_count` | 0 | 14 |
| `COMPOSITOR_DIRTY_WAKE.byte` | 0 | 0 |
| `COMPOSITOR_FRAME_WQ` | len 0, raw ring `[13,13,13,13]` | len 0, raw ring `[13,0,0,0]` |
| `CLIENT_FRAME_WQ` | waiter `[16]` | waiter `[16]` |
| Window slot 0 | `generation=25944`, `last_uploaded_gen=0`, `last_read_gen=25944`, waiter `Some(16)` | `generation=53`, `last_uploaded_gen=0`, `last_read_gen=52`, waiter `Some(16)` |
| TID 13 | current on CPU2 | current on CPU0 |
| `gpu_pci_lock_byte` | 1 | 0 |
| Failure mode | compositor-wait softlock | CPU0 timer-regression panic |

The important new signal is `last_read_gen=52` while `generation=53`: the attempted predicate kept readiness alive, but the runtime did not reach the frame-present path that clears `waiting_thread_id`. Treating `waiting_thread_id.is_some()` as unconditional compositor readiness appears too broad because it can keep BWM runnable even when the next required transition is not simply "wake and inspect dirty bit again."

## F. Proposed Next Step

Turn 5 should refine the readiness model before another fix attempt. Specifically, inspect the BWM syscall sequence around op 22/14/10/16 and distinguish:

- unconsumed client pixels: `generation > last_read_gen`;
- read-but-not-presented client frame: `waiting_thread_id.is_some() && last_read_gen == generation`;
- GPU-uploaded multi-window path: `generation > last_uploaded_gen`.

The next patch likely should not use `waiting_thread_id.is_some()` alone as a compositor-wait dirty predicate. It should either target the precise consumer path that clears/wakes `waiting_thread_id`, or ensure the wait predicate cannot create a CPU0 userspace busy-loop while frame presentation is stalled.
