# Turn 51 Validation: P18 Completion Early-Boot Fallback Allowlist

## Scope

- Appended the P18 `Completion::wait_timeout()` early-boot fallback entry to `docs/polling-allowlist.md`.
- Expanded the inline comment above the pre-scheduler fallback branch in `kernel/src/task/completion.rs`.
- Runtime behavior unchanged: the existing CNTPCT deadline path and fallback loops are identical.

## Static Checks

- Verified the fallback branch is only used when `current_thread_id()` returns `None`.
- `git diff --check docs/polling-allowlist.md kernel/src/task/completion.rs` produced no output.
- Source diff artifact is scoped to only:
  - `docs/polling-allowlist.md`
  - `kernel/src/task/completion.rs`

## Builds

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All builds completed successfully. Warning/error grep artifacts for aarch64 and x86 builds are empty; EFI build also produced no warning/error lines.

## Parallels Boot Gate

- CPU0 regression scan: `0` bytes.
- Heartbeat reached `uptime_ms=99254`.
- CPU0 timer ticks reached `60000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.

## Artifacts

- `turn51-artifacts/source-diff.txt`
- `turn51-artifacts/source-diff-stat.txt`
- `turn51-artifacts/build-userspace.log`
- `turn51-artifacts/build-ext2.log`
- `turn51-artifacts/build-aarch64.log`
- `turn51-artifacts/build-aarch64-warning-error-grep.txt`
- `turn51-artifacts/build-x86.log`
- `turn51-artifacts/build-x86-warning-error-grep.txt`
- `turn51-artifacts/build-efi.log`
- `turn51-artifacts/boot-parallels/boot-1-run.out`
- `turn51-artifacts/boot-parallels/boot-1-serial.log`
- `turn51-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn51-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`
- `turn51-artifacts/boot-parallels/boot-1-cpu0-ticks.txt`
- `turn51-artifacts/boot-parallels/boot-1-gpu-network.txt`
- `turn51-artifacts/boot-parallels/live-ping.txt`
- `turn51-artifacts/boot-parallels/live-arp.txt`
