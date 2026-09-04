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
anywhere. At round 3 an eighth decision point was found and repaired: AHCI's
`scheduler_sleep_ready` (`kernel/src/drivers/ahci/mod.rs`) decided
scheduler-sleep eligibility from "a thread id exists and the timer runs", which
the aarch64 idle identity satisfies. It now calls the same refusal. The name
census did NOT reach it, and could not -- it names 7 subjects, the 7
definitions `grep -rEn "fn [a-z0-9_]*_can_sleep" kernel/src` reports -- which
is what the round-3 review raised as N03 and N15 and what the call-site census
below repairs.

Round 3 also fixed the ratchets themselves. Both matched raw body text, so
deleting an executable guard and leaving its name behind in a comment kept them
green -- that was the review's N04, demonstrated against a doctored copy. Both
now strip comments before matching and require the guard as a CALL in the
executable body. 5 mutation legs prove they are not vacuous: the 3 round-2
legs, plus one per rule that performs exactly the delete-the-call,
keep-the-name edit (`sleep_predicate_rule_rejects_a_refusal_left_only_in_a_comment`
and `blocking_primitive_rule_rejects_a_refusal_left_only_in_a_comment`).

Round 4 fixed the discovery. Reading names still could not see
`scheduler_sleep_ready`, so deleting ITS call and leaving the name in a comment
kept the rule green -- the review's N03 and N15. A third census now discovers
predicates by call site instead: it locates the primitives that block their
CALLER, the functions that reach one without carrying a family name (a body
that calls a primitive, and a single-expression forwarder to one of those --
`Completion::wait_timeout_uninterruptible` is how the AHCI wait reaches
`block_current_for_io`), and then, at each call site of those, the booleans
the code consulted first: the condition of an `if` the call sits inside, or of
an `if` before it that leaves the body. A guard written as a bound name or a
struct field is resolved back to the call that produced it, which is what
attributes AHCI's `if token.scheduler_running` to `scheduler_sleep_ready` two
functions away. It finds 5 guard positions on this tree, 2 of which decide
something other than sleep eligibility and are recorded as such in an
exact-match table; a discovered predicate that neither consults the refusal nor
appears there is a finding, and so is a table row the census stops finding.
The `_can_sleep` census is kept unchanged as the first leg, and the sleep-guard
census carries 2 legs of its own.

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

`init_scheduler()` is at `kernel/src/main_aarch64.rs:838`, and it is where the
boot context becomes CPU 0's idle task. The handoff is not the next statement.
Between the two, still on that identity, `kernel_main` initializes the
workqueue subsystem (`:844`), the softirq subsystem and the loopback pump
(`:850`, `:851`), spawns the render thread when a display is present and VirGL
does not own it (`:869`), initializes and enables tracing (through `:880`),
pre-loads `/sbin/init` from ext2 while the timer is still off (`:891`,
`read_init_from_ext2` at `:893`), activates xHCI MSI (`:910`), initializes the
timer interrupt (`:924`), brings up the secondary CPUs and their GICR
redistributor map (through `:1231`), and adds a pinned ksoftirqd per newly
online CPU (`:1235`). Only then does it hand the rest of the boot sequence to
one kernel thread, `boot_continuation`, spawned as `kboot` at `:1250`, release
the boot preemption pin at `:1264`, enable interrupts at `:1266`, and enter its
idle loop -- `wfi` plus `drain_loopback_from_idle()` at `:1273`.

The `/sbin/init` pre-load reads ext2 while still on the idle identity, and its
placement before `timer_interrupt::init()` at `:924` is what keeps that safe:
with the timer not yet running, the sleep-eligibility predicates answer false
there, so the read spins instead of publishing a blocked state. The round-3
review reached the same reading of that window under F01. Everything that runs
after the handoff -- the kthread and workqueue self-tests, Test 7's daemon
phase and the CPU-1 kthread it joins, the boot-test batteries, the ext2
test-binary loader, the init launch, the completion marker -- runs on
`boot_continuation`, which is a schedulable identity the scheduler hands a
continuation back to.
claim-lint:ok: every line number in this paragraph was read out of
kernel/src/main_aarch64.rs at write time with `grep -n`; the ordering claim is
also pinned by tests/aarch64_testing_profile_structure.rs::the_boot_sequence_runs_in_a_kernel_thread_and_the_loader_with_it.

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

