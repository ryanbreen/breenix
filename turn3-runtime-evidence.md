# Turn 3 Runtime Evidence: System-Ready xHCI MSI Activation

Status: INCONCLUSIVE

Turn 3 implemented the requested shape locally, but the runtime gate did not
pass. Per directive, the source edits to `kernel/src/drivers/usb/xhci.rs` and
`kernel/src/main_aarch64.rs` were reverted before this writeup was committed.

## A. Reverted Commit Summary

Captured artifacts:

- `turn3-artifacts/703db7de-attempt.diff`
- `turn3-artifacts/c077c6ba-revert.diff`
- `turn3-artifacts/c077c6ba-message.txt`

`703db7de` enabled the xHCI GIC SPI at the end of `xhci::init()`, immediately
after storing `XHCI_IRQ`, and removed the `poll >= 50` first-enable path from
`poll_hid_events()`.

`c077c6ba` reverted that patch. The revert commit message only identifies the
reverted commit, but PR #333 from Turn 2 explains the reason: enabling at the
end of `xhci::init()` fired too early, before AHCI/filesystem/scheduler boot
work completed, and caused disk reads to stall.

The safe precondition is therefore not "xHCI init complete"; it is "boot-critical
storage/filesystem work complete."

## B. Attempted Design

The local patch added:

- a private locked helper in `xhci.rs` that performed the one-shot
  `SPI_ACTIVATED.compare_exchange(false, true, ...)`
- `clear_spi_pending(state.irq)`
- `enable_spi(state.irq)`
- `DIAG_SPI_ENABLE_COUNT += 1`
- a rare success print for the system-ready caller only:
  `[xhci] activate_msi_if_ready: source=system-ready spi=56 DIAG_SPI_ENABLE_COUNT=1`
- a public `xhci::activate_msi_if_ready()` wrapper that acquired `XHCI_LOCK`
- a transitional `poll >= 50` fallback that called the same locked helper
  without printing from the timer path

The attempted trigger point was in `kernel/src/main_aarch64.rs` immediately
after `/sbin/init` preload and before `timer_interrupt::init()`. At that point:

- `drivers::init()` had completed
- xHCI init had completed
- AHCI init had completed
- ext2 root/home mounts had completed
- process manager, scheduler, workqueue, and softirq init had completed
- `/sbin/init` had been preloaded from ext2
- CPU0 timer polling had not started, so the poll-50 fallback could not have won

This is the intended "system-ready but pre-timer" position.

## C. Build Verification

Both requested builds were clean.

Artifacts:

- `turn3-artifacts/build-aarch64.log`
- `turn3-artifacts/build-x86.log`
- `turn3-artifacts/build-efi.log`

Commands:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

`grep -E "^(warning|error)"` was empty for both build logs.

## D. Runtime Evidence

Artifacts:

- `turn3-artifacts/parallels-boot-newpath.log`
- `turn3-artifacts/parallels-boot-newpath-grep.txt`
- `turn3-artifacts/prlctl-after-boot.txt`
- `turn3-artifacts/prlctl-stop.txt`
- `turn3-artifacts/run-parallels-xtrace.log`

The first background `run.sh --parallels` launch exited before the build steps.
Running under `bash -x ./run.sh --parallels` produced the actual VM boot and
serial capture. The VM name for the real run was `breenix-1779251152`.

The system-ready activation did fire, before timer init and before the CPU0
regression alarm:

```text
295 [boot] Pre-loading /sbin/init from ext2 (before timer)...
296 [boot] Init binary pre-loaded: 296776 bytes
297 [xhci] activate_msi_if_ready: source=system-ready spi=56 DIAG_SPI_ENABLE_COUNT=1
298 [boot] Initializing timer interrupt...
```

The boot then reached userspace and ran for about 45 seconds before the existing
CPU0 timer regression panic:

```text
327 [boot] Launching init from pre-loaded ELF...
341 [init] Breenix init starting (PID 1)
343 [freeze-watch] uptime_ms=1738 ... timer_ticks_cpu0=5 timer_ticks_cpu1=337 ...
373 [freeze-watch] uptime_ms=45301 ... timer_ticks_cpu0=5 timer_ticks_cpu1=29644 ...
375 !!! CPU0 REGRESSION ALARM !!!
376 CPU0 tick_count = 5, max peer = 30000
382 panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:
383 CPU0 timer regression: tick_count=5 but peer max=30000
```

This proves the new path beat the timer fallback: the success marker appears
before `timer_interrupt::init()`, so the `poll >= 50` path could not have fired
first.

The missing gate is MSI event proof. The serial log contains no
`MSI_EVENT_COUNT` line and no other direct evidence that xHCI `handle_interrupt()`
processed an event after the SPI was enabled. The grep artifact confirms the
activation marker, xHCI enumeration, and the later CPU0 regression panic, but
not `MSI_EVENT_COUNT > 0`.

## E. Pass Criteria Checklist

- Build aarch64 clean: PASS
- Build x86 clean: PASS
- System-ready activation marker before timer fallback: PASS
- `DIAG_SPI_ENABLE_COUNT >= 1`: PASS (`DIAG_SPI_ENABLE_COUNT=1` at serial line 297)
- No new kernel panic introduced: FAIL/INCONCLUSIVE. The pre-existing CPU0 timer
  regression still panics at `timer_interrupt.rs:598`; this run does not isolate
  whether the xHCI patch changed panic timing.
- `MSI_EVENT_COUNT >= 1`: FAIL. No serial evidence showed MSI events processed.
- Fallback fires at most once and ideally never: INCONCLUSIVE. The activation
  marker proves system-ready won, but there is no separate fallback counter
  print after the source edits were reverted.

## F. Conclusion

The attempted fix achieved half of the goal: xHCI SPI activation no longer
depended on CPU0 timer polling, and it happened at a safer point than the
reverted `703db7de` end-of-init attempt.

The full gate failed because runtime evidence did not show `MSI_EVENT_COUNT > 0`.
The code change was therefore reverted and is not committed.

Recommended next investigation:

1. Add a non-hot-path way to read or print xHCI diagnostic counters after
   system-ready activation, without touching prohibited timer/syscall paths.
2. Determine whether the lack of `MSI_EVENT_COUNT` is real MSI non-delivery,
   no post-activation xHCI events before CPU0 panic, or simply missing serial
   visibility for an already-incremented counter.
3. If Claude wants another implementation turn, keep the system-ready trigger
   location but add evidence plumbing that can prove `MSI_EVENT_COUNT` without
   weakening the runtime gate.
