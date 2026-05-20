# Turn 4: System-Ready xHCI MSI Activation + Observability

Status: INCONCLUSIVE

Turn 4 restored the Turn 3 implementation locally and added the requested
single one-shot boot diagnostic, but the runtime gate still did not prove MSI
event delivery. Per the directive, the source change is not being shipped.

## A. Implementation Attempt

Attempted source diff:

- `turn4-artifacts/system-ready-msi-observability-attempt.diff`

The local patch touched only:

- `kernel/src/drivers/usb/xhci.rs`
- `kernel/src/main_aarch64.rs`

Applied shape:

- `xhci.rs:5498` added locked `activate_msi_if_ready_locked(...)`
- `xhci.rs:6305` added public `activate_msi_if_ready()`
- `xhci.rs:6133` converted the poll-50 branch into a transitional fallback
  calling the same helper without printing from the timer path
- `main_aarch64.rs:862` called `activate_msi_if_ready()` after `/sbin/init`
  preload and before timer init
- `main_aarch64.rs:868` added one boot-context diagnostic print:
  `MSI_EVENT_COUNT`, `EVENT_COUNT`, `POLL_COUNT`, and `SPI_ACTIVATED`

The first runtime attempt used `log::info!` for the new markers, which did not
appear in the serial capture. The patch was then adjusted to use
`serial_println!` / `crate::serial_println!`, still outside hot paths.

## B. Build Verification

Artifacts:

- `turn4-artifacts/build-aarch64.log`
- `turn4-artifacts/build-x86.log`
- `turn4-artifacts/build-efi.log`

Commands completed cleanly:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
scripts/parallels/build-efi.sh --kernel
```

`grep -E "^(warning|error)"` was empty for both Rust build logs.

## C. Runtime Evidence

Final runtime artifacts:

- `turn4-artifacts/parallels-boot-decimal-input.log`
- `turn4-artifacts/parallels-boot-decimal-input-grep.txt`
- `turn4-artifacts/key-injection-decimal.log`
- `turn4-artifacts/run-parallels-decimal-xtrace.log`
- `turn4-artifacts/prlctl-stop-decimal.txt`
- `turn4-artifacts/prlctl-after-stop-decimal.txt`

The system-ready activation fired before timer init:

```text
[boot] Init binary pre-loaded: 296776 bytes
[xhci] activate_msi_if_ready: source=system-ready spi=56 DIAG_SPI_ENABLE_COUNT=1
[xhci] post-activation: MSI_EVENT_COUNT=0 EVENT_COUNT=0 POLL_COUNT=0 SPI_ACTIVATED=true
[boot] Initializing timer interrupt...
[boot] Launching init from pre-loaded ELF...
[timer] cpu0 ticks=5000
[timer] cpu0 ticks=10000
[timer] cpu0 ticks=15000
[timer] cpu0 ticks=20000
[timer] cpu0 ticks=25000
```

This proves:

- the system-ready path beat the timer fallback
- `DIAG_SPI_ENABLE_COUNT=1`
- boot reached userspace
- the previous Turn 3 CPU0 timer regression did not reproduce in this run

But the required MSI event evidence is still missing:

```text
MSI_EVENT_COUNT=0 EVENT_COUNT=0 POLL_COUNT=0 SPI_ACTIVATED=true
```

Input injection notes:

- `prlctl send-key` is not supported on this host; Parallels 26.3.2 reports the
  supported action as `send-key-event`.
- The directive's hex key values were rejected by `send-key-event`.
- A follow-up run used decimal key values (`20`, `24`, `18`, `57`) and the CLI
  accepted the press/release events.
- The one-shot diagnostic still printed `MSI_EVENT_COUNT=0`.

## D. Case Verdict

Case B / INCONCLUSIVE.

Key injection did occur in the decimal-key run, but the only allowed one-shot
diagnostic is located immediately after SPI activation. The serial log proves
that diagnostic still saw zero MSI events. Because there is no later
non-hot-path diagnostic, this run cannot distinguish between:

- real MSI non-delivery after GIC SPI enable
- input timing that missed the immediate post-activation observation window
- xHCI events delivered later with no serial-visible counter print

The audit criterion `MSI_EVENT_COUNT > 0` was not met, so this is not Case A.
It is also not the borderline Case C success path because decimal key injection
was accepted by the CLI.

## E. Status

INCONCLUSIVE.

The source edits should remain reverted until MSI event delivery can be proven.

## F. Turn 5 Proposal

Use one of these evidence paths:

1. Add a second one-shot diagnostic at a later non-hot-path boot point after
   userspace/input has had time to run, then repeat decimal `send-key-event`
   injection.
2. Use GDB on a fresh Parallels boot with breakpoints or watchpoints around
   `kernel::drivers::usb::xhci::handle_interrupt` and `MSI_EVENT_COUNT` to
   prove whether SPI 56 is entering the xHCI MSI handler after activation.
3. If logging must remain single-site, move the one-shot diagnostic later than
   immediate post-activation so it can observe injected HID activity.

