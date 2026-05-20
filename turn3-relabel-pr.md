# Turn 3 Relabel PR Close-Out

Status: COMPLETE

## A. What was relabeled

### 1. `MSI_*_NEEDS_REQUEUE` flag comment

Location after edit: `kernel/src/drivers/usb/xhci.rs:433`

Before:

```rust
/// Flags set by MSI interrupt handler to request requeue from timer poll.
/// Requeuing from IRQ context causes MSI storms on virtual XHCI controllers.
```

After:

```rust
/// Flags set by wait_for_command_completion() when timer-side recovery consumes
/// transfer events while waiting for recovery commands.
/// The next timer poll requeues the corresponding HID endpoint after recovery.
```

Reason: Turn 1 source reading showed these flags are set from `wait_for_command_completion()` during timer-side recovery command waits, not by `handle_interrupt()`.

### 2. Polling section title

Location after edit: `kernel/src/drivers/usb/xhci.rs:5500`

Before:

```rust
// =============================================================================
// Polling Mode (fallback for systems without interrupt support)
// =============================================================================
```

After:

```rust
// =============================================================================
// Timer-Driven Housekeeping + Event Drain (NOT a fallback)
//
// Despite the legacy section name, this is NOT optional polling that disables
// when MSI is available. On aarch64, the timer path is load-bearing for:
//   1. First-time xHCI GIC SPI activation (poll >= 50, one-shot)
//   2. Endpoint reset recovery for CC=12 errors (per-endpoint rate-limited)
//   3. Doorbell re-ring after SPI activation (poll == 75, one-shot)
//   4. Requeue from events consumed inside recovery command waits
//   5. Rescue drain when handle_interrupt() bails on XHCI_LOCK try_lock
//      contention (handle_interrupt() disables the GIC SPI BEFORE try_lock()
//      and returns without re-enabling on contention; only this timer path can
//      drain after that)
//   6. Periodic diagnostics every 2000 polls
//
// The event-ring drain done here overlaps with handle_interrupt(). That overlap
// is intentional for the rescue scenario in (5). Whether the drain can be
// narrowed to housekeeping-only requires runtime measurement that is currently
// blocked by an unrelated pre-existing CPU0 timer regression on Parallels; see
// the follow-up issue for the audit conclusion.
// =============================================================================
```

Reason: the old title implied optional fallback behavior. The source and runtime audit showed the timer path is load-bearing on aarch64.

### 3. `poll_hid_events()` docstring

Location after edit: `kernel/src/drivers/usb/xhci.rs:5698`

Before:

```rust
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

After:

```rust
/// Timer-driven xHCI housekeeping and event drain (aarch64).
///
/// Called from the aarch64 CPU0 timer interrupt at 200 Hz (every 5ms).
/// This is NOT a "fallback for systems without interrupt support"; it owns
/// responsibilities that handle_interrupt() does not:
///
/// - One-time SPI activation at `poll >= 50` (~250ms after init). Until this
///   runs, the xHCI GIC SPI is disabled and handle_interrupt() cannot be
///   triggered for HID events. Deleting this path would break MSI delivery.
/// - Doorbell re-ring at `poll == 75`, one-shot, after SPI activation.
/// - Endpoint reset recovery for CC=12 errors (per-endpoint rate-limited via
///   RESET_INTERVAL_TICKS). handle_interrupt() only flags NEEDS_RESET_*; the
///   actual Reset Endpoint + Set TR Dequeue Pointer is timer-side.
/// - Requeue from MSI_*_NEEDS_REQUEUE flags set by
///   wait_for_command_completion() when it consumes transfer events while
///   waiting for recovery commands.
/// - Rescue event-ring drain for the case where handle_interrupt() disabled
///   the GIC SPI before its try_lock(), lost the lock to a concurrent
///   timer-side holder, and returned WITHOUT re-enabling the SPI. In that
///   scenario this timer path is the only remaining drain path.
/// - Periodic diagnostics every 2000 polls.
///
/// The event-ring drain done here overlaps with handle_interrupt()'s drain.
/// That overlap is intentional for the rescue case. Whether the drain can be
/// narrowed away (keeping only housekeeping) requires runtime evidence that is
/// environmentally blocked on Parallels by a pre-existing CPU0 timer regression
/// (timer_interrupt.rs:598). See follow-up issue.
pub fn poll_hid_events() {
```

Reason: the old docstring called the event drain a "safety net only" and called `handle_interrupt()` primary. The audit cannot support that claim. It can support that the timer path owns first SPI activation, recovery, requeue, diagnostics, and a rescue drain scenario.

## B. Build verification

aarch64:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Result: exited 0. The warning/error filter produced no output. Full local log: `/tmp/breenix-turn3-aarch64-build.log`.

x86:

```bash
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

Result: exited 0. The warning/error filter produced no output. Full local log: `/tmp/breenix-turn3-x86-build.log`.

No QEMU or Parallels boot was run for Turn 3 because the requested change was comments only and no behavioral code changed.

## C. PR URL

https://github.com/ryanbreen/breenix/pull/346

Comment-only relabel commit: `993f40c2 docs(xhci): relabel poll_hid_events as load-bearing, not MSI fallback`

## D. Why this is the right answer

Turn 1 established from source that `poll_hid_events()` is not an optional fallback: it owns first xHCI GIC SPI activation, endpoint reset recovery, doorbell re-ring, recovery-command requeue, periodic diagnostics, and the only drain path after the IRQ handler disables SPI then returns on lock contention. Turn 2 could not answer whether the overlapping event-ring drain can be narrowed, because the Parallels CPU0 timer regression stops the boot before `poll >= 50` and `SPI_ACTIVATED=1`. The defensible outcome is therefore KEEP+RELABEL now, with drain-narrowing deferred until post-regression runtime evidence exists.

## E. Status

COMPLETE
