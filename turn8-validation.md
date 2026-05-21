# Turn 8 validation

## Build gates

Kernel:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

`turn8-artifacts/build-fix.log` completed successfully. Warning/error grep was empty.

Userspace, ext2 disk, and Parallels EFI image were rebuilt:

- `turn8-artifacts/build-userspace.log`
- `turn8-artifacts/build-ext2.log`
- `turn8-artifacts/build-efi.log`

Warning/error grep was empty for all three logs.

`cargo fmt --check` was attempted but is not a usable Turn 8 gate because it fails on pre-existing unrelated formatting/trailing-whitespace issues, including `tests/shared_qemu.rs`.

## Stress gate

Harness:

```text
bash turn8-artifacts/run_20boot_scheduler_gate.sh
```

Result:

```text
overall: pass
boots: 20 failures=0 data_abort_boots=0 max_msi_irq_delta=1
scheduler_stale_totals: not_ready=0 current=0 deferred=0
```

All 20 boots met the Turn 8 pass criteria:

- 0 DATA_ABORT boots
- `pid1=yes` in all boots
- xHCI MSI/IRQ counts present and within +/-2 in all boots
- compositor activity observed in all boots
- no panic/synchronous-exception failure markers

The full aggregate is in `turn8-artifacts/stress-20boot/aggregate-result.txt`; per-boot serial logs, counter extracts, screenshots, and result files are under `turn8-artifacts/stress-20boot/boot-*`.
