# Aarch64 testing-profile revival — 2026-09-04

This fix-forward revives the soft-float aarch64 `testing` boot from Breenix
`bfbb7575`. It addresses the #562 softirq self-test panic, the #761 ext2 test
loader continuation loss, and incorporates the five musl BTRT entries from
Lane B (`4d2a151e` and `f1711505`).

## #562: local softirq work and a migratory daemon

The deferred softirq bitmap is per-CPU. The old implementation published one
global, unpinned `ksoftirqd`; wakeup placement could migrate it to a secondary
CPU, where it read that CPU's empty bitmap instead of CPU 0's pending work.
Meanwhile, the aarch64 boot continuation deliberately held CPU 0's preemption
guard until after self-tests. The self-test therefore could not schedule the
daemon on the CPU whose bitmap it had raised. IRQ-exit draining completed 25
iterations, but the test's daemon-identity assertion remained false.

The fix publishes a lock-free daemon handle for each supported CPU, starts one
daemon for each online CPU with a production CPU affinity, and wakes the local
CPU's handle. Thread affinity is preserved through scheduler insertion,
wakeup, stealing decisions, cloning, and testing-profile fork construction.
The aarch64 test-assumption repair runs the existing workload in a schedulable
CPU-1-affine kthread and joins it. Its iteration-count and daemon-identity
assertions remain present.

`kernel/src/task/kthread.rs` is a Tier-2 file. Its change was required because
the CPU-targeted creator had been test-only, while the repaired production
softirq topology needs CPU-targeted workers. Observation through tracing could
show the wrong-CPU bitmap reads but could not supply that missing production
primitive. The entry path gained 0 logging calls, allocations, or blocking
locks.

The soft-float `--features testing` runtime boot recorded:

```text
SOFTIRQ_TEST: iteration limit passed (25 total iterations, ksoftirqd/1)
[test] Loading test binaries from ext2...
```

## #761: an idle identity was not a sleepable continuation

The loader ran as CPU 0's boot continuation while the scheduler represented
that CPU with its idle thread. With interrupts masked, VirtIO block treated a
present thread ID as enough to choose IRQ completion. `Completion` then treated
the boot preemption pin as a syscall sleep bracket, removed the pin, marked the
idle identity blocked, and called the kernel scheduler. Redispatch of an idle
identity resumes the canonical idle loop, not the saved loader call. The
VirtIO request completed and published its token, but the loader stack was
abandoned while it held the request and ext2 guards.

The completion predicate now requires enabled interrupts, thread context
outside interrupt/softirq handling, `preempt_count == 1`, and a non-idle
scheduler identity. VirtIO block rejects unavailable IRQ completion before
request publication. The aarch64 testing loader explicitly restores interrupts,
reads the ELF batch before process publication, and stages the created test
threads on CPU 0 until it emits the loader completion marker. The fixture
builder also disables ext2 `dir_index`, matching the kernel reader's current
linear-directory support for the enlarged catalog.

The runtime boot at `6b885352` used the soft-float kernel target and a writable
copy of the full ext2 fixture:

```text
[test] Loaded 78/78 test binaries (0 failed, 0 not found)
[test] Test processes loaded - will run via timer interrupts
```

The same serial captured 78 exact per-program load markers and 78 distinct
program names before the completion marker.

## Lane B: musl programs are now scored

The merge commit incorporates five additive catalog entries. Each program's
own exit code is the BTRT result criterion. The clean full-catalog boot emitted
five distinct passing KTAP records, summarized as:

```text
MUSL_BTRT_TALLY: passed=5 failed=0 total=5
```

A scratch `hello_musl` build changed its exit from 0 to 7. A five-program
fixture reached `===BTRT_READY===` and produced:

```text
not ok 378 utest_hello_musl # FAIL error_code=2 detail=0x7
# 19 passed, 1 failed, 90 skipped
MUSL_BTRT_MUTATION: passed=4 failed=1 total=5 failed=[utest_hello_musl:0x7]
```

The mutation ELF was then replaced with the byte-identical clean artifact
(SHA-256
`0146a714ec08841aa8b9e852d37549738aea3297a722e97c1753b8e35baccb34`),
and the full ext2 fixture was regenerated.

## Validation

The final source-bearing tip was `6b885352`. The three aarch64 commands used
`aarch64-breenix-kernel.json`, `-Z build-std=core,alloc`, and
`-Z build-std-features=compiler-builtins-mem`.

| Profile | Build result | Soft-float guard |
|---|---|---|
| aarch64 `testing` | `Finished release profile [optimized] target(s) in 5.56s` | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| aarch64 `boot_tests` | `Finished release profile [optimized] target(s) in 6.04s` | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| aarch64, no features | `Finished release profile [optimized] target(s) in 5.51s` | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| x86 `testing,external_test_bins` on Beast | `Finished release profile [optimized] target(s) in 19.15s` | not applicable |

Each build log contained 0 source warning/error lines. The local checkout
does not contain `rust-fork/library`, so the x86 build used an isolated Beast
checkout with `BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library`.

The requested structure-test glob was compiled directly with `rustc --test`
to avoid the repository build script's nested-Cargo package-cache lock:

```text
STRUCTURE_TEST_FILES_PASSED:26
STRUCTURE_ASSERTIONS_PASSED:507
```

## Remaining scope

The #562 assertion and #761 loader completion are now executable in the
aarch64 testing profile. Indexed ext2 directories remain outside the kernel
reader's supported format; generated test fixtures use linear directories,
and #778 tracks native indexed-directory lookup.
The loader proof stops at its completion marker, while the separate BTRT run
continues through the five musl exits and its mutation oracle. Results for
unrelated userspace programs after loader release remain owned by their
individual test criteria rather than these two issue repairs.
