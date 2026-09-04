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

`kernel/src/task/kthread.rs` is a Tier-2 file under the stricter of the two
tables in this tree: `CLAUDE.md`'s Tier-2 table has 6 rows and includes it and
`kernel/src/task/workqueue.rs`; `AGENTS.md`'s has 4 rows and includes neither.
This round treats it as Tier-2. Its change was required because
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

**One rule.** claim-lint:ok: the census counts are the ratchets' own --
8 `*_can_sleep` subjects and the #580 inventory of ten, in
tests/teardown_structure.rs.
`kernel/src/task/idle_sleep.rs` owns the decision. Each
`*_can_sleep` predicate calls `idle_identity_must_not_sleep()`, and each member
of the #580/#648 blocking-primitive family that publishes the caller's blocked
state calls `refuse_idle_block()`, the in-scheduler-lock spelling of the same
decision. That covers FUTEX_WAIT and the other direct `prepare_to_wait_checked`
users by construction rather than by remembering to ask a predicate first.
`IDLE_SLEEP_REFUSED` counts refusals and prints one marker line on first
occurrence. Two census ratchets in `tests/teardown_structure.rs` locate their
subjects by shape -- `_can_sleep` in a name, the family's own name prefixes --
so a predicate or primitive added later is censused without being listed
anywhere. At round 3 the census reached its eighth subject: AHCI's
`scheduler_sleep_ready` (`kernel/src/drivers/ahci/mod.rs`) decided
scheduler-sleep eligibility from "a thread id exists and the timer runs", which
the aarch64 idle identity satisfies. It now calls the same refusal, so the rule
and its census agree about every decision point rather than about seven of
eight.

Round 3 also fixed the ratchets themselves. Both matched raw body text, so
deleting an executable guard and leaving its name behind in a comment kept them
green -- that was the review's N04, demonstrated against a doctored copy. Both
now strip comments before matching and require the guard as a CALL in the
executable body. 5 mutation legs prove they are not vacuous: the 3 round-2
legs, plus one per rule that performs exactly the delete-the-call,
keep-the-name edit (`sleep_predicate_rule_rejects_a_refusal_left_only_in_a_comment`
and `blocking_primitive_rule_rejects_a_refusal_left_only_in_a_comment`).

The rule is scoped to aarch64, and that scoping is measured. On x86_64 the boot
thread is the idle task by construction and the boot loader reads test binaries
under a per-block `without_interrupts`, so the VirtIO ISR can only run once the
loader blocks and the scheduler switches away from it. Applying the refusal
there stopped the boot after its first binary
(`serials/r2/x86-boot-parallel-refusal-applied-to-x86.txt`), while `main`
finishes (`serials/r2/x86-boot-parallel-main.txt`). The refusal therefore
applies where dispatch discards the continuation and nowhere else; on x86 it
answers false, which is the behavior those paths had before this branch.

**One relocation, widened at round 3.** Round 2 moved the loader alone into a
kernel thread and joined it from the idle identity. That left a join reachable
from the idle identity, which is the same shape one level up, so round 3 moves
the whole remainder instead.

After `init_scheduler()`, the timer and the secondary CPUs are live and
`kernel_main` does exactly two things: it hands the rest of the boot sequence
to one kernel thread, `boot_continuation`, and it releases the boot preemption
pin and idles. Everything that follows -- the kthread and workqueue self-tests,
Test 7's daemon phase and the CPU-1 kthread it joins, the boot-test batteries,
the ext2 loader, the init launch, the completion marker -- runs on that thread,
which is a schedulable identity the scheduler hands a continuation back to.

The loader therefore needs no kthread of its own any more: it is a plain call
on `boot_continuation`. The idle identity joins nothing, blocks on nothing, and
runs no test.
claim-lint:ok: pinned by tests/teardown_structure.rs::no_kthread_join_is_reachable_from_the_idle_identity and
tests/aarch64_testing_profile_structure.rs::the_boot_sequence_runs_in_a_kernel_thread_and_the_loader_with_it.

Releasing the pin is part of the handoff, not an optional tidy-up: the boot
sequence is a thread now, and CPU 0 has to be preemptible for the scheduler to
dispatch it at all. Two things that had been resting on that pin were given
their own mechanism:

