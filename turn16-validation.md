# Turn 16 Validation

Status: COMPLETE

## Source Cleanup

Deleted the dormant VirtIO PCI input polling block from `kernel/src/drivers/virtio/input_pci.rs`.

Caller verification artifact: `turn16-artifacts/input-pci-caller-search.txt`.

Result: no `input_pci::` or `drivers::virtio::input_pci` caller exists in `kernel/src`, and no `input_pci.rs` poll function remains. The remaining `poll_events()` matches are unrelated live paths such as VirtIO MMIO input's internal IRQ-driven drain.

## Inventory and Planning Docs

Updated `turn1-polling-inventory.md` P5:

- VirtIO MMIO input is already IRQ-driven via `input_mmio::handle_interrupt()`.
- EHCI keyboard remains deferred because it needs IRQ infrastructure.
- Dormant VirtIO PCI input polling was deleted in this turn.

Added:

- `turn16-p6-p18-survey.md`
- `linux-profile-virtgpu-debug-watchdog.md`
- `linux-profile-artifacts/virtgpu-watchdog-source-refs.txt`

Chosen next target: P10, the GPU PCI freeze watchdog. It is a periodic diagnostic sampler, while GPU PCI command completion is already IRQ-driven through `GPU_COMPLETION`.

## Build

Command:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

Result: PASS.

Artifacts:

- `turn16-artifacts/build-x86.log`
- `turn16-artifacts/build-x86-warning-error-grep.txt`

Warning/error grep output is empty.

## Fresh-Deploy Boot

Command:

```text
./run.sh --parallels --test 60
```

Result: PASS.

Artifacts:

- `turn16-artifacts/boot-1-run.out`
- `turn16-artifacts/boot-1-serial.log`
- `turn16-artifacts/boot-1-key-markers.txt`
- `turn16-artifacts/boot-1-final-serial-tail.txt`
- `turn16-artifacts/boot-1-run-warning-error-grep.txt`
- `turn16-artifacts/boot-1-screenshot.png`

Pass evidence:

- Heartbeat reached `uptime_ms=74301`.
- CPU0 timer ticks advanced through `45000`.
- `bwm-fps` and `virgl-composite` continued through the test window.
- No `CPU0 timer regression`, `END SOFT LOCKUP DUMP`, `KERNEL PANIC`, or `panic` lines were found.
- Boot run warning/error grep output is empty for compile-stage warnings/errors.

The Parallels screenshot helper printed that no matching window was found, then still wrote `/tmp/breenix-screenshot.png`; this did not affect serial-based pass criteria.

## Cleanup

Stopped and deleted the temporary Parallels VM `breenix-1779304633`.

QEMU cleanup command was run after validation and reported all QEMU processes killed.
