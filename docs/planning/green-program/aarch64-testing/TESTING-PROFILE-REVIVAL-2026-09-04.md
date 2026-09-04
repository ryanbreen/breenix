# Aarch64 testing-profile revival — 2026-09-04

This fix-forward revives the soft-float aarch64 `testing` boot from Breenix
`bfbb7575`. It addresses the #562 softirq self-test panic, the #761 ext2 test
loader continuation loss, and incorporates the five musl BTRT entries from
Lane B (`4d2a151e` and `f1711505`).

Round 2 re-derived each runtime number here against `deebc5d1`, and the serial
behind it is committed beside this file -- see
`serials/r2/README.md` for the capture command and one row per file. Where a
round-1 number did not reproduce, it is corrected below rather than repeated.

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
CPU's handle through a bounds-checked index. Thread affinity is preserved
through scheduler insertion, wakeup, stealing decisions, cloning, and
testing-profile fork construction.

Only Test 7's daemon-verification phase runs in the CPU-1-pinned kthread.
Tests 1-6 and 8 run in the boot context on both arches, where they ran before
this branch: the phase needs a CPU the scheduler can switch away from and needs
its raise and its drain on one CPU, which 0 of the other 7 tests do. Both of
Test 7's assertions are unchanged, and its serial marker now reports the daemon
the handler observed rather than the CPU the phase was aimed at.

`kernel/src/task/kthread.rs` is a Tier-2 file. Its change was required because
the CPU-targeted creator had been test-only, while the repaired production
softirq topology needs CPU-targeted workers. Observation through tracing could
show the wrong-CPU bitmap reads but could not supply that missing production
primitive. The entry path gained 0 logging calls, allocations, or blocking
locks.

The handler's identity read moved off `scheduler::current_thread_id()`, which
takes the global SCHEDULER spin lock, onto the lock-free per-CPU pointer. The
#562 RCA flagged that call and said any fix moving this workload onto a
preemptible CPU should re-examine it; the workload moved onto a CPU with three
contending peers, so it was re-examined. The handler's code now contains 0
scheduler-lock calls, and `tests/aarch64_testing_profile_structure.rs` pins
that.

The soft-float `--features testing` runtime boot recorded
(`serials/r2/testing-profile-boot.txt`):

