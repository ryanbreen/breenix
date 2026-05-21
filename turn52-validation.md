# Turn 52 Validation: P17 SMP Secondary CPU Online Wait Allowlist

## Scope

- Appended the P17 SMP secondary CPU online wait entry to `docs/polling-allowlist.md`.
- Added an inline comment block immediately above the SMP wait loop in `kernel/src/main_aarch64.rs`.
- Runtime behavior unchanged: the existing `cpus_online()` loop and explicit timeout check are identical.

## Static Checks

- Verified `main_aarch64.rs` is outside the prohibited Tier-1 file list and outside gold-master regions.
- Verified the loop is bounded by the existing 100ms timeout.
- `git diff --check docs/polling-allowlist.md kernel/src/main_aarch64.rs` produced no output.
- Source diff artifact is scoped to only:
  - `docs/polling-allowlist.md`
  - `kernel/src/main_aarch64.rs`

## Builds

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All builds completed successfully. Warning/error grep artifacts for aarch64 and x86 builds are empty; EFI build also produced no warning/error lines.

## Parallels Boot Gate

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=115258`.
- CPU0 timer ticks reached `70000`.
- SMP evidence present:
  - GICR covers 8 redistributors.
  - PSCI CPU_ON succeeded for secondary CPUs.
  - Final marker: `[smp] 8 CPUs online`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.

## Artifacts

- `turn52-artifacts/source-diff.txt`
- `turn52-artifacts/source-diff-stat.txt`
- `turn52-artifacts/build-userspace.log`
- `turn52-artifacts/build-ext2.log`
- `turn52-artifacts/build-aarch64.log`
- `turn52-artifacts/build-aarch64-warning-error-grep.txt`
- `turn52-artifacts/build-x86.log`
- `turn52-artifacts/build-x86-warning-error-grep.txt`
- `turn52-artifacts/build-efi.log`
- `turn52-artifacts/boot-parallels/boot-1-run.out`
- `turn52-artifacts/boot-parallels/boot-1-serial.log`
- `turn52-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn52-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn52-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn52-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn52-artifacts/boot-parallels/boot-1-smp-evidence.txt`
- `turn52-artifacts/boot-parallels/live-ping.txt`
- `turn52-artifacts/boot-parallels/live-arp.txt`
