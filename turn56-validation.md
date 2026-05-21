# Turn 56 Validation

## Scope

- Source changes limited to:
  - `docs/polling-allowlist.md`
  - `kernel/src/drivers/ahci/mod.rs`
- AHCI ISR source edit is comment-only and limited to a two-line inline comment.
- `docs/polling-allowlist.md` was append-only and preserves all prior P-entries.
- Scoped source diff artifact: `turn56-artifacts/source-diff-stat.txt` shows exactly these two files.

## Build Gate

Commands run:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All commands completed successfully. Warning/error grep artifacts are 0 bytes:

- `turn56-artifacts/build-userspace-aarch64.warnerr`
- `turn56-artifacts/create-ext2-aarch64.warnerr`
- `turn56-artifacts/build-kernel-aarch64.warnerr`
- `turn56-artifacts/build-qemu-uefi.warnerr`
- `turn56-artifacts/build-efi.warnerr`

## Parallels Boot Gate

Single boot command:

- `./run.sh --parallels --no-build --test 65`

Pass evidence:

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=152281`.
- CPU0 timer ticks reached `95000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, `0.0% packet loss`.
- AHCI/ext2 sanity: `[ext2] Found ext2 superblock on AHCI device 1`.
- AHCI ISR sanity: userspace boot and ext2 root reads continued through the AHCI interrupt-driven completion path.

## Artifacts

- `turn56-artifacts/source-diff-stat.txt`
- `turn56-artifacts/build-userspace-aarch64.log`
- `turn56-artifacts/create-ext2-aarch64.log`
- `turn56-artifacts/build-kernel-aarch64.log`
- `turn56-artifacts/build-qemu-uefi.log`
- `turn56-artifacts/build-efi.log`
- `turn56-artifacts/boot-parallels/boot-1-run.out`
- `turn56-artifacts/boot-parallels/boot-1-serial.log`
- `turn56-artifacts/boot-parallels/boot-1-summary.txt`
- `turn56-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn56-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn56-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn56-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn56-artifacts/boot-parallels/boot-1-smp-evidence.txt`
- `turn56-artifacts/boot-parallels/live-ping.txt`
- `turn56-artifacts/boot-parallels/live-arp.txt`

Result: COMPLETE/PASS.
