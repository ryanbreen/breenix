# Turn 1 xHCI dispatch map

Scope: source walk only. No code changes and no tests were run, per directive.

Preflight caveat: the worktree is clean on `investigation/xhci-hid-poll-audit`, but both local branch tip and `origin/main` currently show `3492a660 scheduler: Linux-equivalent atomic wake-enqueue (delete rescue infrastructure) (#344)` as the latest merge. The directive expected PR #345 to be present; this map describes the source currently checked out in this branch.

## A. `handle_interrupt()` walkthrough

Signature:

```rust
// kernel/src/drivers/usb/xhci.rs:5246
pub fn handle_interrupt()
```

Callers:

- `kernel/src/arch_impl/aarch64/exception.rs:1271-1274`: the GIC SPI dispatch path calls `crate::drivers::usb::xhci::handle_interrupt()` when `irq_id == crate::drivers::usb::xhci::get_irq()`.
- `kernel/src/drivers/usb/xhci.rs:6274-6283`: `get_irq()` returns the early `XHCI_IRQ` value if non-zero, otherwise `XHCI_STATE.irq` after initialization.
- No x86 caller was found by `rg "handle_interrupt|poll_hid_events" kernel/src`.

Top-level order:

1. If `XHCI_INITIALIZED` is false, load `XHCI_IRQ`, disable and clear that SPI if non-zero, then return (`xhci.rs:5247-5256`).
2. Load `XHCI_STATE` (`5259-5264`).
3. Disable and clear the GIC SPI before taking the lock (`5266-5277`).
4. Take `XHCI_LOCK.try_lock()` (`5279-5283`). If contended, it returns immediately. Because the SPI was disabled before this point, this path does not re-enable the SPI.
5. Acknowledge xHC interrupt state: read/write IMAN.IP at `rt_base + 0x20` and USBSTS.EINT at `op_base + 0x04` (`5285-5294`).
6. Loop over the event ring while the current event TRB cycle bit matches `EVENT_RING_CYCLE` (`5296-5484`).
7. For every consumed event, advance `EVENT_RING_DEQUEUE`, toggle `EVENT_RING_CYCLE` on wrap, and write ERDP with EHB (`5473-5482`).
8. Clear pending and re-enable the GIC SPI after the ring drain completes (`5486-5495`).

Event handling:

- Reads the shared `EVENT_RING` and invalidates the current TRB cache line (`5298-5310`).
- Counts events in `MSI_EVENT_COUNT` (`5316`).
- `TRANSFER_EVENT`: records first-transfer diagnostics, processes keyboard/mouse/NKRO/mouse2 reports, submits them to `super::hid::process_keyboard_report()` or `process_mouse_report()`, and calls `queue_hid_transfer()` to requeue the endpoint (`5320-5425`).
- Error transfer completions do not reset endpoints in this handler. They set `NEEDS_RESET_*` flags for the timer path (`5426-5452`).
- `COMMAND_COMPLETION`: ignored as stray during interrupt handling (`5454-5457`).
- `PORT_STATUS_CHANGE`: calls `acknowledge_port_changes(state.op_base, port_id)` and increments `PSC_COUNT` (`5458-5467`).

Important callees and side effects:

- `queue_hid_transfer(state, hid_idx, slot_id, dci)` (`xhci.rs:3417-3529`) writes a Normal TRB to a HID transfer ring, cleans DMA cache, and rings the endpoint doorbell with `ring_doorbell()`.
- `ring_doorbell(state, slot, target)` writes to `state.db_base + slot * 4` (`1490-1495`).
- `acknowledge_port_changes(op_base, port_id)` reads PORTSC and writes RW1C change bits (`2275-2290`).
- MMIO writes from this path: GIC disable/clear/enable, IMAN, USBSTS, ERDP, endpoint doorbells through `queue_hid_transfer()`, and PORTSC for port-status changes.

Locking/concurrency:

- Uses `XHCI_LOCK.try_lock()`; it never spins in IRQ context.
- The SPI is disabled before `try_lock()`. If the lock is contended, `handle_interrupt()` returns with the SPI still disabled. The comment says `poll_hid_events will handle events`, which makes the timer path more than a passive fallback in that scenario.

## B. `poll_hid_events()` walkthrough

Signature:

