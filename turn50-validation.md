# Turn 50 Validation: P16 GICR_WAKER ChildrenAsleep Handshake Allowlist

## Scope

- Appended the P16 GICR_WAKER ProcessorSleep / ChildrenAsleep entry to `docs/polling-allowlist.md`.
- Expanded the inline comment above the GICR_WAKER bounded spin in `kernel/src/arch_impl/aarch64/gic.rs`.
- Runtime behavior unchanged: the existing `for _ in 0..10_000` loop is identical.

## Tier-2 / Gold-Master Check

- `gic.rs` is a Tier-2 file, so the edit was limited to explanatory comments.
- The GICR_WAKER spin is upstream of the gold-master SGI enable block.
- The gold-master region starts later in `init_gicv3_redistributor()` at the `GICR_ISENABLER0` SGI admission enable write and was not modified.

## Static Checks

- `git diff --check docs/polling-allowlist.md kernel/src/arch_impl/aarch64/gic.rs` produced no output.
- Source diff artifact is scoped to only:
  - `docs/polling-allowlist.md`
  - `kernel/src/arch_impl/aarch64/gic.rs`

## Builds

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All builds completed successfully. Warning/error grep artifacts for aarch64 and x86 builds are empty; EFI build also produced no warning/error lines.

## Parallels Boot Gate

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=62216`.
- CPU0 timer ticks reached `35000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.

## Artifacts

- `turn50-artifacts/source-diff.txt`
- `turn50-artifacts/source-diff-stat.txt`
- `turn50-artifacts/build-userspace.log`
- `turn50-artifacts/build-ext2.log`
- `turn50-artifacts/build-aarch64.log`
- `turn50-artifacts/build-aarch64-warning-error-grep.txt`
- `turn50-artifacts/build-x86.log`
- `turn50-artifacts/build-x86-warning-error-grep.txt`
- `turn50-artifacts/build-efi.log`
- `turn50-artifacts/boot-parallels/boot-1-run.out`
- `turn50-artifacts/boot-parallels/boot-1-serial.log`
- `turn50-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn50-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn50-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn50-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn50-artifacts/boot-parallels/live-ping.txt`
- `turn50-artifacts/boot-parallels/live-arp.txt`
