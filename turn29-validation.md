# Turn 29 Validation

Status: INCONCLUSIVE

## 29A Diff Baseline

Re-read `turn28-artifacts/source-diff.txt`. The preserved Turn 28 diff deleted
the two init-time polling loops in `kernel/src/net/mod.rs`:

- gateway ARP loop: `for _i in 0..100`, 100 calls to `process_rx()`
- ICMP reply loop: `for _ in 0..20`, 20 calls to `process_rx()`

## 29B Implementation Summary

Attempted source change touched only `kernel/src/net/mod.rs`.

The diff:

- deleted the same two polling loops as Turn 28
- made the first gateway ARP request non-fatal
- replaced the immediate unresolved-ARP early return with a non-fatal log
- made ICMP probe failure non-fatal
- kept `NET: Network initialization complete`
- kept `enable_msi_spi()` / `enable_net_irq()` at the end of `init_common()`

Saved diff:

- `turn29-artifacts/source-diff-stat.txt`
- `turn29-artifacts/source-diff.txt`

Diff stat: 1 file changed, 18 insertions, 49 deletions.

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn29-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn29-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

Kernel and userspace liveness passed:

- `turn29-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=81253`
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and printed `[bounce] Window mode`

The key Turn 29 markers were present:

- line 264: `NET: Gateway ARP not resolved during init; will resolve via IRQ path`
- line 267: `NET: On-demand ARP resolved gateway MAC`
- line 268: `NET: Network initialization complete`
- line 269: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`

So the Turn 28 early-return control-flow bug was fixed: init reached the end and
MSI-X enable ran.

Functional network still failed:

- `ping -c 1 -W 1000 10.211.55.100`: 100% packet loss
- `ssh -p 2222 user@10.211.55.100`: timed out
- host ARP learned the guest MAC: `00:1c:42:de:6f:5c`

## Interpretation

Turn 29 fixed the specific Turn 28 failure: `enable_msi_spi()` now runs and the
serial contains `Network initialization complete`.

However, live inbound networking is still silent. This is a deeper failure than
the Turn 28 early return. The boot proves loop deletion plus unconditional MSI-X
enable is CPU0-safe, but does not yet restore functional RX.

One important observation: the init-time `ping(gateway)` call triggered the
existing on-demand ARP path, which still has its own bounded `process_rx()` loop
at `net/mod.rs:682-696` in the post-revert source. That path resolved gateway
ARP before MSI-X enable. Despite that, live host ping/SSH still failed after
MSI-X was enabled.

Source was reverted after preserving the attempted diff. This commit is
diagnostic-only.