```rust
// kernel/src/drivers/usb/xhci.rs:5687
pub fn poll_hid_events()
```

Callers:

- `kernel/src/arch_impl/aarch64/timer_interrupt.rs:750-754`: CPU 0 timer interrupt calls `crate::drivers::usb::xhci::poll_hid_events()` on every timer tick after polling PS/2 and EHCI input.
- `kernel/src/main_aarch64.rs:743-748`: boot message says USB HID input is active via xHCI and "polled from timer" because PCI interrupt routing may not be available.
- No x86 caller was found.

Top-level order:

1. Return unless `XHCI_INITIALIZED` is true (`5688-5690`).
2. Increment `POLL_COUNT` (`5692`).
3. Take `XHCI_LOCK.try_lock()` and skip this tick on contention (`5694-5698`).
4. Load `XHCI_STATE` (`5700-5705`).
5. Acknowledge IMAN.IP and USBSTS.EINT if set (`5707-5717`).
6. Run deferred TRB queueing only if `DEFERRED_TRB_POLL > 0`; current source has `const DEFERRED_TRB_POLL: u64 = 0`, so this branch is inactive (`82-99`, `5719-5727`).
7. Loop over the same shared event ring until the cycle bit no longer matches (`5729-6024`).
8. Process timer-only housekeeping: safety requeues, endpoint reset recovery, deferred SPI activation, `HID_TRBS_QUEUED` fixup, doorbell re-ring, and diagnostics (`6026-6262`).

Event handling:

- Reads the same `EVENT_RING`, uses the same `EVENT_RING_DEQUEUE` and `EVENT_RING_CYCLE`, invalidates the current event TRB cache line, and advances ERDP with EHB just like `handle_interrupt()` (`5730-5748`, `6013-6022`).
- Counts timer-drained events in `EVENT_COUNT` (`5748`).
- `TRANSFER_EVENT`: handles EP0 GET_REPORT response/late response, keyboard boot, keyboard NKRO, mouse, and mouse2; successful events call `process_keyboard_report()` or `process_mouse_report()` and requeue via `queue_hid_transfer()` (`5755-5997`).
- Error transfer completions capture extra endpoint/slot diagnostics and set `NEEDS_RESET_*` flags (`5920-5997`).
- `COMMAND_COMPLETION`: ignored as stray, with comments noting recovery commands are handled by `wait_for_command_completion()` (`5999-6002`).
- `PORT_STATUS_CHANGE`: increments `PSC_COUNT` and calls `acknowledge_port_changes()` (`6003-6008`).

Timer-only work after event drain:

- Consumes `MSI_*_NEEDS_REQUEUE` flags and calls `queue_hid_transfer()` (`6026-6047`). Despite the name/comment at `433-438`, these flags are not set by `handle_interrupt()` in current source; they are set by `wait_for_command_completion()` when it consumes endpoint-not-enabled transfer events while waiting for recovery command completions (`3721-3741`).
- Executes endpoint reset recovery for `NEEDS_RESET_*` flags with a per-endpoint `RESET_INTERVAL_TICKS` rate limit (`6051-6095`).
- `reset_halted_endpoint()` issues Reset Endpoint, optionally zeros the transfer ring, issues Set TR Dequeue Pointer, and requeues the HID transfer (`3772-3884`). It uses `wait_for_command_completion()`, which itself drains the event ring and writes ERDP while waiting (`3653-3769`).
- Enables the xHCI GIC SPI once at `poll >= 50` if `SPI_ACTIVATED` is false (`6097-6107`).
- Sets `HID_TRBS_QUEUED` at `poll >= 100` if still false (`6109-6112`).
- Re-rings all configured HID endpoint doorbells once at `poll == 75` (`6114-6135`).
- Runs periodic diagnostics every 2000 polls, reading USBCMD, USBSTS, IMAN, ERDP, event ring state, output endpoint context, transfer ring TRB 0, and PORTSC (`6164-6260`).

Locking/concurrency:

- Uses the same `XHCI_LOCK.try_lock()` as `handle_interrupt()`.
- Unlike the IRQ path, it does not disable the SPI before lock acquisition.
- It can run concurrently with an interrupt attempt, but the shared lock serializes event-ring and transfer-ring state when acquired. If either path cannot acquire the lock, it skips work rather than spinning.

