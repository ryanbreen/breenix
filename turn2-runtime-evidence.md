# Turn 2 Runtime Evidence: XHCI Event Drain Path

Status: INCONCLUSIVE

Turn 2 used only existing runtime evidence. No code, logging, or counter changes were made.

## A. Counter print site references

The directive expected a timer-side periodic diagnostic print around the end of `poll_hid_events()`. Current `kernel/src/drivers/usb/xhci.rs` does not serial-print those counters there. The periodic block at `kernel/src/drivers/usb/xhci.rs:6164` only snapshots MMIO and endpoint state into atomics every 2000 polls.

Existing printable diagnostic output is appended by `format_trace_buffer()` for the xHCI trace/proc output:

```rust
// kernel/src/drivers/usb/xhci.rs:1003
let _ = writeln!(out, "=== XHCI_DIAG ===");
let _ = writeln!(out, "poll_count {}", POLL_COUNT.load(Ordering::Relaxed));
let _ = writeln!(out, "event_count {}", EVENT_COUNT.load(Ordering::Relaxed));
```

The path-specific counters are incremented in the two drain loops:

```rust
// kernel/src/drivers/usb/xhci.rs:5316
MSI_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

// kernel/src/drivers/usb/xhci.rs:5748
let _evt_num = EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
```

`MSI_EVENT_COUNT` and `PSC_COUNT` are not printed by `format_trace_buffer()`, so the Parallels run used GDB reads of the existing atomics after serial logs failed to expose them.

Important activation reference:

```rust
// kernel/src/drivers/usb/xhci.rs:6102
if state.irq != 0 && poll >= 50 && !SPI_ACTIVATED.load(Ordering::Relaxed) {
    SPI_ACTIVATED.store(true, Ordering::Release);
    crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
    crate::arch_impl::aarch64::gic::enable_spi(state.irq);
    DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed);
}
```

## B. QEMU boot evidence

Artifacts:

- `turn2-artifacts/aarch64-qemu-boot.log`
- `turn2-artifacts/aarch64-qemu-serial-last-attempt.log`

The native aarch64 QEMU boot did not reach usable xHCI HID evidence. The harness config uses virtio keyboard/tablet devices (`docker/qemu/run-aarch64-boot-test-native.sh:77-79`), not a USB xHCI HID device, and the boot test failed all five attempts before any xHCI HID diagnostic interval.

Raw excerpt:

```text
Attempt 1/5...
FAIL: Userspace not detected (     530 lines)
Attempt 2/5...
FAIL: Kernel panic (     643 lines)
Attempt 3/5...
FAIL: CPU exception (    5474 lines)
Attempt 4/5...
FAIL: CPU exception (   21095 lines)
Attempt 5/5...
FAIL: CPU exception (   13579 lines)

ARM64 BOOT TEST: FAILED (after 5 attempts)
Last output:
[UNHANDLED_EC] cpu=0 EC=0x0 ELR=0xffff000040121468
```

Counter table:

| Metric | First diagnostic | Second diagnostic | Third diagnostic |
|---|---:|---:|---:|
| `POLL_COUNT` | n/a | n/a | n/a |
| `EVENT_COUNT` | n/a | n/a | n/a |
| `MSI_EVENT_COUNT` | n/a | n/a | n/a |
| `PSC_COUNT` | n/a | n/a | n/a |
| `SPI_ACTIVATED` | n/a | n/a | n/a |

QEMU result: no xHCI HID transfer-event evidence. This environment cannot answer the drain-path question from this capture.

## C. Parallels boot evidence

Artifacts:

- `turn2-artifacts/aarch64-parallels-run.out`
- `turn2-artifacts/aarch64-parallels-boot.log`
- `turn2-artifacts/aarch64-parallels-key-events.log`
- `turn2-artifacts/aarch64-parallels-screenshot.png`
- `turn2-artifacts/aarch64-parallels-stop.log`
- `turn2-artifacts/parallels-gdb2/`

