# Turn 6 Validation

## Scope

- Source change limited to `kernel/src/arch_impl/aarch64/gic.rs`.
- Added a runtime split-deactivate gate:
  - VMware Fusion: `EOImode=0` single-step EOI+deactivate.
  - Parallels/non-VMware: `EOImode=1` split EOI/DIR preserved.
- `exception.rs` call sites unchanged.

## Build Gate

Artifacts are under:

`/Users/wrb/Downloads/Ralph/breenix-vmware-feature-parity-1779352066/turn6-artifacts`

Commands run:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

All warning/error grep artifacts are 0 bytes.

## VMware Boot Gate

Single boot artifact directory:

`/Users/wrb/Downloads/Ralph/breenix-vmware-feature-parity-1779352066/turn6-artifacts/vmware-boot-1`

Pass evidence:

- `EOImode=0 (single-step EOI+deactivate) - VMware path`
- `ICC_CTLR_EL1: 0x400 -> 0x400 (EOImode=0)`
- `GIC initialized (version 3)`
- `4 CPUs online`
- `Breenix ARM64 Boot Complete!`
- Reached userspace and ran `bcheck`.
- CPU0 timer ticks reached `70000`.
- No `EOR/DIR two-step deactivation is not supported` VMX panic.

Remaining VMware gap observed: e1000 TX timeout causes DNS/http checks to fail.

## Parallels Boot Gate

Single boot command:

- `./run.sh --parallels --test 65`

Pass evidence:

- `EOImode=1 (split EOI/DIR) - non-VMware path`
- CPU0 regression scan: 0 bytes.
- Heartbeat reached `uptime_ms=112298`.
- CPU0 timer ticks reached `75000`.
- T33 network markers present:
  - VirtIO GPU PCI initialized.
  - VirtIO-net MSI-X SPI 55 enabled post-init.
  - Callback suppression synchronously cleared.
  - NetRx softirq pre-primed.
- Live ping to `10.211.55.100`: 1 transmitted, 1 received, 0.0% packet loss.
- AHCI/ext2 sanity: `[ext2] Found ext2 superblock on AHCI device 1`.

Result: COMPLETE/PASS.
