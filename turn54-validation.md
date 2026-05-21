# Turn 54 Validation

## Scope

- Source changes limited to:
  - `docs/polling-allowlist.md`
  - `kernel/src/drivers/ahci/mod.rs`
- AHCI source edits are comment-only.
- `docs/polling-allowlist.md` was append-only and preserves P15/P11/P16/P18/P17.
- Scoped source diff artifact: `turn54-artifacts/source-diff-stat.txt` shows exactly these two files.

## Build Gate

Commands run:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All commands completed successfully. Warning/error grep artifacts are 0 bytes:

- `turn54-artifacts/build-userspace-aarch64.warnerr`
- `turn54-artifacts/create-ext2-aarch64.warnerr`
- `turn54-artifacts/build-kernel-aarch64.warnerr`
- `turn54-artifacts/build-qemu-uefi.warnerr`
- `turn54-artifacts/build-efi.warnerr`

## Parallels Boot Gate

Single boot command:

- `./run.sh --parallels --no-build --test 65`

Pass evidence:

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=118241`.
- CPU0 timer ticks reached `70000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, `0.0% packet loss`.
- AHCI/ext2 sanity: `[ext2] Found ext2 superblock on AHCI device 1`.

## Artifacts

- `turn54-artifacts/source-diff-stat.txt`
- `turn54-artifacts/build-userspace-aarch64.log`
- `turn54-artifacts/create-ext2-aarch64.log`
- `turn54-artifacts/build-kernel-aarch64.log`
- `turn54-artifacts/build-qemu-uefi.log`
- `turn54-artifacts/build-efi.log`
- `turn54-artifacts/boot-parallels/boot-1-run.out`
- `turn54-artifacts/boot-parallels/boot-1-serial.log`
- `turn54-artifacts/boot-parallels/boot-1-summary.txt`
- `turn54-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn54-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn54-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn54-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn54-artifacts/boot-parallels/boot-1-smp-evidence.txt`
- `turn54-artifacts/boot-parallels/live-ping.txt`
- `turn54-artifacts/boot-parallels/live-arp.txt`

Result: COMPLETE/PASS.
