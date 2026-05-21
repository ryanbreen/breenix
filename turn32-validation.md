# Turn 32 Validation

Status: INCONCLUSIVE

## 32A Audit Findings

Documented in `turn32-artifacts/x86-hlt-poll-sites.md`.

Found exactly three x86-only idle-loop NIC polling workarounds in
`kernel/src/main.rs`:

- `dns_test_only_main()`, pre-edit range `kernel/src/main.rs:805-815`
- `blocking_recv_test_main()`, pre-edit range `kernel/src/main.rs:871-881`
- `nonblock_eagain_test_main()`, pre-edit range `kernel/src/main.rs:940-950`

Each site had the same shape:

- `x86_64::instructions::interrupts::enable_and_hlt()`
- `task::scheduler::yield_current()`
- stale comment: `Poll for received packets (workaround for softirq timing)`
- `net::process_rx()`
- `net::drain_loopback_queue()`

The `net::process_rx()` calls were NIC polling workarounds. The
`net::drain_loopback_queue()` calls are loopback-specific and were kept in the
attempted diff.

## 32B Attempted Source Change

Attempted source change touched only `kernel/src/main.rs`.

The attempted diff deleted:

- 3 `net::process_rx()` calls
- 3 stale `Poll for received packets (workaround for softirq timing)` comments

Saved attempted diff:

- `turn32-artifacts/source-diff-stat.txt`
- `turn32-artifacts/source-diff.txt`

Attempted diff stat: 1 file changed, 9 deletions.

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep files:

- `turn32-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn32-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Boot Result

Single fresh Parallels boot was structurally healthy:

- `turn32-artifacts/boot-1-fail-marker-scan.txt`: 0 bytes
- heartbeat reached `uptime_ms=75266` during the 60 second run
- after the live ping probe, heartbeat reached `uptime_ms=107283`
- after the live ping probe, CPU0 timer reached `ticks=60000`
- bwm/compositor started
- bsshd listened on port 2222 and init printed `[init] bsshd started`
- bounce started and printed `[bounce] Window mode`

Substep 4-5 markers were preserved:

- line 266: `NET: ARP cache miss for 10.211.55.1, sending ARP request`
- line 267: `NET: Failed to send ping: ArpMiss: reply will populate cache via IRQ`
- line 268: `NET: Network initialization complete`
- line 269: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
- line 270: `NET: pre-primed NetRx softirq for bootstrap callback re-enable`

## External Ping Result

Live host ping to the guest failed on the same boot:

- command: `ping -c 1 -W 2000 10.211.55.100`
- result: `1 packets transmitted, 0 packets received, 100.0% packet loss`
- host ARP learned guest MAC `00:1c:42:e9:05:4d`

Because the directive lists external ping failure as a fail criterion when the
probe is run, this turn is INCONCLUSIVE even though the source change is
x86-only and the aarch64 boot itself stayed healthy.

## Revert

Reverted `kernel/src/main.rs` after preserving the attempted diff and runtime
evidence. No Turn 32 source change remains in the working tree.

Artifact:

- `turn32-artifacts/source-reverted.txt`

## Verdict

INCONCLUSIVE.

The x86 polling deletion built cleanly and the aarch64 boot remained healthy,
but the live external ping failed on the single boot. Per the directive, the
source change was reverted and this commit is diagnostic-only.