```text
SOFTIRQ_TEST: iteration limit passed (25 total iterations, ksoftirqd/1)
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

Round 2 replaced round 1's two separate repairs of that shape with one rule and
one relocation.

**One rule.** `kernel/src/task/idle_sleep.rs` owns the decision. Each
`*_can_sleep` predicate calls `idle_identity_must_not_sleep()`, and each member
of the #580/#648 blocking-primitive family that publishes the caller's blocked
state calls `refuse_idle_block()`, the in-scheduler-lock spelling of the same
decision. That covers FUTEX_WAIT and the other direct `prepare_to_wait_checked`
users by construction rather than by remembering to ask a predicate first.
`IDLE_SLEEP_REFUSED` counts refusals and prints one marker line on first
occurrence. Two census ratchets in `tests/teardown_structure.rs` locate their
subjects by shape -- `_can_sleep` in a name, the family's own name prefixes --
so a predicate or primitive added later is censused without being listed
anywhere. 3 mutation legs prove they are not vacuous.

The rule is scoped to aarch64, and that scoping is measured. On x86_64 the boot
thread is the idle task by construction and the boot loader reads test binaries
under a per-block `without_interrupts`, so the VirtIO ISR can only run once the
loader blocks and the scheduler switches away from it. Applying the refusal
there stopped the boot after its first binary
(`serials/r2/x86-boot-parallel-refusal-applied-to-x86.txt`), while `main`
finishes (`serials/r2/x86-boot-parallel-main.txt`). The refusal therefore
applies where dispatch discards the continuation and nowhere else; on x86 it
answers false, which is the behavior those paths had before this branch.

**One relocation.** The loader itself moved off the idle identity: it is handed
to `kthread_run` and joined from boot, so the identity holding the ext2 and
VirtIO guards is one the scheduler can hand a continuation back to. The boot
CPU keeps its preemption pin; what stays on it is the join's bounded halt loop.
The boot CPU is unmasked before the spawn, because the VirtIO block interrupt
is an SPI the GIC targets there and nothing else would have unmasked it once
the loader left -- measured, before that line existed, as `Block MMIO read
timeout` on the first inode read and then a wedged gate for every read after it
(`serials/r2/loader-kthread-masked-boot-cpu.txt`).

With both in place the profile records 0 `IDLE_SLEEP_REFUSED` lines. Before the
relocation, with the rule alone, it recorded exactly 1, during the pre-timer
`/sbin/init` pre-load (`serials/r2/idle-refusal-before-the-loader-moved.txt`).

The runtime boot at `deebc5d1` used the soft-float kernel target and a writable
copy of the full ext2 fixture (`serials/r2/testing-profile-boot.txt`):

```text
[test] Loaded 78/78 test binaries (0 failed, 0 not found)
[test] Test processes loaded - will run via timer interrupts
```

The same serial carries 78 per-program `[test] Loaded <name> (PID n)` markers
before the completion marker.

## The fixture, and the count line

Round 1 published `[test] Loaded 78/78`; the round-1 review reproduced 77/78
against the fixture it had. Both are right about their own fixture. The cause
is not in the kernel or the image builder: `tcp_cloexec_exec_test` entered
`kernel/src/boot/test_list.rs` with PR #765 on 2026-09-03, and the
`userspace/programs/aarch64/` ELF set the reviewer's image was built from
predates it. Rebuilding the aarch64 userspace produces
`tcp_cloexec_exec_test.elf` like the other 147, and building the five vendored
musl programs alongside it leaves 0 of the 78 catalog names without a file --
after which the round-1 branch tip itself also loads 78 of 78
(`serials/r2/testing-profile-boot-at-06d149b6.txt`).

So the count line is left to follow the fixture, and the loader now names what
it could not resolve and why (`[test] Not found: <name> (<reason>)`), which is
what turns the next stale fixture into a diagnosable line instead of an
inflated number. `scripts/create_ext2_disk.sh` is back to `main`'s text: the
`-O ^dir_index` flag round 1 added is withdrawn, along with the claim that the
kernel cannot traverse an indexed directory. The pre-change fixture had
`DIR_INDEX` set and `/bin` carried `EXT2_INDEX_FL`, and the kernel read from it
correctly; the fixture this round's boots use is built with `dir_index` on.

## What the profile does after the loader

The completion marker is not the end of the boot, and round 1's document did
not say what happens next. A few seconds after the loader releases the batch,
with the 78 test processes running, the profile takes a system-wide soft
lockup: 3 `EXT2_LOCK_SPIN_STALL` lines on `ROOT_EXT2_read` (~0.5s each) and
then `!!! SOFT LOCKUP DETECTED !!!`. The dump shows CPUs 1-3 each with a
current thread and a non-empty ready queue -- a livelock, not a lost wakeup --
and the signature matches open issue #728, "Both arches: concurrent ext2
read-park vs write-spin is a livelock shape".

It is present at the round-1 tip too
(`serials/r2/testing-profile-boot-at-06d149b6.txt`) and it is not addressed
here: #562 and #761 are about reaching the loader's completion marker, and
#728 is a different defect with its own scope. It is recorded rather than
carried silently, and it is why the `testing,btrt` boot against the full
fixture never reaches `===BTRT_READY===`.

## Validation

Commands and their results, at `deebc5d1` unless stated. The 4 aarch64 builds
use `aarch64-breenix-kernel.json`, `-Z build-std=core,alloc`, and
`-Z build-std-features=compiler-builtins-mem`.

| Profile | `^(warning|error)` lines | Soft-float guard |
|---|---|---|
| aarch64 `testing` | 1, the toolchain's `core v0.0.0` future-incompat notice | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| aarch64 `boot_tests` | 1, the same notice | same `PASS` line |
| aarch64, no features | 1, the same notice | same `PASS` line |
| aarch64 `testing,btrt` | 1, the same notice | not re-run for this profile |
| x86 `testing,external_test_bins` on beast | 0 | not applicable |

0 of those lines is a project warning; the `core v0.0.0` notice is the
toolchain's and appears on `main` too. The x86 build ran in an isolated beast
checkout (`/root/breenix-a64r2`) with
`BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library`, forced clean by
touching `kernel/src/task/scheduler.rs` first.

x86 was booted as well, not only built: `./docker/qemu/run-boot-parallel.sh 1`,
2 runs of the branch and 2 of `main` with the same disks, all four reporting
the same signature -- `TEST_TALLY: exited=22 nonzero=1 failed=[simple_exit:42]`
after `USERSPACE TEST COMPLETE`, which is the x86 gate's known red. A third
branch run showed `nonzero=3` with two `loopback_wake_test` entries and is
recorded as a flake rather than smoothed over
(`serials/r2/x86-boot-parallel-tally-lines.txt`).

Structure tests, via `cargo test --test <name>` for each of the 26 files
matching `tests/*_structure.rs`: 516 passed, 0 failed. Cargo did not self-lock
in this worktree, so no `rustc --test` fallback was needed.

## Remaining scope

The #562 assertion and #761 loader completion are executable in the aarch64
testing profile. The post-loader soft lockup above is open and attributed to
#728. Indexed ext2 directories are supported by the reader that this round
measured, so #778 is closed. Results for userspace programs after loader
release remain owned by their individual test criteria rather than these two
issue repairs.
