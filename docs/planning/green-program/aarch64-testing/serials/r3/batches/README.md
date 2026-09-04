# Committed batches, aarch64 `testing` profile and the two other profiles

N18 (round-4 review): round 4 published 15 summary rows backed by 4 committed
`.txt` files, so 11 of its rows were assertions nobody could re-derive. Round 5
re-ran the whole batch and committed 17 raw serials for 17 rows. Each row below
is derived from the file named in its own row, by the commands printed under
the table.

```
$ find docs/planning/green-program/aarch64-testing/serials/r3/batches -name "*.txt" | wc -l
      17
```

17 files, 17 rows: 12 `testing` boots, 1 `boot_tests` boot, 2 no-feature boots,
and the 2 x86 runs. Round 4's `testing-lockup-boot6.txt` and
`testing-lockup-boot1.txt` were 2 partial specimens of a batch that committed 0
of its other 10 boots, against a fixture that is not reproducible from the
tree; 12 of the 12 boots of this batch are committed in full instead, so those
2 files are removed here and remain in history at `436c93f7`.

## How these boots were produced

The 15 aarch64 boots below were built and run at branch head `19f65895`.
`kernel/src` is byte-identical to round 4's head: `git diff --stat 436c93f7
HEAD -- kernel/` prints 0 lines. So these are round 4's kernels re-measured
against a fixture that can be rebuilt from this tree.

```
userspace/programs/build.sh --arch aarch64        # 148 aarch64 binaries
scripts/create_ext2_disk.sh --arch aarch64        # -> target/ext2-aarch64.img

cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64 [--features testing | --features boot_tests | ]

qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
  -kernel <kernel-aarch64> \
  -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device \
  -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 \
  -drive if=none,id=ext2,format=raw,file=<fresh copy of target/ext2-aarch64.img> \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 \
  -serial file:<out>
```

3 QEMUs at a time, 45s each, a fresh copy of the fixture per boot.
`scripts/check-kernel-no-neon.sh` reported `PASS: 0 FP/SIMD load/store
instructions in kernel .text (allowlisted & suppressed: 0)` on 3 of the 3
kernels.

The fixture is not committed (256 MB). Its sha256 at run time was
`669bacc673c92d3b38f3e5b7393fcca01187ab014466ef7cd28cbd016bdd3546`.

**Disclosed narrowing.** The count line reads `Loaded 73/78 test binaries (0
failed, 5 not found)`. The 5 absent binaries are `hello_musl`, `env_musl_test`,
`uname_musl_test`, `rlimit_musl_test` and `identity_musl_test`: they come from
`userspace/c-programs`, which needs a musl libc under `third-party/musl-install`
that this worktree does not have, so `userspace/programs/build.sh --arch
aarch64` does not produce them. Round 4's fixture had a different 1 missing
(`tcp_cloexec_exec_test`) and its "78 of 78" sentence is withdrawn: it does not
reproduce against either fixture.

## `--features testing`, 12 boots

Marker = `[test] Test processes loaded - will run via timer interrupts`.
Lockup = `!!! SOFT LOCKUP DETECTED !!!`. Stall = `EXT2_LOCK_SPIN_STALL`.

| Boot | file | marker line | loaded | lockup line | stalls | of those, before the lockup | lines |
|---|---|---|---|---|---|---|---|
| 1 | `testing-boot1.txt` | 1810 | 73/78 | 3305 | 7 | 7 | 7619 |
| 2 | `testing-boot2.txt` | 1804 | 73/78 | 3353 | 8 | 8 | 7658 |
| 3 | `testing-boot3.txt` | 1809 | 73/78 | 3297 | 4 | 4 | 7604 |
| 4 | `testing-boot4.txt` | 1805 | 73/78 | 3189 | 7 | 7 | 7507 |
| 5 | `testing-boot5.txt` | 1806 | 73/78 | 3270 | 7 | 7 | 7588 |
| 6 | `testing-boot6.txt` | 1818 | 73/78 | 2284 | 2 | 2 | 2385 |
| 7 | `testing-boot7.txt` | 1819 | 73/78 | 3247 | 7 | 7 | 7566 |
| 8 | `testing-boot8.txt` | 1804 | 73/78 | 3269 | 7 | 7 | 7591 |
| 9 | `testing-boot9.txt` | 1813 | 73/78 | 3220 | 4 | 4 | 7542 |
| 10 | `testing-boot10.txt` | 1819 | 73/78 | 3251 | 7 | 7 | 7568 |
| 11 | `testing-boot11.txt` | 1811 | 73/78 | 3195 | 6 | 6 | 7512 |
| 12 | `testing-boot12.txt` | 1810 | 73/78 | 3216 | 6 | 6 | 7538 |

Row-derivation commands, run against each `testing-boot<N>.txt` at write time:

```
marker line   grep -an "Test processes loaded - will run via timer interrupts" <f> | head -1 | cut -d: -f1
loaded        grep -a "test binaries (" <f> | head -1
lockup line   grep -an "SOFT LOCKUP DETECTED" <f> | head -1 | cut -d: -f1
stalls        grep -ac "EXT2_LOCK_SPIN_STALL" <f>
before        grep -an "EXT2_LOCK_SPIN_STALL" <f> | cut -d: -f1 | awk -v L=<lockup> '$1 < L' | wc -l
lines         wc -l < <f>
```

Totals: 12 of 12 reached the marker, 12 of 12 loaded 73 of 78, 12 of 12 then
locked up, and in 12 of the 12 every `EXT2_LOCK_SPIN_STALL` line came before the
lockup line (2 to 8 per boot). Round 4 saw 11 of 12 lock up; this batch saw 12
of 12, which is the same signature at a higher rate against a fixture with a
different binary set, not a new one.

`grep -ac IDLE_SLEEP_REFUSED` returns 0 on 12 of 12. Park-race counters print
once per boot: `WORKQUEUE_PARK_RACE:cancelled=0:intent_cleared=0` in 11 of 12
and `cancelled=1:intent_cleared=0` in boot 6; `KSOFTIRQD_PARK_RACE:cancelled=0`
in 12 of 12.

Boot 6 is the short one: it locked up at line 2284 of 2385 rather than around
3200 of 7500, and it is the 1 boot whose workqueue park race reported
`cancelled=1`.

## `--features boot_tests`, 1 boot

`boot_tests-boot1.txt`: `grep -ac "All boot tests passed!"` -> 1. 709 lines.

## No features, 2 boots

| File | pre-load line | `grep -ac "^\[heartbeat\]"` | lines |
|---|---|---|---|
| `nofeatures-boot1.txt` | `[boot] Init binary pre-loaded: 298704 bytes` | 45 | 697 |
| `nofeatures-boot2.txt` | `[boot] Init binary pre-loaded: 298704 bytes` | 45 | 637 |

This profile matters because the init launch moved onto the boot continuation.
The 298704-byte figure is this fixture's `/sbin/init`; round 4's 298616 was a
different build of it.

## x86 `testing,external_test_bins`, 2 runs

Unchanged from round 4, and both files are the ones those rows were derived
from. `./docker/qemu/run-boot-parallel.sh 1` under xvfb on beast
(`breenix-x86`, `/root/breenix-a64r2` at `ad455130`,
`BREENIX_RUST_FORK_LIBRARY` set, forced clean by touching
`kernel/src/task/scheduler.rs`). Both runs were scored FAIL by
`scripts/x86-gate-verdict.sh`.

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