Round 4 narrowed the feature-gate arm, which was the review's N16: it excused
a gated joiner with no further question asked, so a `#[cfg(feature = "testing")]` helper
called straight from `kernel_main` could join on the idle identity and stay
green. A gate decides which build compiles the code, not which identity runs
it, so the arm now applies only while `kernel_main` has no path to the joiner,
over a closure computed across cfg-gated callees. 2 further legs cover it: the
gated helper called directly, and a gated helper 2 hops out behind a gated
first hop.

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
and `[KSOFTIRQD_PARK_RACE:cancelled=N]` reach serial from the self-tests. In the
committed 12-boot batch (`serials/r3/batches/README.md`, with all 12 raw
serials committed beside it), 1 of the 12 boots reported
`WORKQUEUE_PARK_RACE:cancelled=1:intent_cleared=0` and the other 11 reported
zeroes; `KSOFTIRQD_PARK_RACE:cancelled=0` in 12 of 12. So the window is real and
it is taken, at 1 boot in 12 on this host, and that 1 is a park the old shape
would have raced. Round 3 published a higher rate over a batch whose serials
were not committed, and round 4 published `intent_cleared=1` over a batch whose
serials no longer exist; both are withdrawn in favour of the row above, which
`grep -ao "WORKQUEUE_PARK_RACE:[^]]*" serials/r3/batches/testing-boot6.txt`
reproduces.

### The measurement

One 12-boot batch on this Mac at the round-5 head, 3 QEMUs at a time, 45s
each, each boot on a fresh copy of the same fixture. `kernel/src` is
byte-identical to the round-4 head (`git diff --stat 436c93f7 HEAD -- kernel/`
prints 0 lines), so this is round 4's kernel re-measured against a fixture
rebuilt from the tree. 15 of the 15 raw serials are committed under
`docs/planning/green-program/aarch64-testing/serials/r3/batches/README.md`'s
directory, 1 file per row:

| Profile | Result |
|---|---|
| `testing`, 12 boots | 12 of 12 reached `[test] Test processes loaded`, 12 of 12 loaded 73 of 78 |
| `boot_tests`, 1 boot | 1 of 1 printed `[boot] All boot tests passed!` |
| no features, 2 boots | 2 of 2 pre-loaded init and ran `/bin/heartbeat` |

The count line is 73 of 78 rather than 78 of 78 because the rebuilt fixture is
missing the 5 musl C programs, which need a musl libc this worktree does not
carry; `serials/r3/batches/README.md` names them. The denominator is the 78
names in `kernel/src/boot/test_list.rs`, which
`grep -cE '^\s*"' kernel/src/boot/test_list.rs` counts as 78. Round 4's fixture loaded 78 of 78, and round 4's own committed
serials say so: at `436c93f7`, `serials/r3/batches/testing-lockup-boot1.txt`
line 1903 and `testing-lockup-boot6.txt` line 1918 each print
`[test] Loaded 78/78 test binaries (0 failed, 0 not found)`
(`git show 436c93f7:<path> | grep -n "Loaded [0-9]*/78"`). The drop to 73 is a
property of this round's fixture, not a correction to round 4's count.

`IDLE_SLEEP_REFUSED` appears 0 times in 12 of 12 of those boots, which is the
runtime half of the refusal's claim: the boot sequence no longer asks the idle
identity to block.

Round 3 also published a 3-way comparison -- a round-2-head batch, a
park-protocol-only batch, and a final batch, 36 boots each -- and those batches
were never committed, so the review could not check them. Those 3 numbers are
withdrawn rather than restated. What survives is the qualitative finding they
were reporting, which has its own committed artifact: relocating the boot
sequence and adding the park protocol, without the interrupts-enabled halt,
wedged inside Test 7's daemon phase, and one of those hung serials is committed
as `serials/r3/test7-wedge-before-the-halt-fix.txt` -- the boot progresses to
`[WORKQUEUE_PARK_RACE:cancelled=0:intent_cleared=0]` at line 174 and reports a
soft lockup 3 lines later. The halt fix below is aimed at exactly that.


## The fixture, and the count line