The Parallels full boot enumerated xHCI HID devices and queued interrupt TRBs:

```text
[drivers] Found XHCI at 00:03.0 [1033:0194]
[xhci] start_hid_polling: kbd=slot2/dci3 nkro=dci5 mouse=slot1/dci3 mouse2=dci5
[xhci] Initialized: 32 slots, MSI irq=56
[boot] USB HID input active via XHCI (polled from timer)
```

I injected two key-event sequences through `prlctl send-key-event`; the scancodes and timestamps are in `turn2-artifacts/aarch64-parallels-key-events.log` and `turn2-artifacts/parallels-gdb2/key-events.log`.

The boot then hit the existing CPU0 timer regression alarm before any xHCI diagnostic interval:

```text
[freeze-watch] uptime_ms=45291 ... timer_ticks_cpu0=6 ... timer_ticks_cpu1=29632 ...

!!! CPU0 REGRESSION ALARM !!!
CPU0 tick_count = 6, max peer = 30000
panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:
CPU0 timer regression: tick_count=6 but peer max=30000
```

Because no serial `XHCI_DIAG` block appeared, I read the existing atomics with Parallels GDB snapshots at approximately 10s, 20s, 30s, and 38s. Reads succeeded with `gdb_rc=0`; GDB emitted target-description warnings, but the memory values were returned consistently.

| Metric | t10 | t20 | t30 | t38 |
|---|---:|---:|---:|---:|
| `POLL_COUNT` | 5 | 5 | 5 | 5 |
| `EVENT_COUNT` timer drain | 0 | 0 | 0 | 0 |
| `MSI_EVENT_COUNT` IRQ drain | 0 | 0 | 0 | 0 |
| `PSC_COUNT` | 0 | 0 | 0 | 0 |
| `SPI_ACTIVATED` | 0 | 0 | 0 | 0 |
| `DIAG_SPI_ENABLE_COUNT` | 0 | 0 | 0 | 0 |
| `XHCI_INITIALIZED` | 1 | 1 | 1 | 1 |
| `XHCI_IRQ` | 56 | 56 | 56 | 56 |
| `HID_TRBS_QUEUED` | 1 | 1 | 1 | 1 |
| `EVENT_RING_DEQUEUE` | 59 | 59 | 59 | 59 |
| `EVENT_RING_CYCLE` | 1 | 1 | 1 | 1 |
| `KBD_EVENT_COUNT` | 0 | 0 | 0 | 0 |
| `NKRO_EVENT_COUNT` | 0 | 0 | 0 | 0 |
| `XFER_OTHER_COUNT` | 0 | 0 | 0 | 0 |
| `XO_ERR_COUNT` | 0 | 0 | 0 | 0 |
| `ENDPOINT_RESET_COUNT` | 0 | 0 | 0 | 0 |
| `ENDPOINT_RESET_FAIL_COUNT` | 0 | 0 | 0 | 0 |
| `NEEDS_RESET_*` | 0 | 0 | 0 | 0 |

Parallels result: xHCI was initialized and HID TRBs were queued, but no HID transfer events were observed by either drain path in the measured window.

## D. GDB breakpoint counts

Artifacts:

- `turn2-artifacts/parallels-gdb-breakpoints/`
- `turn2-artifacts/parallels-gdb/`

I attempted a breakpoint-count run against:

- `kernel::drivers::usb::xhci::poll_hid_events` at `0xffff0000400b572c`
- `kernel::drivers::usb::xhci::handle_interrupt` at `0xffff0000400b4cbc`

The breakpoint run did not produce hit counts. GDB disconnected while connecting to the Parallels guest debugger:

```text
breakpoint-count.gdb:9: Error in sourced command file:
Remote communication error.  Target disconnected: error while reading: Connection reset by peer.
```

The earlier `turn2-artifacts/parallels-gdb/` attach also failed because guest-debugger was started before the generated VM had reached running state:

```text
Executing command 'guestdebugger' failed. Error Unable to perform the operation because "breenix-1779247556" is not started.
```

