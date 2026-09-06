# #766 -- a timer wake dispatches only after a full round robin (x86)

Round: R157 small-PR mode, 2026-09-06. Branch `fix/766-x86-timer-wake-dispatch`,
based on `origin/main` @ `783a6a53`. Line numbers in the RCA below are read off
that commit unless stated otherwise.

Issues in view: **#766** (this one), **#631** (the `clock_gettime_test` Test 3
sub-millisecond red, closed with its failure attributed to #766's overrun
distribution).

---

## 1. RCA -- one timer wake, traced end to end on x86

### 1.1 The chain

A thread calls the kernel-side sleep primitive
(`Scheduler::block_current_for_timer`, `kernel/src/task/scheduler.rs:3819`).
That sets `ThreadState::BlockedOnTimer`, stores an absolute `wake_time_ns`,
pushes `Reverse((wake_time_ns, tid))` onto `self.timer_heap`, and removes the
thread from the per-CPU ready queues. From there:

| # | Where | What happens |
|---|---|---|
| 1 | `kernel/src/time/timer.rs:19,36` | `PIT_HZ = 200`, so one tick is 5 ms and `MS_PER_TICK = 5`. |
| 2 | `kernel/src/interrupts/timer_entry.asm:109` | Each tick calls `timer_interrupt_handler`. |
| 3 | `kernel/src/interrupts/timer.rs:42-68` | The handler increments `TICKS`, decrements `CURRENT_QUANTUM`, and at `:64-66` sets `need_resched` **only when the quantum reaches 0**. It does not read the timer heap, so a deadline that has come due cannot itself ask for a reschedule. |
| 4 | `kernel/src/interrupts/timer_entry.asm:121` | On IRQ return the stub calls `check_need_resched_and_switch`. |
| 5 | `kernel/src/interrupts/context_switch.rs:155,161,269,360` | That function gates on `per_cpu::can_schedule` (`kernel/src/per_cpu.rs:1364`), then consumes the flag with `check_and_clear_need_resched()` at `:269` and calls `scheduler::schedule()` at `:360`. So the IRQ-return path **does** honour `need_resched` on each IRQ return; the flag is simply raised only at quantum expiry (or by an explicit `yield_current()`). |
| 6 | `kernel/src/task/scheduler.rs:2204` | `schedule()` calls `wake_expired_timers()` first (the #568 ordering). |
| 7 | `kernel/src/task/scheduler.rs:4145-4155` | `wake_expired_timers` peeks the min-heap and breaks as soon as `wake_time > now_ns`, so it visits exactly the entries whose deadline has already passed. |
| 8 | **`kernel/src/task/scheduler.rs:4258`** | **`self.per_cpu_queues[target].push_back(tid);`** -- the woken thread is appended to the **tail** of its target ready queue. |
| 9 | `kernel/src/task/scheduler.rs:2262-2280` | The selection loop immediately below pops the **front** of `per_cpu_queues[current_cpu]`. |
| 10 | `kernel/src/task/scheduler.rs:1351` | `#[cfg(not(target_arch = "aarch64"))] pub(crate) const MAX_CPUS: usize = 1;` -- on x86 there is one queue, shared by the runnable threads, and no peer CPU can pick the wake up. |
| 11 | `kernel/src/interrupts/context_switch.rs:561` -> `kernel/src/interrupts/timer.rs:80-86,33` | The dispatch path calls `reset_quantum()`, which restores `CURRENT_QUANTUM` to `TIME_QUANTUM = 10` ticks. At 5 ms/tick that is a **50 ms slice per thread per turn**. |

### 1.2 The producing line

**`kernel/src/task/scheduler.rs:4258` (at `783a6a53`)** --
`self.per_cpu_queues[target].push_back(tid);` inside `wake_expired_timers`.

A thread that reaches that line is, by step 7, a thread that is *already
late*. Appending it to the tail makes its remaining wait the sum of the quanta
of the threads ahead of it in the single x86 queue. That is the "full round
robin" the issue title names.

### 1.3 The arithmetic, and what "a full round robin" means

Total overrun decomposes into two terms:

* **detection**: deadline -> the next `schedule()` pass. Since `need_resched`
  is only raised at quantum expiry (step 3), this is bounded by the running
  thread's remaining quantum: **< 50 ms**.
* **queue position**: the number of threads ahead of the woken one x 50 ms.
  This term is unbounded in queue length and dominates.

#766's own population census (`serials/693-rca/x86-693rca-tcg3-boot24-late_lost_wake-kernel-20260902.txt`,
quoted in the issue) counts 31 `Added thread` lines against 11
`exited with code` lines before the measured write, i.e. of order 20 live
threads -> a round of order 1 s. The issue's measured distribution (min 84 ms,
median 426.5 ms, p90 2592 ms, max 10318 ms over 324 trials) is that second
term, at various queue depths.