Round 1 published `[test] Loaded 78/78`. The round-1 review, reading an image
built before that day, reported a lower count; that figure lives in the review
and in no committed serial, so this document does not restate it as a
measurement. Both readings are right about their own fixture. The cause is
not in the kernel or the image builder: `tcp_cloexec_exec_test` entered
`kernel/src/boot/test_list.rs` with PR #765 on 2026-09-02 (`63e5f8e0`, author and commit date
`2026-09-02T10:51:17-04:00`; merge `509802e5`, `2026-09-02T15:42:07-04:00`),
and the
`userspace/programs/aarch64/` ELF set the reviewer's image was built from
predates it. Rebuilding the aarch64 userspace produces
`tcp_cloexec_exec_test.elf` alongside the rest of the catalog, and building the
five vendored musl programs with it leaves 0 of the 78 catalog names without a
file -- after which the round-1 branch tip itself also loads 78 of 78
(`serials/r2/testing-profile-boot-at-06d149b6.txt` line 1900).

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

Incidence over the committed 12-boot batch (`serials/r3/batches/README.md`,
whose 12 raw serials are committed beside it): 12 of 12 boots reach the marker,
and there is a post-loader lockup with the #728 signature on 12 of the 12.
Signature means `EXT2_LOCK_SPIN_STALL` lines precede the lockup: in 12 of those
12 they do, 2 to 8 of them per boot, and 0 stall lines appear after a lockup
line. Round 4 measured 11 of 12 against a fixture with a different binary set;
that is the same signature at a lower rate, not a different one. The shortest
specimen is `serials/r3/batches/testing-boot6.txt` (2 stalls, both before the
lockup at line 2284 of 2385) and the longest is
`serials/r3/batches/testing-boot2.txt` (8 stalls, all before the lockup at line
3353 of 7658). The lockup is present at the round-1 tip too
(`serials/r2/testing-profile-boot-at-06d149b6.txt`), so it is not something
this branch introduced. It is not fixed in this branch; #728 is the next
lane.

It is not fixed here. #562 and #761 are about reaching the loader's completion
marker; #728 is a different defect with its own scope, its own filing, and a
fix that has to cover `ROOT_EXT2` and `HOME_EXT2` on both arches. It is why the
`testing,btrt` boot against the full fixture does not reach `===BTRT_READY===`
in 1 of the 1 full-catalog capture (`serials/r2/musl-btrt-full-catalog.txt`).


## Validation

Commands and their results at round 5, re-run on the round-5 head. The 3
aarch64 builds use `aarch64-breenix-kernel.json`, `-Z build-std=core,alloc`,
and `-Z build-std-features=compiler-builtins-mem`.

| Profile | `^(warning|error)` lines | Soft-float guard |
|---|---|---|
| aarch64 `testing` | 1, the toolchain's `core v0.0.0` future-incompat notice | `PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).` |
| aarch64 `boot_tests` | 1, the same notice | same `PASS` line |
| aarch64, no features | 1, the same notice | same `PASS` line |
| x86 `testing,external_test_bins` on beast | 0 | not applicable |

0 of those lines is a project warning; the `core v0.0.0` notice is the
toolchain's and appears on `main` too. 4 of the 4 rows were re-run at the
round-5 head `6cb75784`. The x86 row ran in an isolated beast checkout
(`/root/breenix-a64r2` on the `breenix-x86` container) with
`BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library`, forced clean by
touching `kernel/src/task/scheduler.rs` first, so the `kernel` crate did
recompile.

Runtime, aarch64, on this Mac, 3 QEMUs at a time, 45s each, a fresh copy of the
same fixture per boot. Each number here is derived from 1 of the 15 serials
committed beside
`docs/planning/green-program/aarch64-testing/serials/r3/batches/README.md`:

- `testing`: 12 of 12 boots reached
  `[test] Test processes loaded - will run via timer interrupts`, and 12 of 12
  reported `Loaded 73/78 test binaries (0 failed, 5 not found)`. The 5 are the
  musl C programs the rebuilt fixture does not carry.
- no features: 2 of 2 boots pre-loaded init (`[boot] Init binary pre-loaded:
  298704 bytes`) and went on to run `/bin/heartbeat`, 45 and 45 `[heartbeat]`
  lines. This profile matters because the init launch moved onto the boot
  continuation.
- `boot_tests`: 1 of 1 boot printed `[boot] All boot tests passed!`, 709
  lines.