## C. Overlap table

| Operation | `handle_interrupt()` | `poll_hid_events()` | Notes |
|---|---:|---:|---|
| Called from real xHCI IRQ | Y | N | `handle_interrupt()` is called by GIC SPI dispatch. |
| Called periodically from timer | N | Y | aarch64 CPU 0 timer path calls it every tick. |
| Gate on `XHCI_INITIALIZED` | Y | Y | Both return before initialized. |
| Take `XHCI_LOCK.try_lock()` | Y | Y | IRQ disables SPI before trying; timer does not. |
| Read IMAN / USBSTS | Y | Y | Both acknowledge pending interrupt state. |
| Write IMAN.IP / USBSTS.EINT | Y | Y | Both can clear xHC interrupt state. |
| Disable GIC SPI | Y | N | IRQ path disables before lock; timer never disables. |
| Enable GIC SPI | Y | Y | IRQ re-enables after successful drain; timer performs first deferred activation only. |
| Read event TRB | Y | Y | Same `EVENT_RING`. |
| Drain all available event TRBs | Y | Y | Both loop until cycle mismatch. |
| Advance software event dequeue | Y | Y | Same `EVENT_RING_DEQUEUE`. |
| Toggle event cycle on wrap | Y | Y | Same `EVENT_RING_CYCLE`. |
| Write ERDP with EHB | Y | Y | Same `ir0 + 0x18` register. |
| Handle Transfer Event success | Y | Y | Both process kbd/NKRO/mouse/mouse2 and requeue. |
| Submit HID report to stdin/input layer | Y | Y | Both call `super::hid::process_*_report()`. |
| Queue replacement HID TRB after success | Y | Y | Both call `queue_hid_transfer()`. |
| Ring endpoint doorbell through requeue | Y | Y | Via `queue_hid_transfer()`. |
| Handle EP0 GET_REPORT response | Partial | Y | IRQ path handles mouse-slot endpoint 1 only when `HID_TRBS_QUEUED`; timer has pending and late-response paths. |
| Mark endpoint reset needed on error CC | Y | Y | Both set `NEEDS_RESET_*`. |
| Execute endpoint Reset Endpoint command | N | Y | Timer calls `reset_halted_endpoint()`. |
| Execute Set TR Dequeue Pointer for recovery | N | Y | Timer-only via `reset_halted_endpoint()`. |
| Requeue from `MSI_*_NEEDS_REQUEUE` flags | N | Y | Flags are currently produced by command-wait event consumption. |
| Acknowledge Port Status Change | Y | Y | Both call `acknowledge_port_changes()`. |
| Initial HID TRB queueing | N | Conditional/inactive | Current `DEFERRED_TRB_POLL = 0`; init calls `start_hid_polling()` directly. |
| Doorbell re-ring after SPI activation | N | Y | Timer-only one-shot at poll 75. |
| Periodic controller diagnostics | N | Y | Timer-only every 2000 polls. |

## D. Misleading comment

The exact section title still says fallback:

```rust
// kernel/src/drivers/usb/xhci.rs:5498-5500
// =============================================================================
// Polling Mode (fallback for systems without interrupt support)
// =============================================================================
```

The current function comment is more specific but still describes a safety-net drain:

```rust
// kernel/src/drivers/usb/xhci.rs:5679-5687
/// Timer-tick housekeeping for xHCI.
///
/// Called from the timer interrupt at 200 Hz (every 5ms). Handles:
/// - One-time deferred SPI activation (first 250ms after init)
/// - Endpoint reset recovery for CC=12 errors
/// - Doorbell re-ring after SPI activation
/// - Draining any events the MSI handler missed (safety net only;
///   the primary event path is handle_interrupt)
pub fn poll_hid_events() {
```

The code does not disable `poll_hid_events()` when MSI is available. `kernel/src/arch_impl/aarch64/timer_interrupt.rs:750-754` calls it unconditionally on CPU 0. `poll_hid_events()` returns only if xHCI is not initialized or if `XHCI_LOCK.try_lock()` is contended.

There is no feature gate or runtime "MSI available, stop polling" branch. Instead, the timer path also performs MSI setup lifecycle work: it enables the SPI for the first time at `poll >= 50` and re-rings endpoint doorbells at `poll == 75`.