Two things this RCA does **not** claim: it does not separate, in the issue's own
324 trials, how much of each gap is quantum exhaustion versus threads blocking
early; and it does not attribute #772 (a dispatched, already-woken reader that
gives the CPU back without consuming its wake), which is a different mechanism
and stays open.

### 1.4 Cross-check: why aarch64 does not show it

The same `push_back` runs on aarch64, and the same leg measures single-digit
milliseconds there: over the 40 boots of the two strict runs in the round
record's section 4, `overrun_ms` reads 2 to 9 (median 5) in run 1 and 0 to 12
(median 3) in run 2, per-boot lines in
`serials/766/21-shipped-leg-strict-run1-19of20-oracle-lines.txt` and
`serials/766/25-shipped-leg-strict-run2-19of20-oracle-lines.txt`. Three
differences account for it:

1. **`MAX_CPUS = 8`** (`kernel/src/task/scheduler.rs:1349`). The wake is routed
   by `find_target_cpu_for_wakeup` (`:4410`), which falls through to the
   least-loaded CPU (`:4455`), so a late wake joins a short queue instead of the
   one queue the runnable threads share.
2. **A 10 ms quantum instead of 50 ms.** `MS_PER_TICK = 1`
   (`kernel/src/time/timer.rs:38`) with `TIME_QUANTUM = 10`
   (`kernel/src/arch_impl/aarch64/timer_interrupt.rs:39`).
3. **An idle-CPU fast path.**
   `kernel/src/arch_impl/aarch64/timer_interrupt.rs:791-796` sets `need_resched`
   on each tick of a CPU that is running its idle thread, so a wake placed on
   an idle CPU's queue is dispatched within one 1 ms tick rather than at quantum
   expiry. x86 reaches the same outcome for the idle case through
   `can_schedule`'s `returning_to_idle_kernel` term (`kernel/src/per_cpu.rs:1477`);
   what x86 has no equivalent of is (1) and (2).

Product: on aarch64 the tail enqueue costs of order one 10 ms quantum on a short
queue. On x86 it costs a whole round of 50 ms slices.

---

## 2. The fix

One line, at the producing line, plus one counter.

```rust
// kernel/src/task/scheduler.rs, in wake_expired_timers
self.per_cpu_queues[target].push_front(tid);      // was push_back(tid)
ENQUEUE_TIMER_WAKE.fetch_add(1, Ordering::Relaxed);
```

**Shape**: a wake whose deadline has already passed is dispatched at the HEAD of
its target ready queue. The wait is then bounded by the running thread's
remaining quantum plus one tick of granularity, because the pass that detects
the expiry is `schedule()`'s own and its selection loop pops the front of that
queue a few lines later:

* x86: `10 ticks * 5 ms + 5 ms` = **55 ms**
* aarch64: `10 ticks * 1 ms + 1 ms` = **11 ms**

**Quantum policy for ordinary preemption is unchanged.** The outgoing thread is
still re-enqueued at the tail by `schedule()`
(`kernel/src/task/scheduler.rs:2250`); a promoted thread is preempted on the
same quantum as anything else and then goes to the tail itself; only a thread
that actually slept is promoted, and only once per wake.

**Why not "the wake sets `need_resched`".** That was the other candidate in the
issue's own list of directions. It was rejected for two reasons. First it treats
the smaller term: detection is already bounded by one quantum, and the term that
produced p90 2592 ms is queue position. Second, `wake_expired_timers` is called
from inside `schedule()` itself (step 6), which has already consumed the flag at
`context_switch.rs:269`; re-raising it there would leave the flag set for the
thread `schedule()` is about to dispatch, so that thread would be preempted at
the very next tick. That is a change to the quantum policy for ordinary
preemption, which this round is required to leave alone.

