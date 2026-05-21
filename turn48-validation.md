# Turn 48 Validation: P15 PCI PM D3hot->D0 Settle Delay Allowlist

## Scope

- Added `docs/polling-allowlist.md` documenting the Linux-rigor polling-elimination allowlist.
- Replaced the inline PCI PM delay comment in `kernel/src/drivers/pci.rs`.
- Runtime behavior unchanged: the existing bounded `10_000_000u64` spin remains identical.

## Static Checks

- Verified the `10_000_000u64` spin is unique in `kernel/src/drivers/pci.rs`.
- `git diff --check docs/polling-allowlist.md kernel/src/drivers/pci.rs` produced no output.

## Builds

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All builds completed successfully. Warning/error grep artifacts for aarch64, x86, and EFI builds are empty.

## Parallels Boot Gate

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=72227`.
- CPU0 timer ticks reached `45000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.

## Artifacts

- `turn48-artifacts/source-diff.txt`
- `turn48-artifacts/source-diff-stat.txt`
- `turn48-artifacts/build-userspace.log`
- `turn48-artifacts/build-ext2.log`
- `turn48-artifacts/build-aarch64.log`
- `turn48-artifacts/build-x86.log`
- `turn48-artifacts/build-efi.log`
- `turn48-artifacts/boot-parallels/boot-1-serial.log`
- `turn48-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn48-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn48-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn48-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn48-artifacts/boot-parallels/live-ping.txt`
- `turn48-artifacts/boot-parallels/live-arp.txt`
