# Turn 30 Validation

Status: COMPLETE

## 30A Source Change Summary

Source change touched only `kernel/src/net/mod.rs`.

The diff reapplies the Turn 29 init restructure and adds the Turn 30 bootstrap:

- deleted the gateway ARP init polling loop
- deleted the ICMP reply init polling loop
- made the initial ARP request non-fatal
- made the init-time ICMP probe failure non-fatal
- kept `init_common()` running through `NET: Network initialization complete`
- kept aarch64 IRQ enable at the end of init
- added one `SoftirqType::NetRx` raise after `enable_msi_spi()` / `enable_net_irq()`
- logged `NET: pre-primed NetRx softirq for bootstrap callback re-enable`

Saved diff:

- `turn30-artifacts/source-diff-stat.txt`
- `turn30-artifacts/source-diff.txt`

Diff stat: 1 file changed, 16 insertions, 48 deletions.

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn30-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn30-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

Single fresh Parallels boot passed:

- `turn30-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=59225` during the 60 second run, and `uptime_ms=120258` after the live ping probe
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and printed `[bounce] Window mode`

Required Turn 29/30 network markers were present:

- line 264: `NET: Gateway ARP not resolved during init; will resolve via IRQ path`
- line 267: `NET: On-demand ARP resolved gateway MAC`
- line 268: `NET: Network initialization complete`
- line 269: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
- line 270: `NET: pre-primed NetRx softirq for bootstrap callback re-enable`

## Functional Network Result

Live host ping to the guest passed on the same boot:

- command: `ping -c 1 -W 2000 10.211.55.100`
- result: `1 packets transmitted, 1 packets received, 0.0% packet loss`
- guest MAC learned by host ARP: `00:1c:42:67:14:43`

Artifacts:

- `turn30-artifacts/live-ping.txt`
- `turn30-artifacts/live-arp.txt`
- `turn30-artifacts/boot-1-serial-after-probes.log`

## Verdict

COMPLETE.

The bootstrap NetRx raise broke the chicken-and-egg condition observed in Turn 29:
with init polling removed and MSI-X enabled at the end of `init_common()`, the
network is now live after boot. This supports the hypothesis that callbacks were
left suppressed until the first NetRx softirq dispatch cleared them via the
existing completion path.
