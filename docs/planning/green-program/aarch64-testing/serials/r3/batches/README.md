# Committed batches, aarch64 `testing` profile and the two other profiles

N18: round 3 published batch rates backed by 0 committed serials, so
those numbers are withdrawn from the plan document. What follows is a batch
re-run at the round-4 head `ad455130` and committed here: one summary row per
boot, plus 2 lockup specimens in full.

The 15 aarch64 boots below (12 `testing`, 1 `boot_tests`, 2 with no
features) each used the soft-float kernel target and this QEMU line, 3 QEMUs at a time, 45s each, each boot on a fresh copy of
`target/ext2-aarch64.img`:

```
cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64 [--features testing | --features boot_tests | ]

qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
  -kernel target/aarch64-breenix-kernel/release/kernel-aarch64 \
  -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device \
  -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 \
  -drive if=none,id=ext2,format=raw,file=<copy> \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 \
  -serial file:<out>
```

The fixture is the same 256MB `ext2-aarch64.img` round 3 built, carried over
byte for byte; it contains `tcp_cloexec_exec_test`, which is what makes the
count line 78 of 78 rather than 77 of 78.

## `--features testing`, 12 boots

Marker = `[test] Test processes loaded - will run via timer interrupts`.
Lockup = `!!! SOFT LOCKUP DETECTED !!!`. Stall = `EXT2_LOCK_SPIN_STALL`.

| Boot | marker line | loaded | lockup line | stalls | of those, before the lockup | lines |
|---|---|---|---|---|---|---|
| 1 | 1904 | 78/78 | 3327 | 4 | 4 | 7657 |
| 2 | 1924 | 78/78 | 3525 | 4 | 4 | 7829 |
| 3 | 1924 | 78/78 | 3421 | 7 | 7 | 7745 |
| 4 | 1924 | 78/78 | 3368 | 4 | 4 | 7702 |
| 5 | 1924 | 78/78 | 3328 | 4 | 4 | 7662 |
| 6 | 1919 | 78/78 | 3464 | 9 | 9 | 7773 |
| 7 | 1924 | 78/78 | 3488 | 7 | 7 | 7816 |
| 8 | 1905 | 78/78 | 3326 | 7 | 7 | 7653 |
| 9 | 1924 | 78/78 | 3363 | 7 | 7 | 7694 |
| 10 | 1904 | 78/78 | no lockup | 0 | 0 | 2840 |
| 11 | 1904 | 78/78 | 3365 | 4 | 4 | 7691 |
| 12 | 1904 | 78/78 | 3380 | 7 | 7 | 7711 |

Totals: 12 of 12 reached the marker, 12 of 12 loaded 78 of 78, 11 of 12 then
locked up, and 11 of the 11 lockups had `EXT2_LOCK_SPIN_STALL` lines before
them (4 to 9 each). The 1 boot that did not lock up had 0 stalls.

`IDLE_SLEEP_REFUSED` appears 0 times in 12 of 12 boots. The park-race counters
reported `WORKQUEUE_PARK_RACE:cancelled=0:intent_cleared=1` in 1 of the 12 and
`cancelled=0:intent_cleared=0` in the other 11; `KSOFTIRQD_PARK_RACE:cancelled=0`
in 12 of 12.

2 of those boots are committed in full:

| File | Which row | Why this one |
|---|---|---|
| `testing-lockup-boot6.txt` | boot 6 | the highest stall count in the batch, 9 |
| `testing-lockup-boot1.txt` | boot 1 | the lowest stall count among the boots that locked up, 4 |

## `--features boot_tests`, 1 boot

1 of 1 printed `[boot] All boot tests passed!`
(`grep -a -c "All boot tests passed!"` -> 1). 860 lines.

## No features, 2 boots

2 of 2 printed `[boot] Init binary pre-loaded: 298616 bytes` and then went on
to run `/bin/heartbeat`: `grep -a -c "^\[heartbeat\]"` -> 46 and 45. This
profile matters because the init launch moved onto the boot continuation.

## x86 `testing,external_test_bins`, 2 runs

`./docker/qemu/run-boot-parallel.sh 1` under xvfb on beast (`breenix-x86`,
`/root/breenix-a64r2` at `ad455130`, `BREENIX_RUST_FORK_LIBRARY` set, forced
clean by touching `kernel/src/task/scheduler.rs`). Both runs were scored FAIL
by `scripts/x86-gate-verdict.sh`.

| File | `TEST_TALLY` |
|---|---|
| `x86-boot-parallel-run1.txt` | `exited=22 nonzero=2 failed=[simple_exit:42,/usr/local/test/bin/clon:1]` |
| `x86-boot-parallel-run2.txt` | `exited=22 nonzero=1 failed=[simple_exit:42]` |

`simple_exit:42` is the by-design nonzero exit the empty
`scripts/x86-gate-allowlist.txt` scores as a failure; it arrives on `main`
between `52491c4b` and this branch's base `bfbb7575`, and is filed as
https://github.com/ryanbreen/breenix/issues/781. The second entry in run 1,
`/usr/local/test/bin/clon`, is the truncated name of `clonevm_exec_test`; it
appeared in 1 of the 2 runs at the same commit and is filed separately as
https://github.com/ryanbreen/breenix/issues/782.
