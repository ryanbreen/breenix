# Turn 27 Validation

Status: INCONCLUSIVE

## 27A Spawn Point

There is no clean post-userspace kernel continuation after a successful
`launch_init_from_elf()` in `kernel/src/main_aarch64.rs`; the success path returns
`Ok(never)` and enters `match never {}`.

Because Turn 27's source allowlist did not include syscall/control-path files, I
did not add a userspace-triggered syscall. The attempted implementation instead
created a dormant `net_arp_primer` kthread at the latest available kernel slot:
after `[smp] 8 CPUs online`, after test-only exits, immediately before launching
`/sbin/init`. The kthread was supposed to wait 30 seconds and then run the actual
primer after userspace services had started.

See `turn27-artifacts/spawn-point.md` and
`turn27-artifacts/turn27-attempted.diff`.

## 27B Implementation Summary

The attempted diff reapplied Turn 26's polling-removal design:

- removed synchronous gateway ARP/ICMP polling from `net::init_common()`
- added `NET_ARP_PRIMER_RAN`
- added `net::spawn_arp_primer()`
- deferred the actual kthread work by 30 seconds before:
  1. incrementing `NET_ARP_PRIMER_RAN`
  2. enabling PCI MSI-X SPI or MMIO net IRQ
  3. sending one async gateway ARP request
  4. exiting

## Build

Clean:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

The ARM64 and x86 warning/error grep files are empty.

## Single Boot Result

FAIL before functional network checks.

Serial order:

- line 315: `[smp] 8 CPUs online`
- line 323: `[boot] deferred network primer scheduled (tid=10)`
- line 324: `[boot] Launching init from pre-loaded ELF...`
- line 338: `[init] Breenix init starting (PID 1)`
- line 339: first userspace service spawn, `/bin/heartbeat`
- line 341+: `CPU0 REGRESSION ALARM`

Failure marker:

- `CPU0 timer regression: tick_count=8 but peer max=30000`

Negative evidence:

- no `[net_arp_primer] running after post-userspace delay`
- no `MSI-X SPI ... enabled`
- no `NET_ARP_PRIMER_RAN`
- no bsshd/bounce markers

## Critical Conclusion

This run does confirm that the maximally deferred primer attempt did not survive,
but it does **not** prove that MSI-X enable itself is the trigger. The regression
hit before the dormant primer performed its delayed work, before
`NET_ARP_PRIMER_RAN`, and before any MSI-X/IRQ enable line.

The diagnostic signal is narrower: creating a dormant primer kthread before
userspace launch changed early post-EL0 scheduling enough to trip the CPU0 guard
before the experiment reached the intended MSI-X binary test point.

Source files were reverted after preserving the attempted diff. The commit is
diagnostic-only.
