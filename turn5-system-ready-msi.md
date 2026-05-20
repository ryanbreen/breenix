# Turn 5: System-Ready xHCI MSI Activation

Status: COMPLETE (Case C)

PR: https://github.com/ryanbreen/breenix/pull/347

Code commit: `b32e773b`

## A. Implementation

Turn 5 re-applied the exact Turn 4 diff from:

- `turn4-artifacts/system-ready-msi-observability-attempt.diff`

The implementation keeps changes confined to:

- `kernel/src/drivers/usb/xhci.rs`
- `kernel/src/main_aarch64.rs`

Key behavior:

- `xhci::activate_msi_if_ready()` arms the xHCI GIC SPI exactly once.
- A locked private helper performs the actual `clear_spi_pending()` and
  `enable_spi()` sequence.
- `poll_hid_events()` keeps a transitional poll-50 fallback, but it now calls
  the same helper and does not print from the timer path.
- `main_aarch64.rs` calls the helper after `/sbin/init` preload and before
  timer init.
- One boot-context diagnostic prints `MSI_EVENT_COUNT`, `EVENT_COUNT`,
  `POLL_COUNT`, and `SPI_ACTIVATED`.

This preserves the safe trigger point proven in Turns 3 and 4 while avoiding
the reverted `703db7de` behavior of enabling SPI at the end of `xhci::init()`.

## B. Build Verification

Artifacts:

- `turn5-artifacts/build-aarch64.log`
- `turn5-artifacts/build-x86.log`
- `turn5-artifacts/build-efi.log`

Commands completed cleanly:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
scripts/parallels/build-efi.sh --kernel
```

`grep -E "^(warning|error)"` was empty for both Rust build logs.

## C. Runtime Evidence

Artifacts:

- `turn5-artifacts/parallels-boot-extended.log`
- `turn5-artifacts/grep-markers.txt`
- `turn5-artifacts/cpu0-tick-trajectory.txt`
- `turn5-artifacts/activation-source.txt`
- `turn5-artifacts/prlctl-stop.log`

Key serial evidence:

```text
[xhci] activate_msi_if_ready: source=system-ready spi=56 DIAG_SPI_ENABLE_COUNT=1
[xhci] post-activation: MSI_EVENT_COUNT=0 EVENT_COUNT=0 POLL_COUNT=0 SPI_ACTIVATED=true
[timer] cpu0 ticks=5000
[timer] cpu0 ticks=10000
[timer] cpu0 ticks=15000
[timer] cpu0 ticks=20000
[timer] cpu0 ticks=25000
[timer] cpu0 ticks=30000
[timer] cpu0 ticks=35000
[timer] cpu0 ticks=40000
[timer] cpu0 ticks=45000
```

The activation-source artifact contains only the system-ready source:

```text
[xhci] activate_msi_if_ready: source=system-ready spi=56 DIAG_SPI_ENABLE_COUNT=1
```

This proves the timer fallback did not win the activation race.

The extended boot also reached userspace:

```text
[boot] Launching init from pre-loaded ELF...
[init] Breenix init starting (PID 1)
```

## D. Case Verdict

Case C: COMPLETE with honest measurement limitation.

`MSI_EVENT_COUNT` stayed `0` in the observed Breenix boot. That is expected in
this Parallels setup because `prlctl send-key-event` routes input through
Parallels' virtio-input path, not through xHCI HID endpoints. The test
environment therefore cannot directly inject xHCI HID transfer events.

Turn 2's Linux probe remains the empirical hardware proof for MSI delivery on
the same Parallels aarch64 platform:

- xHCI uses MSI address `0x02250040`, data `0x0038` = SPI 56
- `/proc/interrupts` showed 137 `xhci_hcd` MSI interrupts at boot
- the GICv2m frame and SPI range match Breenix's routing assumptions

The structural fix is proven by Turn 5:

- SPI activation happens from system-ready before timer init
- `SPI_ACTIVATED=true`
- `DIAG_SPI_ENABLE_COUNT=1`
- CPU0 timer stayed healthy to 45000 ticks
- userspace PID 1 launched
- no timer fallback activation appeared

## E. Status

COMPLETE.

The code was kept, committed, pushed, and opened as PR #347.

## F. Follow-Up Notes

Direct `MSI_EVENT_COUNT > 0` proof still requires an input source that actually
targets xHCI, such as physical USB HID passthrough. `prlctl send-key-event` is
not sufficient because it exercises a different device path.

If operator review requires direct Breenix-side MSI handler proof without USB
passthrough, the next practical approach is GDB instrumentation around
`kernel::drivers::usb::xhci::handle_interrupt` and `MSI_EVENT_COUNT`.