- The testing-profile staging pen used to be "the boot CPU does not dispatch,
  so the freshly created user threads parked on its queue cannot run a partial
  catalog". `begin_test_binary_staging()` now takes a preempt pin on its own
  CPU and records which CPU that is; new user threads pin to that CPU;
  `finish_test_binary_staging()` clears the pins and releases the pin, and is a
  no-op unless staging opened, so the two are a matched pair by construction.
  claim-lint:ok: the pairing is a compare_exchange on 1 flag,
  kernel/src/task/scheduler.rs.
- `launch_init_from_elf` used to release the boot pin. It runs on the boot
  continuation now, on whichever CPU the scheduler placed it, so decrementing
  a preempt count there would underflow a different CPU's. The release moved to
  the handoff and the call is gone.

A census ratchet in `tests/teardown_structure.rs`,
`no_kthread_join_is_reachable_from_the_idle_identity`, holds the shape open.
claim-lint:ok: the census is that test, which fails if it classifies fewer
than 10 callers, so an empty run cannot pass for a clean one.
Each `kthread_join` caller in `kernel/src` must be a kthread body (reachable
by name from a function handed to a `kthread_run` spawn), a context compiled
only under a feature, or a stop-then-join teardown that calls `kthread_stop` in
the same body. The subjects are located by those shapes, and no list of names appears in
the rule. Its
mutation leg restores round 2's exact shape -- `kernel_main` joining the thread
it spawned -- and reddens.

The loader still unmasks interrupts before its first ext2 read: the preceding
self-tests deliberately return with IRQs masked, and VirtIO MMIO completion is
IRQ-driven. Measured, before that line existed, as
`[test] Loaded 0/78 test binaries (0 failed, 78 not found)`
(`serials/r2/loader-kthread-masked-boot-cpu.txt`, which contains that line 1
time). The `Block MMIO read timeout` and wedged-gate detail came from an
instrumented run that was not committed: `grep -c` for either string in that
file returns 0.

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
## What moving the boot sequence onto a thread exposed

Two production defects the boot preemption pin had been hiding. Both are
pre-existing shapes; both became reachable the moment the boot sequence stopped
owning a CPU no other thread could be scheduled onto.
claim-lint:ok: the 2 defects are the 2 subsections below, each with its own
measurement.

**A wait that masked its own wakeup.** `kthread_join` waited with a bare
`arch_halt()`. On aarch64 that is `wfi` with whatever DAIF the caller had, and
the callers here have IRQs masked: the kthread and workqueue self-tests each
end with `arch_disable_interrupts()`. A masked `wfi` returns immediately on a
pending interrupt WITHOUT taking it, so the loop spins at full speed and
the CPU takes 0 context switches. While the joiner was CPU 0 it cost a
spin and no more: the joined thread was on one of the other 3 CPUs. Once the joiner is an ordinary
kernel thread, the scheduler can place it on the same CPU as the thread it is
waiting for, and then that thread does not run at all.

Caught with GDB on a wedged boot rather than reasoned about: 3 of the 4 CPUs
were `[halted]` in `idle_loop_arm64`, CPU 1 was `[running]` inside
`test_softirq`, and CPU 1's `cpsr` read `0x800000c5` -- the `I` bit set. The
instruction at `$pc` was the `ldarb` of `handle.inner.exited` immediately after
the loop's `wfi`. The thread it was waiting for was `softirq-test/1`, pinned to
CPU 1 by `kthread_run_on_cpu`.

The fix is at the primitive, not the call site: `kthread_join`'s loop,
`kthread_park_prepared`'s loop, and `Work::wait`'s loop now use
`arch_halt_with_interrupts()`, which unmasks before the `wfi`. A wait for
another thread has to leave the CPU able to switch to it, and the halt now
unmasks before the `wfi` at 3 of the 3 call sites listed above.

**A check-then-park that could lose a wakeup.** `worker_thread_fn` popped an
empty queue and then called `kthread_park()`, and `ksoftirqd_fn` read a
pending bitmap of 0 and then called `kthread_park()`. A producer that queues
work in that window finds the consumer still `Running`, so its `unblock` has
no state to change, and the consumer parks on top of the work. Linux publishes
the sleep intent first and re-checks the condition after (`prepare_to_wait`,
`kthread_parkme`), and that is what both loops do now:
`kthread_prepare_to_park()`, the re-check, then `kthread_park_prepared()` or
`kthread_cancel_park()`.