GDB counter reads in `turn2-artifacts/parallels-gdb2/` did work, but breakpoint hit ratios are unavailable.

| Probe | Result |
|---|---|
| `poll_hid_events` breakpoint hits | unavailable |
| `handle_interrupt` breakpoint hits | unavailable |
| IRQ try-lock failure breakpoint | not attempted after breakpoint attach failure |

## E. Conclusion: which path drains events

This turn cannot determine which path drains real HID transfer events.

Concrete observed facts:

- QEMU did not provide usable xHCI HID evidence.
- Parallels did enumerate xHCI HID and queued HID TRBs.
- `POLL_COUNT` reached only 5 and then stopped.
- `SPI_ACTIVATED` stayed false and `DIAG_SPI_ENABLE_COUNT` stayed 0.
- The IRQ number was present (`XHCI_IRQ=56`), but the one-shot SPI activation gate at `poll >= 50` never ran.
- `EVENT_COUNT` and `MSI_EVENT_COUNT` both stayed 0 after injected key events.

This is evidence that no HID transfer events were observed by either drain loop in the captured Parallels window. It is not evidence that timer drain is primary, and it is not evidence that IRQ drain is redundant. The current measurement is capped by the CPU0 timer regression before xHCI SPI activation.

## F. Lock-contention path: hit or not

Not observed.

`handle_interrupt()` disables the GIC SPI before taking `XHCI_LOCK`:

```rust
// kernel/src/drivers/usb/xhci.rs:5274
if state.irq != 0 {
    crate::arch_impl::aarch64::gic::disable_spi(state.irq);
    crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
}

// kernel/src/drivers/usb/xhci.rs:5280
let _guard = match XHCI_LOCK.try_lock() {
    Some(g) => g,
    None => return,
};
```

In this run, `SPI_ACTIVATED=0`, `DIAG_SPI_ENABLE_COUNT=0`, and `MSI_EVENT_COUNT=0`, so the xHCI SPI was never enabled in the measured window. That means the lock-contention early-return path was not exercised by the evidence collected here.

## G. Turn 3 proposal

Do not delete or refactor the timer event-drain overlap yet. Turn 2 failed to reach the runtime condition needed to evaluate the overlap.

Recommended Turn 3 direction:

1. Remove the measurement ceiling first. Use an aarch64/Parallels boot mode that reaches at least `POLL_COUNT >= 50`, or get explicit direction to handle the CPU0 timer regression alarm that stops the current run before xHCI SPI activation.
2. Once `SPI_ACTIVATED=1`, repeat the same no-code counter read with injected key events and collect `EVENT_COUNT`, `MSI_EVENT_COUNT`, `PSC_COUNT`, and `POLL_COUNT`.
3. If the boot still cannot reach that point, use a more robust Parallels GDB harness that attaches after the VM is running but before the CPU0 regression window, then arms breakpoints without repeated reconnects.

Decision criteria for the next turn:

- If `MSI_EVENT_COUNT > 0` under HID input, IRQ drain is real and the timer drain overlap may be narrowed to housekeeping/recovery.
- If `MSI_EVENT_COUNT == 0` while `EVENT_COUNT > 0`, timer drain is currently primary and MSI delivery needs fixing or explicit documentation.
- If both stay 0 after `SPI_ACTIVATED=1` and key events, the problem is below the drain-path split: HID transfers are not producing events.
- If the IRQ try-lock failure branch is hit after SPI activation, treat that as a correctness bug because `handle_interrupt()` disables the SPI before the contended-lock early return.

## Build and artifact notes

The Parallels run built the UEFI loader and kernel cleanly in the captured output. The userspace build stage reported an existing local `rust-fork` dependency issue:

```text
failed to get `std_detect` as a dependency of package `std v0.0.0`
failed to read `/Users/wrb/fun/code/breenix/rust-fork/library/stdarch/crates/std_detect/Cargo.toml`
```

The Parallels script continued by installing existing userspace binaries. I did not alter this because Turn 2 was observation-only.