x86 was booted as well, not only built: `./docker/qemu/run-boot-parallel.sh 1`
under xvfb on beast, 2 runs at `ad455130`, both serials committed as
`serials/r3/batches/x86-boot-parallel-run1.txt` and `-run2.txt`. 2 of the 2
were scored FAIL by `scripts/x86-gate-verdict.sh`. Run 2 reported
`TEST_TALLY: exited=22 nonzero=1 failed=[simple_exit:42]`; run 1 reported
`exited=22 nonzero=2 failed=[simple_exit:42,/usr/local/test/bin/clon:1]`, an
extra intermittent failure at the same commit that is now filed as
https://github.com/ryanbreen/breenix/issues/782. Round 3's "4 of 4 reported the
same line" covered runs whose serials were not committed, and is withdrawn.

That red is not this branch's. A fifth run at `52491c4b` (main, 2026-09-02)
passed 1 of 1, so it arrives on `main` between that revision and `bfbb7575`.
The cause is that `simple_exit` exits 42 because exiting 42 is the whole
program, and the tally counts a nonzero exit as a failed process while
`scripts/x86-gate-allowlist.txt` holds 0 entries. Round 2 recorded this as
"the x86 gate's known red" without filing it; it is now #781.

Mutation legs, run against the working tree rather than an in-memory copy, so
the red text below is what a reviewer gets by making the same edit. claim-lint:ok:
6 of the 6 results in this section were re-run at the round-5 head, per the
round-5 notes in `tests/teardown_structure.rs`; the filtered-out counts are 98
because that file's test count is now 99.