`kthread_unpark` also moved its flag clear inside the scheduler lock the parker
publishes `Blocked` under, so the flag and the scheduling state are one
decision under one lock. Without that, an unpark could clear the flag, find the
parker still `Running` -- so its `unblock` has no state to change -- and the
parker would
publish `Blocked` anyway and be stranded.

Both windows are counted, not asserted: `[WORKQUEUE_PARK_RACE:cancelled=N:intent_cleared=N]`
and `[KSOFTIRQD_PARK_RACE:cancelled=N]` reach serial from the self-tests. 3 of
the 6 boots of the round's final build reported a count above 0 -- 2
`cancelled=1` and 1 `intent_cleared=1` -- so the window is real and is being
taken. Each of those is 1 wakeup the old shape would have dropped.

### The measurement

Four 12-boot batches on this Mac, 3 QEMUs at a time, 45s each, each boot on a
fresh copy of the same fixture, counted by
`grep -al "Test processes loaded" | wc -l`:

| Build | Reached the completion marker |
|---|---|
| round-2 head `d7679a7b` | 35 of 36 |
| round 3, boot sequence on a kthread, park protocol only | 30 of 36 |
| round 3, plus the interrupts-enabled halt | 36 of 36 |

The middle row is the regression this round would have shipped: 5 of its 6
failures wedge in Test 7's daemon phase with `[WORKQUEUE_PARK_RACE:...]`
already on serial, 1 wedges in the workqueue test. The top row's single failure
is the same defect reached the other way -- a workqueue worker placed on the
preempt-pinned boot CPU while its flusher masked-spins -- which is why the
bottom row is better than the baseline rather than merely equal to it. A hung
serial from the middle row is committed as
`serials/r3/test7-wedge-before-the-halt-fix.txt`.


## The fixture, and the count line

Round 1 published `[test] Loaded 78/78`; the round-1 review reproduced 77/78
against the fixture it had. Both are right about their own fixture. The cause
is not in the kernel or the image builder: `tcp_cloexec_exec_test` entered
`kernel/src/boot/test_list.rs` with PR #765 on 2026-09-02 (`63e5f8e0`, author and commit date
`2026-09-02T10:51:17-04:00`; merge `509802e5`, `2026-09-02T15:42:07-04:00`),
and the
`userspace/programs/aarch64/` ELF set the reviewer's image was built from
predates it. Rebuilding the aarch64 userspace produces
`tcp_cloexec_exec_test.elf` like the other 147, and building the five vendored
musl programs alongside it leaves 0 of the 78 catalog names without a file --
after which the round-1 branch tip itself also loads 78 of 78
(`serials/r2/testing-profile-boot-at-06d149b6.txt`).

So the count line is left to follow the fixture, and the loader now names what
it could not resolve and why (`[test] Not found: <name> (<reason>)`), which is
what turns the next stale fixture into a diagnosable line instead of an
inflated number.
claim-lint:ok: the resolver reports 2 arms, not-found and resolver-error, in
kernel/src/main_aarch64.rs. `scripts/create_ext2_disk.sh` no longer differs from `main` in any
behaviour: the `-O ^dir_index` flag round 1 added is withdrawn, along with the
claim that the kernel cannot traverse an indexed directory. It is not
byte-identical to `main` -- `git diff --numstat origin/main -- 
scripts/create_ext2_disk.sh` reports `4 2`, four inserted and two deleted
comment lines that replace "Create hardlinks for all applets" with the
counted form (48 names in each of the 2 loops, 48 unique) that claim-lint
asks for. `git diff origin/main -- scripts/create_ext2_disk.sh` shows those
6 lines and nothing else. The pre-change fixture had
`DIR_INDEX` set and `/bin` carried `EXT2_INDEX_FL`, and the kernel read from it
correctly; the fixture this round's boots use is built with `dir_index` on.

## What the profile does after the loader, and what it reaches first

The plain `--features testing` profile REACHES its completion marker. The
round-3 acceptance boot (`serials/r3/testing-profile-boot.txt`) puts the marker
at line 1924 of 7754 and the soft lockup 1517 lines later:

```text
[test] Loaded http_test (PID 79)
[test] Loaded 78/78 test binaries (0 failed, 0 not found)
[test] Test processes loaded - will run via timer interrupts
[test] Entering scheduler idle loop
breenix> EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)
```

The last 20 lines of that same boot, which are the tail of the lockup dump:

```text
  PID 107 [terminated] thread-107
  PID 112 [ready] true_test_child_112
  PID 114 [ready] signal_exec_test_child_114
  PID 118 [terminated] sigchld_test_child_118
  PID 123 [ready] sigkill_teardown_test_child_123
  PID 132 [ready] head_test_child_132
  PID 133 [ready] wc_test_child_133
  PID 134 [ready] ls_test_child_134
  PID 135 [ready] which_test_child_135
Trace counters:
  SYSCALL_TOTAL:    3638
  IRQ_TOTAL:        139314
  CTX_SWITCH_TOTAL: 27658
  TIMER_TICK_TOTAL: 54583
  FORK_TOTAL:       54
  EXEC_TOTAL:       0
  Global ticks:     11756
  Timer IRQ count:  54586
!!! END SOFT LOCKUP DUMP !!!

```

So the lockup does NOT prevent the profile's completion marker: it happens
after it, with the 78 test processes already running and 3638 syscalls already
serviced. The marker is what #562 and #761 are about; the lockup is what
happens next.

The attribution is to open issue #728, "Both arches: concurrent ext2 read-park
vs write-spin is a livelock shape", and it is a signature match rather than a
guess. That issue's mechanism is a reader that PARKS holding `ROOT_EXT2.read()`
against a writer that SPINS in `upgradeable_read().upgrade()`, and it names the
missing precondition on aarch64 as "one overlapping ext2 write". This boot
supplies it: the 9 `EXT2_LOCK_SPIN_STALL` lines in that serial are 7 on
`lock=ROOT_EXT2_read` and 2 on `lock=ROOT_EXT2_write`, interleaved, each about
0.5s, immediately before `!!! SOFT LOCKUP DETECTED !!!`.

Incidence, re-derived over the 12-boot round-3 batch `c2A`
(`grep -l "SOFT LOCKUP DETECTED"` -> 11 of 12;
`grep -l "Test processes loaded"` -> 12 of 12): every boot reaches the marker,
and 11 of 12 go on to lock up. The per-boot stall counts in that batch are
9, 10, 7, 5, 0, 7, 4, 7, 6, 4, 7, 4 -- the 0 is the boot that did not lock up.
It is present at the round-1 tip too
(`serials/r2/testing-profile-boot-at-06d149b6.txt`), so it is not something
this branch introduced.

It is not fixed here. #562 and #761 are about reaching the loader's completion
marker; #728 is a different defect with its own scope, its own filing, and a
fix that has to cover `ROOT_EXT2` and `HOME_EXT2` on both arches. It is why the
`testing,btrt` boot against the full fixture does not reach `===BTRT_READY===`
in 1 of the 1 full-catalog capture (`serials/r2/musl-btrt-full-catalog.txt`).


## Validation

Commands and their results at round 3, on the round-3 head. The 3 aarch64
builds use `aarch64-breenix-kernel.json`, `-Z build-std=core,alloc`, and
`-Z build-std-features=compiler-builtins-mem`.

| Profile | `^(warning|error)` lines | Soft-float guard |
|---|---|---|
| aarch64 `testing` | 1, the toolchain's `core v0.0.0` future-incompat notice | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| aarch64 `boot_tests` | 1, the same notice | same `PASS` line |
| aarch64, no features | 1, the same notice | same `PASS` line |
| x86 `testing,external_test_bins` on beast | 0 | not applicable |

0 of those lines is a project warning; the `core v0.0.0` notice is the
toolchain's and appears on `main` too. The x86 build ran in an isolated beast
checkout (`/root/breenix-a64r2`) with
`BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library`, forced clean by
touching `kernel/src/task/scheduler.rs` first.

Runtime, aarch64, on this Mac, 3 QEMUs at a time, 45s each, a fresh copy of the
same fixture per boot:

- `testing`: 36 of 36 boots reached
  `[test] Test processes loaded - will run via timer interrupts`
  (`grep -al` over the 3 12-boot batches `c2A`, `c2B`, `c2C`).
