# Turn 33 Validation

Status: COMPLETE

## Source Changes

Touched only the allowed source files:

- `kernel/src/main.rs`
- `kernel/src/net/mod.rs`

Re-applied the Turn 32 x86 hlt-loop cleanup:

- deleted 3 `net::process_rx()` calls in x86 test idle loops
- deleted the 3 stale `Poll for received packets (workaround for softirq timing)` comments
- kept all 3 `net::drain_loopback_queue()` calls

Hardened the Turn 30 bootstrap in `init_common()`:

- after aarch64 PCI MSI-X enable, call `net_pci::reenable_and_check_race()`
  synchronously when PCI net is initialized
- log `NET: synchronously cleared virtio callback suppression`
- keep the existing `SoftirqType::NetRx` raise as a redundant path

Saved diff:

- `turn33-artifacts/source-diff-stat.txt`
- `turn33-artifacts/source-diff.txt`

Diff stat: 2 files changed, 8 insertions, 16 deletions.

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn33-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn33-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

Single fresh Parallels boot passed:

- `turn33-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=66230` in the 60 second run
- after the live ping probe, heartbeat reached `uptime_ms=103250`
- after the live ping probe, CPU0 timer reached `ticks=70000`
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and printed `[bounce] Window mode`

Substep 4-5 markers were preserved:

- line 266: `NET: ARP cache miss for 10.211.55.1, sending ARP request`
- line 267: `NET: Failed to send ping: ArpMiss: reply will populate cache via IRQ`
- line 268: `NET: Network initialization complete`
- line 269: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
- line 271: `NET: pre-primed NetRx softirq for bootstrap callback re-enable`

New Turn 33 hardening marker was present:

- line 270: `NET: synchronously cleared virtio callback suppression`

## External Ping Result

Live host ping to the guest passed on the same boot:

- command: `ping -c 1 -W 2000 10.211.55.100`
- result: `1 packets transmitted, 1 packets received, 0.0% packet loss`
- guest MAC learned by host ARP: `00:1c:42:51:ce:32`

Artifacts:

- `turn33-artifacts/live-ping.txt`
- `turn33-artifacts/live-arp.txt`
- `turn33-artifacts/boot-1-serial-after-probes.log`

## X86 Runtime Note

x86 build is clean. The existing x86 runtime workflows were not run in this
turn; runtime verification remains the operator's x86 test workflow.

## Verdict

COMPLETE.

The synchronous callback clear removes the bootstrap timing dependency observed
in Turn 32, while the x86 hlt-loop NIC polling deletion is now included. The
single aarch64 boot retained all health markers and external ping succeeded.