## E. Linux reference comparison

Source checked from:

- `https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/usb/host/xhci-ring.c`
- `https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/usb/host/xhci.c`

Relevant Linux flow:

- `xhci_irq()` (`xhci-ring.c:3177-3221`) takes `xhci->lock`, checks USBSTS, clears `STS_EINT`, then calls `xhci_handle_events(xhci, xhci->interrupters[0], false)`.
- `xhci_handle_events()` (`3086-3139`) clears IMAN.IP, loops over all OS-owned event TRBs, dispatches each event, advances the software dequeue, periodically updates ERDP under load, and finally updates ERDP with EHB.
- `xhci_handle_event_trb()` (`2986-3030`) dispatches command completions, port status, transfer events, device notifications, and vendor events.
- Linux timer references in these files are command timeout work (`cmd_timer`), root-hub/port status polling, and a compliance-mode recovery quirk (`xhci.c:369-430`). I found no Linux timer path analogous to Breenix `poll_hid_events()` that drains HID transfer events from the xHC event ring.

Linux does have polling concepts, but they are not HID event-ring fallback polling:

- `hcd->uses_new_polling = 1` in `xhci_run()` (`xhci.c:633-645`) belongs to USB core/root-hub polling behavior.
- `comp_mode_recovery_timer` polls USB3 port link state every 2 seconds for a specific compliance-mode hardware quirk (`xhci.c:409-418`).
- Command timers handle command timeout/recovery, not HID report delivery.

## F. Hypothesis for redundancy/divergence

The code itself suggests `poll_hid_events()` is currently load-bearing, but not because it is a clean fallback for systems without interrupt support.

The event-ring drain and HID report handling are true overlap: both functions read the same event ring, advance the same dequeue/cycle state, write ERDP, process the same HID endpoints, submit reports, and requeue transfer TRBs. That part looks redundant if MSI delivery is reliable and `handle_interrupt()` can always complete.

The divergent responsibilities are timer-only:

- First SPI activation after init (`poll >= 50`).
- Doorbell re-ring after SPI activation (`poll == 75`).
- Endpoint reset recovery after error completions.
- Requeue from events consumed inside recovery command waits.
- Periodic diagnostics.

The most concrete suspicious load-bearing scenario is the IRQ lock-contention path: `handle_interrupt()` disables the SPI, then uses `try_lock()`, then returns on contention before re-enabling. In that scenario, the timer path is the only path that can keep draining the event ring afterward. That is not "systems without interrupt support"; it is "IRQ handler bailed after masking the SPI" or "timer-owned recovery/setup still required."

Current source also has comments and constants indicating the design is mid-transition:

- `DEFERRED_TRB_POLL` is zero, so deferred queueing from the timer is disabled and init queues TRBs inline.
- A disabled deferred reconfigure block at `6137-6157` says the old deferred path caused CC=12 and is no longer needed.
- Comments at `433-438` say `MSI_*_NEEDS_REQUEUE` flags are set by the MSI handler to avoid IRQ-context requeue storms, but current `handle_interrupt()` directly requeues on success and does not set those flags. The flags are set by `wait_for_command_completion()` during timer-side recovery.

Based on source alone, I would not delete the timer path yet. The correct next step is runtime evidence that separates the overlapped drain from the timer-only duties, especially SPI activation, doorbell re-ring, endpoint reset recovery, and the IRQ try-lock contention path.

## G. Turn 2 proposal

Run nonintrusive runtime evidence collection before editing code. Specifically: boot with current code and capture xHCI counters/markers that distinguish MSI-drained events (`MSI_EVENT_COUNT`) from timer-drained events (`EVENT_COUNT`), confirm whether the first successful HID events arrive through MSI or timer after SPI activation, and use GDB breakpoints/watchpoints if needed on `handle_interrupt()`, `poll_hid_events()`, `EVENT_RING_DEQUEUE`, and `SPI_ACTIVATED`. Also inspect whether any `handle_interrupt()` call hits the lock-contended return after disabling the SPI. If timer event drain is zero under load and only timer setup/recovery is active, the likely refactor is to split timer housekeeping from event-ring drain; if timer drain is nonzero or SPI contention occurs, the documentation must name that precise scenario before any deletion.