**Hot-path cost at source level.** One `VecDeque::push_front` in place of one
`VecDeque::push_back` -- both O(1) writes at opposite ends of the same ring
buffer -- plus one relaxed 64-bit increment. No lock, no allocation, no
formatting, no page-table walk, no I/O. The function is not an interrupt
handler; it runs under the scheduler lock from `schedule()` and from the
blocking-syscall wait loops.

**Not claimed.** Deadline order among threads promoted in the same pass: the
heap pops earliest-deadline-first and each pop goes to the front, so a pass that
promotes several reverses their order relative to one another. They are already late, and a
promoted thread does not wait on the others' quanta, so the property the fix is
for is unaffected; earliest-deadline dispatch would be a separate change with
its own evidence.

The change is applied on both architectures rather than under a `cfg`. On
aarch64 the target queue is usually short or empty, so head-vs-tail is mostly
indistinguishable there; keeping one behaviour keeps the two arches' dispatch
rule readable as one rule, and the aarch64 gates are run against it (section 5).

### 2.1 Cross-class fairness -- what the promoted thread costs the queue it jumps

The `Not claimed` paragraph above is about order WITHIN one
`wake_expired_timers` pass. This subsection is about the other question, raised
by review as `unbounded-head-priority-for-rearming-timers`: what repeated
promotion costs the threads a wake is promoted PAST. A thread that sleeps for a
short period, wakes, and sleeps again is promoted on each of its wakes, so the
question is not answered by looking at one pass.

**What the change really does.** The ready queue stops being strict FIFO. Two
classes now share it: the threads `wake_expired_timers` promotes (by
construction, threads whose deadline has already passed) and everything else,
which includes the outgoing thread `schedule()` re-enqueues at the tail. Before
the change a waiting thread's position was monotonically non-increasing until
it was dispatched, so its wait was bounded by the threads already ahead of it.
After the change a promotion can insert ahead of it, and its position can grow.
That is a real loss of a FIFO progress property and it is not bounded by the
queue's length.

**What bounds it instead, on a uniprocessor.** On x86 `MAX_CPUS` is 1
(`kernel/src/task/scheduler.rs:1351`), and a promotion happens only inside a
`schedule()` pass whose own selection loop then dispatches the promoted thread.
So a thread that was jumped is delayed only by the promoted thread actually
RUNNING. Take a CPU-bound thread T waiting with `j` threads ahead of it, a
quantum `Q`, and let `u` be the fraction of a window of length `t` that the
timer-wake class occupies the CPU. T is selected once the `j` threads ahead of
it and the promotions interposed since have both had the CPU, which gives

```
j * Q + u * t = t        ->        t <= j * Q / (1 - u)
```

Under a tail enqueue the same thread waits at most `j * Q`, because promotions
go behind it. So the head enqueue inflates a CPU-bound thread's worst-case wait
by a factor of `1 / (1 - u)`, with `u` the CPU the timer-wake class consumes in
the same window.

Two things follow, and they are the answer to the review's "no kernel-side
bound":

* The inflation diverges only as `u` approaches 1. At `u = 1` the timer-wake
  class occupies the CPU outright, and T gets no CPU under a TAIL enqueue
  either, so a workload that starves T after this change starves it before the
  change as well. What the head enqueue takes from T is its ORDER, not its
  share.
* `u` is workload-controlled and the kernel applies no admission control to it.
  But a thread that sleeps and immediately sleeps again contributes to `u` only
  the time between its wake and its next block, which for that shape is
  microseconds; a thread that sleeps and then computes contributes its compute
  and is a CPU-bound thread for most of the window anyway. The population the
  review names -- many short-period re-armers -- is the population with the
  smallest `u` per member.

**No aging term was added, and this round does not claim one.** The inequality above is the
whole of the bound, and it comes from the promoted thread having to run rather
than from any fairness mechanism in the scheduler. A workload that needs T's
ORDER protected rather than only its share needs a scheduler policy change with
its own evidence; this round does not make one.

**What is measured rather than derived.** The oracle in section 3 runs
`REARMERS = 4` re-arming threads at a 10 ms period against `PEERS = 8`
CPU-bound peers and reports `peer_max_gap_ms`: the worst interval any of those
peers spent off the CPU during the window, measured by the peers themselves
from consecutive clock reads in their own spin loops. The readings are in the
round record. That is one point of the design space, not the sweep over
re-armer counts and periods the inequality would need to be confirmed as a
curve, and `peer_gap_bound_ms` is a starvation ceiling rather than a latency
certification.