- no features: 2 of 2 boots launched init from the pre-loaded ELF and reached
  `[heartbeat]`. This profile matters because the init launch moved onto the
  boot continuation.
- `boot_tests`: 1 of 1 boot printed `[boot] All boot tests passed!`.

x86 was booted as well, not only built: `./docker/qemu/run-boot-parallel.sh 1`
under xvfb on beast, 2 runs of this round and 1 each of `d7679a7b` (round 2)
and `bfbb7575` (the branch base, which is on `main`).
4 of those 4 reported the same
line -- `TEST_TALLY: exited=22 nonzero=1 failed=[simple_exit:42]` -- after
`USERSPACE TEST COMPLETE`, and 4 of 4 were scored FAIL by
`scripts/x86-gate-verdict.sh`.

That red is not this branch's. A fifth run at `52491c4b` (main, 2026-09-02)
passed 1 of 1, so it arrives on `main` between that revision and `bfbb7575`.
The cause is that `simple_exit` exits 42 because exiting 42 is the whole
program, and the tally counts a nonzero exit as a failed process while
`scripts/x86-gate-allowlist.txt` holds 0 entries. Round 2 recorded this as
"the x86 gate's known red" without filing it; it is now #781.

Mutation legs, run against the working tree rather than an in-memory copy, so
the red text below is what a reviewer gets by making the same edit:

```text
$ # completion.rs: delete the call, keep the name in a comment
$ cargo test --test teardown_structure sleep_predicates_consult_the_shared_idle_refusal
every *_can_sleep predicate routes through the shared idle refusal: ["kernel/src/task/completion.rs :: current_context_can_sleep  (sleep eligibility decided without the shared idle refusal)"]
test result: FAILED. 0 passed; 1 failed
$ # scheduler.rs: same edit against the in-lock spelling
$ cargo test --test teardown_structure blocking_primitives_refuse_the_idle_identity
every blocking primitive refuses the idle identity: ["kernel/src/task/scheduler.rs :: block_current_inner  (publishes a blocked state without refusing the idle identity, and delegates to no family member that does)"]
test result: FAILED. 0 passed; 1 failed
```

Both were reverted and both baselines re-run green afterwards.

Structure tests, via `cargo test --test <name>` for each of the 26 files
matching `tests/*_structure.rs`: 520 passed, 0 failed.

## Remaining scope

The #562 assertion and #761 loader completion are executable in the aarch64
testing profile. The post-loader soft lockup above is open and attributed to
#728. Indexed ext2 directories are supported by the reader that this round
measured, so #778 is closed. Results for userspace programs after loader
release remain owned by their individual test criteria rather than these two
issue repairs.
issue repairs.

### Disclosed and not fixed here

**The IRQ-exit path still takes the global scheduler lock.** `do_softirq`'s
iteration-limit arm calls `wakeup_ksoftirqd()`, which calls `kthread_unpark`,
which calls `with_scheduler`, which blocks in `lock_scheduler()` on the one
global `SCHEDULER` mutex -- from an interrupt handler.

It is pre-existing, and that is checked rather than asserted: `git blame`
against `origin/main` puts 4 of the 4 lines of that path on `main` --
`exception.rs:2212` at `0668208d7` (2026-02-06), `softirqd.rs:220` at
`edf9186fe` (2026-01-21), `kthread.rs:227` at `9f67bb795` (2026-01-18), and
`scheduler.rs:4609` at `af73c0d45` (2026-08-06), which is an ancestor of
`origin/main`. Round 3's edit to `kthread_unpark` moves the parked-flag store
INSIDE the `with_scheduler` call that was already there; it neither adds nor
removes a lock on that path.

Linux takes no global lock here: `wakeup_softirqd()` wakes the local
`ksoftirqd` through `wake_up_process`, which takes that task's own runqueue
lock with `raw_spin_lock_irqsave`. Per-runqueue and interrupt-safe, not one
mutex for the machine. An IRQ-exit wakeup in this kernel shares the property
whichever thread it wakes, because `Scheduler::unblock` takes `&mut Scheduler`
and that reference exists only inside the `lock_scheduler()` critical section.
The fix is per-CPU or irqsave-typed runqueue locking -- a scheduler change, not
a testing-profile change -- so it is filed as #780 rather than attempted here.
