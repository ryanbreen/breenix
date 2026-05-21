# Turn 28 Validation

Status: INCONCLUSIVE

Outcome: C - different failure

## 28A Loop Inventory

Recorded in `turn28-artifacts/loops-to-delete.txt`.

Deleted ranges from pre-edit `kernel/src/net/mod.rs`:

- gateway ARP polling loop: lines 375-401, `for _i in 0..100`
- ICMP reply polling loop: lines 422-429, `for _ in 0..20`

## 28B Source Diff

The attempted source diff touched only `kernel/src/net/mod.rs`:

- 37 deletions
- 0 insertions
- no changes to ARP send, ARP resolution check, ICMP send, IRQ-enable block,
  driver files, tracing, main boot code, or userspace

Saved in:

- `turn28-artifacts/source-diff-stat.txt`
- `turn28-artifacts/source-diff.txt`

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn28-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn28-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

The boot did not hit the CPU0 regression:

- `turn28-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=82226`
- CPU0 tick marker reached `50000`
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and BWM discovered the Bounce window

Network init failed differently:

- line 262: `NET: Sending ARP request for gateway 10.211.55.1`
- line 263: `ARP request sent successfully`
- line 264: `NET: Gateway ARP not resolved, skipping ping test`
- no `NET: Network initialization complete`
- no `MSI-X SPI ... enabled`
- no ICMP send marker

Live network probes after the 60s boot confirmed the functional failure:

- `ping -c 1 -W 1000 10.211.55.100`: 100% packet loss
- `ssh -p 2222 user@10.211.55.100`: timed out
- host ARP did learn the guest MAC: `00:1c:42:d1:51:f6`

## Interpretation

Turn 28 did isolate loop deletion alone. The result was not the CPU0 regression
seen in T22/T26/T27. Instead, the gateway ARP check immediately failed because
the loop removal gave the stack no receive-processing window before checking
`arp::lookup(&gateway)`.

Because the existing code returns on unresolved ARP, the original IRQ/MSI enable
block at the end of `init_common()` is never reached. The system remains alive,
but network RX is never enabled and bsshd is unreachable.

Conclusion: the polling loops are load-bearing for network bring-up in the
current control flow. They are not proven load-bearing for CPU0 stability by this
turn. A future fix must preserve the timing/receive window or restructure the
early return/IRQ-enable ordering deliberately; simple deletion is not viable.

Source was reverted after preserving the attempted diff. This commit is
diagnostic-only.
