# Turn 9 Fix Attempted: P5 Input Polling Elimination

Status: INCONCLUSIVE / blocked by baseline fresh-deploy regression.

## Attempted Source Change

The attempted P5 conversion was implemented and built cleanly, then reverted after validation failed. The full reverted patch is saved at:

- `turn9-artifacts/source-attempt.diff`

The attempted patch covered:

- VirtIO MMIO input used-ring drain renamed from public polling API to private IRQ-side drain helper.
- VirtIO MMIO input IRQ/event/key-byte counters.
- EHCI keyboard qTD drain moved behind an EHCI IRQ handler with MSI/GICv2m setup.
- EHCI IRQ/completion/error/key-byte counters.
- EHCI IRQ dispatch from the aarch64 SPI path.
- Removal of dormant `virtio/input_pci.rs` timer-oriented polling block.
- `/proc/xhci/counters` extension for input evidence.
- A late userspace `xhci_counters` spawn to capture post-injection input counters.

## Build Result

The attempted source built without warnings or errors:

- `turn9-artifacts/build-userspace.log`
- `turn9-artifacts/build-ext2.log`
- `turn9-artifacts/build-aarch64.log`
- `turn9-artifacts/build-x86.log`
- `turn9-artifacts/build-efi.log`

`grep -E "^(warning|error)" turn9-artifacts/build-*.log` produced no output.

## Validation Refutation

The first 20-boot stress attempt used `run.sh --parallels --no-build` and initially booted a stale Parallels HDD. Evidence: `/bin/xhci_counters` printed only the original three xHCI counters even though the attempted kernel binary contained the new counter strings. `scripts/parallels/build-efi.sh --kernel` had refreshed `target/parallels/breenix-efi.img`, but `--no-build` boots `target/parallels/breenix-efi.hdd`.

After refreshing the Parallels HDDs, the attempted source failed a fresh boot before the input gate could run:

- Log: `turn9-artifacts/runsh-predeploy-smoke-serial.log`
- PID 1 started, then stalled while spawning `/bin/heartbeat`.
- `timer_ticks_cpu0` stayed at 5-6 while peer CPUs advanced past 27k ticks.
- The CPU0 regression guard panicked at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598`.

The attempted source was then reverted and the same fresh-deploy smoke was run on branch tip `f08c5328`:

- Log: `turn9-artifacts/baseline-f08c-fresh-smoke-serial.log`
- Same failure mode: PID 1 starts, stalls before init services, CPU0 timer ticks stay at 5, and the CPU0 regression guard panics.

That refutes the attempted P5 conversion as the cause of the CPU0 regression. The Turn 9 gate is blocked by a pre-existing fresh-deploy boot failure that stale `--no-build` Parallels HDDs had masked.

## Follow-Up Filed

Created Beads issue `breenix-oia`: fresh Parallels deploy trips CPU0 regression before init services.

## Source State

No Turn 9 source changes remain in the worktree. Only diagnostics/artifacts are intended for commit.
