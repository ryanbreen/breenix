# Turn 49 Validation: P11 VirtIO Reset Status Handshake Allowlist

## Scope

- Appended the P11 VirtIO reset status handshake entry to `docs/polling-allowlist.md`.
- Expanded the inline comment above the VirtIO reset status loop in `kernel/src/drivers/virtio/mod.rs`.
- Runtime behavior unchanged: the existing reset loop and bounds remain identical.

## Static Checks

- Verified the existing reset loop in `VirtioDevice::init()`.
- `git diff --check docs/polling-allowlist.md kernel/src/drivers/virtio/mod.rs` produced no output.
- Source diff artifact is scoped to only:
  - `docs/polling-allowlist.md`
  - `kernel/src/drivers/virtio/mod.rs`

## Builds

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All builds completed successfully. Warning/error grep artifacts for aarch64 and x86 builds are empty; EFI build also produced no warning/error lines.

## Parallels Boot Gate

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=149300`.
- CPU0 timer ticks reached `90000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.

## Artifacts

- `turn49-artifacts/source-diff.txt`
- `turn49-artifacts/source-diff-stat.txt`
- `turn49-artifacts/build-userspace.log`
- `turn49-artifacts/build-ext2.log`
- `turn49-artifacts/build-aarch64.log`
- `turn49-artifacts/build-aarch64-warning-error-grep.txt`
- `turn49-artifacts/build-x86.log`
- `turn49-artifacts/build-x86-warning-error-grep.txt`
- `turn49-artifacts/build-efi.log`
- `turn49-artifacts/boot-parallels/boot-1-run.out`
- `turn49-artifacts/boot-parallels/boot-1-serial.log`
- `turn49-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn49-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn49-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn49-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn49-artifacts/boot-parallels/live-ping.txt`
- `turn49-artifacts/boot-parallels/live-arp.txt`
