# Turn 17 Validation

Status: COMPLETE

## Source Changes

Modified:

- `kernel/src/drivers/virtio/gpu_pci.rs`
- `kernel/src/main_aarch64.rs`
- `kernel/src/tracing/providers/virtgpu.rs`
- `turn1-polling-inventory.md`

Deleted from `gpu_pci.rs`:

- `FREEZE_WATCH_STARTED`
- `start_freeze_watchdog()`
- `freeze_watchdog_thread()`
- `current_cpu_id_for_watch()`
- `timer_tick_snapshot()`
- `process_manager_lock_status()`
- `gpu_pci_lock_status()`
- `freeze_watch_sleep_ms()`
- watchdog-only GPU PCI lock hold attribution state/helpers

Removed the `main_aarch64.rs` call that spawned the watchdog after timer init.

Deleted `virtgpu::freeze_watch_snapshot()` because its only caller was the watchdog.

Updated `turn1-polling-inventory.md` P10 to DONE.

## Observability Decision

Preserved:

- GPU progress counters already registered by the virtgpu trace provider and exposed through `/proc/trace/counters`, including submissions, completions, failures, wait timeouts, and resource-2 flush success.
- GPU command completion events and wait path tracing.
- Existing generic scheduler/timer/process observability such as `/proc/stat`, `/proc/[pid]/status`, and trace counters.

Deleted without replacement:

- Periodic `[freeze-watch]` serial lines.
- Periodic `[gpu-pci-lock-attrib]` serial lines.
- Watchdog-only process-manager lock, scheduler lock, GPU lock, and per-CPU timer snapshots.

Reason: those snapshots were diagnostic polling output, not command completion state. Keeping them would require a new proc surface for data that is not part of normal GPU operation. The durable GPU progress counters already exist on demand through the trace provider.

## GPU IRQ Completion Path

The GPU IRQ completion path was not changed. Reference artifact:

- `turn17-artifacts/gpu-completion-path-refs.txt`

It still includes:

- `GPU_COMPLETED_USED_IDX`
- `GPU_COMPLETION`
- `handle_interrupt()`
- `ctrlq_completion_token()`
- `prepare_ctrlq_completion_wait()`

## Source Scan

Artifact:

- `turn17-artifacts/source-freeze-watch-scan.txt`

Result: empty. No `freeze-watch`, `start_freeze_watchdog`, `freeze_watch`, `FREEZE_WATCH`, or `gpu-pci-lock-attrib` source references remain under `kernel/src`.

## Build Gates

Commands:

```text
./userspace/programs/build.sh --arch aarch64
./scripts/create_ext2_disk.sh --arch aarch64
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
scripts/parallels/build-efi.sh --kernel
```

Result: PASS.

Warning/error grep artifacts are all empty:

- `turn17-artifacts/build-userspace-warning-error-grep.txt`
- `turn17-artifacts/build-ext2-warning-error-grep.txt`
- `turn17-artifacts/build-aarch64-warning-error-grep.txt`
- `turn17-artifacts/build-x86-warning-error-grep.txt`
- `turn17-artifacts/build-efi-warning-error-grep.txt`

## Fresh-Deploy Boot

Command:

```text
./run.sh --parallels --test 60
```

Result: PASS.

Artifacts:

- `turn17-artifacts/boot-1-run.out`
- `turn17-artifacts/boot-1-serial.log`
- `turn17-artifacts/boot-1-key-markers.txt`
- `turn17-artifacts/boot-1-final-serial-tail.txt`
- `turn17-artifacts/boot-1-fail-marker-scan.txt`
- `turn17-artifacts/boot-1-run-warning-error-grep.txt`
- `turn17-artifacts/boot-1-screenshot.png`

Pass evidence:

- Heartbeat reached `uptime_ms=82311`.
- CPU0 timer ticks reached `50000`.
- `bwm-fps` continued through the test window.
- `virgl-composite` reached frame `13000`.
- `bsshd` started on port 2222.
- `bounce` started and was discovered by BWM.
- The only process exit in the log was expected `xhci_counters` completion.
- `turn17-artifacts/boot-1-fail-marker-scan.txt` is empty: no `freeze-watch`, `gpu-pci-lock-attrib`, `CPU0 timer regression`, soft-lockup, panic, or kernel panic markers.
- Boot run warning/error grep is empty for compile-stage warnings/errors.

The Parallels screenshot helper again printed that no matching window was found, then wrote `/tmp/breenix-screenshot.png`; serial-based pass criteria were unaffected.

## Cleanup

Stopped and deleted temporary Parallels VM `breenix-1779305322`.

QEMU cleanup reported `All QEMU processes killed`.
