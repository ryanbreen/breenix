# Turn 31 Validation

Status: COMPLETE

## 31A Loop Location

Documented in `turn31-artifacts/loop-location.md`.

Pre-edit location:

- function: `send_ipv4()` in `kernel/src/net/mod.rs`
- line range: `kernel/src/net/mod.rs:638-665`
- loop: `for _ in 0..50`
- per-iteration behavior: `process_rx()`, spin `0..500_000` with
  `core::hint::spin_loop()`, then retry `arp::lookup(&next_hop)`

## 31B Source Change

Chose Option A: drop on ARP miss.

The new `send_ipv4()` contract is asynchronous for neighbor resolution:

- on ARP cache miss, send an ARP request
- do not synchronously poll RX
- return `Err("ArpMiss: reply will populate cache via IRQ")`
- callers or higher layers should retry naturally

Also updated stale comments that referenced the removed ARP polling loop.

File scope confirmation: source changes touched only `kernel/src/net/mod.rs`.

Saved diff:

- `turn31-artifacts/source-diff-stat.txt`
- `turn31-artifacts/source-diff.txt`

Diff stat: 1 file changed, 12 insertions, 25 deletions.

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn31-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn31-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

Single fresh Parallels boot passed:

- `turn31-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- `turn31-artifacts/boot-1-on-demand-polling-scan.txt`: 0 bytes
- `turn31-artifacts/boot-1-on-demand-polling-after-probes-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=66213` in the 60 second run
- after the live ping probe, heartbeat reached `uptime_ms=127245`
- after the live ping probe, CPU0 timer reached `ticks=90000`
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and printed `[bounce] Window mode`

Required Turn 30 markers were preserved:

- line 268: `NET: Network initialization complete`
- line 269: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
- line 270: `NET: pre-primed NetRx softirq for bootstrap callback re-enable`

Expected Substep 5 behavior appeared:

- line 266: `NET: ARP cache miss for 10.211.55.1, sending ARP request`
- line 267: `NET: Failed to send ping: ArpMiss: reply will populate cache via IRQ`
- init continued after the `ArpMiss`

## External Ping Result

Live host ping to the guest passed on the same boot:

- command: `ping -c 1 -W 2000 10.211.55.100`
- result: `1 packets transmitted, 1 packets received, 0.0% packet loss`
- guest MAC learned by host ARP: `00:1c:42:4a:35:1c`

Artifacts:

- `turn31-artifacts/live-ping.txt`
- `turn31-artifacts/live-arp.txt`
- `turn31-artifacts/boot-1-serial-after-probes.log`

## Verdict

COMPLETE.

The synchronous on-demand ARP polling loop is gone. Init-time outbound ping now
reports `ArpMiss` without blocking, while the critical inbound IRQ/NetRx path
continues to work: external host ping succeeds after boot.