```text
$ # completion.rs: delete the call, keep the name in a comment
$ cargo test --test teardown_structure sleep_predicates_consult_the_shared_idle_refusal -- --exact
every *_can_sleep predicate routes through the shared idle refusal: ["kernel/src/task/completion.rs :: current_context_can_sleep  (sleep eligibility decided without the shared idle refusal)"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
$ # scheduler.rs: same edit against the in-lock spelling
$ cargo test --test teardown_structure blocking_primitives_refuse_the_idle_identity -- --exact
every blocking primitive refuses the idle identity: ["kernel/src/task/scheduler.rs :: block_current_inner  (publishes a blocked state without refusing the idle identity, and delegates to no family member that does)"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

Both were reverted and both baselines re-run green afterwards.

Round 4 added a third and a fourth leg, for the 2 ratchet gaps the round-3
review found. The call-site sleep-guard census, with AHCI's
`&& !idle_identity_must_not_sleep()` replaced by
`&& core::hint::black_box(true) /* idle_identity_must_not_sleep() */`:

```text
$ cargo test --test teardown_structure sleep_guards_at_blocking_call_sites_consult_the_shared_idle_refusal -- --exact
every boolean consulted before a call that sleeps routes through the shared idle refusal: ["kernel/src/drivers/ahci/mod.rs :: wait_cmd_slot0 guards wait_timeout_uninterruptible on scheduler_sleep_ready  (sleep eligibility decided without the shared idle refusal)"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

N21 (round-4 review): round 4 published `92 filtered out` for that run and the
reviewer measured 94 at the round-4 head. Both are superseded by the 98 above,
re-run at the round-5 head. The round-4 text also carried a second finding
string, `"no discovered guard consults the shared refusal: the rule is passing
vacuously"`. It is gone, and its absence is the round-5 repair: the census now
reads `wait_timeout_inner` too, whose guard consults the refusal, so 1 guard
still consults it while AHCI's does not.

The same edit left the `_can_sleep` name census green (`test result: ok. 1
passed; 0 failed; 0 ignored; 0 measured; 98 filtered out`), which is the
N03/N15 gap. And the no-idle-join census, with a `#[cfg(feature = "testing")]`
helper appended to `kernel/src/main_aarch64.rs` and called from `kernel_main`
one line before the pin release:

```text
$ cargo test --features testing --test teardown_structure no_kthread_join_is_reachable_from_the_idle_identity -- --exact
every kthread_join caller is a kthread body, a feature-gated context, or a stop-then-join teardown: ["kernel/src/main_aarch64.rs :: testing_only_join_helper  (joins a kernel thread, is reachable from kernel_main -- the idle identity -- and is neither a kthread body nor a stop-then-join teardown)"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

With the reachability arm forced off -- round 3's rule exactly -- that same
mutation passed (`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;
98 filtered out`), which is the N16 gap. Both mutations were reverted.

Round 5 adds a fifth and a sixth leg, for the 2 gaps the round-4 review found.
N16: the same helper called as
`testing_only_join_helper/* call from kernel_main */(&boot_continuation_thread);`
used to leave that census green at 1 passed, because a call was recognised only
when `(` sat immediately after the identifier; it now reddens with the finding
above, and with round 4's rule restored (`bytes.get(end)`) it passes 1 of 1.

N20: `alternate_sleep_eligibility()` ORed into `wait_timeout_inner`'s sleep-path
guard, with `fn alternate_sleep_eligibility() -> bool { true }` added to
`kernel/src/task/completion.rs`:

```text
$ cargo test --test teardown_structure sleep_guards_at_blocking_call_sites_consult_the_shared_idle_refusal -- --exact
every boolean consulted before a call that sleeps routes through the shared idle refusal: ["kernel/src/task/completion.rs :: wait_timeout_inner guards block_current_for_io_with_timeout on alternate_sleep_eligibility  (sleep eligibility decided without the shared idle refusal)"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

Against round 4's rule exactly -- the wrapper-body pass disabled and its 6
classification rows removed -- that same mutation passed both sleep censuses
(`test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 97 filtered out`),
which is the N20 gap. With the pass disabled but the rows left in place the
census reddens through its stale-row arm, naming 6 of 6. All 4 round-5 edits
were reverted.

The call-site census discovers 15 guard positions on this tree, printed by the
test itself under `--nocapture`:

```text
call-site sleep-guard census (15 discovered):
  kernel/src/drivers/ahci/mod.rs :: wait_cmd_slot0 guards wait_timeout_uninterruptible on scheduler_sleep_ready
  kernel/src/syscall/graphics.rs :: handle_virgl_op guards handle_compositor_wait on window_frame_pending
  kernel/src/syscall/graphics.rs :: handle_virgl_op guards handle_wait_stress_wait on window_frame_pending
  kernel/src/syscall/graphics.rs :: handle_virgl_op guards prepare_to_wait on window_frame_pending
  kernel/src/syscall/handlers.rs :: sys_read guards block_current_in_syscall on is_home_path
  kernel/src/syscall/handlers.rs :: sys_read guards block_current_in_syscall on tcp_has_data
  kernel/src/syscall/signal.rs :: sys_pause_with_frame_aarch64 guards block_current_for_signal_with_context on has_deliverable_signals
  kernel/src/syscall/signal.rs :: sys_sigsuspend_with_frame_aarch64 guards block_current_for_signal_with_context on has_deliverable_signals
  kernel/src/syscall/socket.rs :: sys_connect_tcp guards block_current_in_syscall on tcp_is_established
  kernel/src/syscall/socket.rs :: sys_connect_tcp guards block_current_in_syscall on tcp_is_failed
  kernel/src/task/completion.rs :: wait_timeout_inner guards block_current_for_io_with_timeout on current_context_can_sleep
  kernel/src/task/kthread.rs :: kthread_park guards kthread_park_prepared on kthread_prepare_to_park
  kernel/src/task/softirqd.rs :: ksoftirqd_fn guards kthread_park_prepared on kthread_prepare_to_park
  kernel/src/task/softirqd.rs :: ksoftirqd_fn guards kthread_park_prepared on softirq_pending
  kernel/src/task/workqueue.rs :: worker_thread_fn guards kthread_park_prepared on kthread_prepare_to_park
```

Of those 15, 2 consult the shared refusal -- `scheduler_sleep_ready` and
`current_context_can_sleep` -- and 13 are answered by 1 of the 8 names in
`GUARDS_THAT_DO_NOT_DECIDE_SLEEP_ELIGIBILITY`, each with the definition it
reads. That table is exact-match in both directions: a discovered predicate
that is in neither set is a finding, and so is a recorded name the census stops
discovering.

Structure tests, via `cargo test --test <name>` for each of the 26 files
matching `tests/*_structure.rs`: 529 passed, 0 failed, and 26 of the 26 report
`0 failed`.

## Remaining scope

The #562 assertion and #761 loader completion are executable in the aarch64
testing profile. The post-loader soft lockup above is open and attributed to
#728. Indexed ext2 directories are supported by the reader that this round
measured, so #778 is closed. Results for userspace programs after loader
release remain owned by their individual test criteria rather than these two
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