**On aarch64 the argument is different and is not made.** With `MAX_CPUS = 8`
the wake is routed by `find_target_cpu_for_wakeup` to a least-loaded CPU, so the
displaced thread need not be on the promoting CPU at all. The serialization
step above is the x86 one; the aarch64 arm of the oracle is read as a
regression guard, as section 1.4 says of the latency reading.

---

## 3. The oracle

`kernel/src/task/timer_wake_oracle.rs`, `boot_tests`-only, called from
`kernel/src/main.rs` (x86, after the kthread lifecycle tests) and
`kernel/src/main_aarch64.rs` (after the pinned-placement census).

`REARMERS = 4` kthreads each sleep `REARMS = 8` times in a row, 10 ms at a
time, against absolute monotonic deadlines they compute themselves; `PEERS = 8`
CPU-bound kthreads are runnable while they do. Two numbers come out:

* `overrun_ms` -- the worst `wake_instant - deadline` of those 32 sleeps, both
  read with `crate::time::get_monotonic_time_ns()` (post-#767 units), with
  `deadline` the same value handed to `block_current_for_timer`, so the interval
  is anchored to the kernel's own deadline rather than to a later clock read.
  This is #766's quantity.
* `peer_max_gap_ms` -- the worst interval any of the 8 peers spent off the CPU
  during the same window, each peer taking the maximum over consecutive clock
  reads in its own spin loop and the leg reporting the maximum over the peers.
  This is section 2.1's quantity. A peer is runnable throughout, so an interval
  longer than a spin batch is time it did not have the CPU.

The re-arming is what makes the second number mean anything: a thread that
sleeps once is promoted once, and the displacement section 2.1 is about only
accumulates across repeated wakes.

Marker:

```
[TIMER_WAKE_LATENCY_ORACLE:<arch>:sleep_ms=10:peers=8:rearmers=4:rearms=32:overrun_ms=N:bound_ms=100:quantum_ms=Q:round_ms=R:peer_max_gap_ms=G:peer_gap_bound_ms=B:wake_enqueues=N:peers_started=8:peers_spinning=8:backstops=0:setup_ms=S:window_ms=W:measured=1:PASS|FAIL]
```

`PASS` requires each of: the window was measured; 4 of 4 re-armers finished and
32 of 32 sleeps completed; the wakes went through the timer-wake enqueue site
(`wake_enqueues >= 4`); 8 of 8 peers were spinning when the sleeps started; the
backstop count is 0; `overrun_ms <= bound_ms`; and
`peer_max_gap_ms <= peer_gap_bound_ms`.

`bound_ms = 100` is a kernel constant. The **mechanism** bound is 55 ms on x86
and 11 ms on aarch64 (section 2); the remainder is an allowance for an emulated
periodic timer being delivered late, and is not part of the mechanism claim.
`quantum_ms` and `round_ms` are printed so the arithmetic can be redone from the
line: `round_ms = peers * quantum_ms` is the *arithmetic* cost of a tail enqueue
(400 ms on x86, 80 ms on aarch64), not a measurement -- a real round also
carries whatever else is runnable in that boot window.

`peer_gap_bound_ms` is derived in the kernel the same way:
`(PEERS + REARMERS + 2) * QUANTUM_MS * 4`, which is 2800 ms on x86 and 560 ms on
aarch64. The factor of four is a deliberate allowance, so the field is a
ceiling on a peer being dispatched AT ALL and no claim is made that a reading
near it would be acceptable.

### The barrier

Two earlier versions of this file are worth recording, because the cost they
measured is what the shipped barrier exists to avoid.

**Version 1 (no barrier).** The peers started spinning the moment they were
created. That does not work: creating a kernel thread allocates and maps a
kernel stack, the boot thread doing the creating is itself preemptible, and the
peers already spinning slow each later creation down. Measured on the x86
boot-test gate, that version created its 8 peers across ~16 s of guest time
(`Added thread 1205 't766_peer'` between the `ms=383874` and `ms=384877` strand
census lines; `Added thread 1212 't766_peer'` between `ms=400041` and
`ms=401061`) and reported

```
[TIMER_WAKE_LATENCY_ORACLE:x86:sleep_ms=10:peers=8:overrun_ms=45:bound_ms=100:quantum_ms=50:round_ms=400:wake_enqueues=1:peers_started=8:backstops=7:measured=1:FAIL]
```

-- 7 of the 8 peers had already passed their own 5 s spin backstop and exited
before the sleeper reached its measurement, so the 45 ms it reported was
measured against almost no contention. That serial was not preserved (the run's
output directory was recycled before the reading was understood); the marker
line and the two census timestamps above are what this document has of it, and
they are quoted here as the reason the barrier exists, not as evidence for any
claim about the fix.

**Version 2 (barrier, `halt`).** The peers parked on a `MEASURE_OPEN` flag with
`arch_halt_with_interrupts()` until the boot thread opened the barrier. That is
much worse, and the reason is worth writing down: a `halt` does not take the
thread out of the ready queue. A parked peer was still dispatched, woke on each
5 ms tick, checked its flag and halted again -- holding its whole 50 ms quantum
per turn, exactly as a spinning peer would. On the x86 boot-test gate the 8
creations then took **362 s of guest time** (first peer between the strand
census lines at `ms=399901` and the next; last peer between `ms=762409` and its
neighbours, and the PIT tick counter moves 44396 -> 116339 across the same span,
which is 71943 ticks x 5 ms = 359.7 s independently of the TSC), and the run
reported `overrun_ms=45 ... backstops=15 ... FAIL`, its backstops being the 30 s
and 60 s liveness budgets expiring during a setup phase that should have taken
under a second. Serial preserved: `serials/766/02-x86-...` is the version-3 red
run, not this one; version 2's own serial was recycled, and the numbers above
are quoted from the census lines read out of it at the time.

**Version 3 (shipped).** A peer parks with `kthread_park()`, which blocks it and
takes it out of the ready queue, so the creations run at full speed. The boot
thread opens the barrier once `PEERS_STARTED == PEERS`, unparks in a retry loop
(a peer that entered `kthread_park()` after a single unpark would otherwise
block with no remaining waker), and the sleeper waits for
`PEERS_SPINNING == PEERS` before its sleep. Measured setup cost on the x86
boot-test gate: `setup_ms=520` for the 9 creations, against 362 000 ms for
version 2. `peers_spinning=8` is pinned by both gates; `setup_ms` and
`window_ms` are printed for the reader.

---

## 4. Evidence

See `766-ROUND-RECORD-2026-09-06.md` in this directory for the run-by-run
receipts (gate serials, mutation table, checker counts, and what is not
claimed).

---

## 5. Where the leg is read

| Gate | What it asserts |
|---|---|
| `docker/qemu/run-x86-boot-tests.sh` | marker present exactly once, and matching the x86 PASS pattern exactly once; the pattern carries three anti-vacuity preflights (it must reject a `FAIL` line at `overrun_ms=2592`, accept a real `PASS` line at `overrun_ms=41`, and reject a `PASS`-shaped line at `rearms=4`) |
| `docker/qemu/run-x86-prod-profile-boot-test.sh` | `[TIMER_WAKE_LATENCY_ORACLE:` count 0 (`TEST_ONLY_MARKERS`) |
| `docker/qemu/run-aarch64-boot-test-strict.sh` | marker matches the aarch64 PASS pattern, which carries the same three anti-vacuity preflights; a `:FAIL` line is a separate, named red; the literal is also in `require_boot_tests_kernel`'s profile census |
| `docker/qemu/run-aarch64-prod-profile-boot-test.sh` | `[TIMER_WAKE_LATENCY_ORACLE:` count 0 |
| `tests/timer_wake_dispatch_structure.rs` | the shape: `push_front` and no `push_back` inside `wake_expired_timers`; the oracle's `QUANTUM_TICKS` equals the `TIME_QUANTUM` literal in both timer handlers; the oracle is `boot_tests`-only and called from both mains; the four gates above name the marker; `REARMERS > 1` and `REARMS > 1`, the four fields section 2.1's reading is carried on, and both boot gates pinning the re-arm count computed from the oracle's own constants. 6 of 6 assertion bodies carry a `#[should_panic]` mutation leg that reddens them. |
