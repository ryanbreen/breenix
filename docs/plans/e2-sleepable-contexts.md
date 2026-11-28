# E2: sleepable contexts — design and PR plan

Ruling DEV-37(2), from the toy-OS audit's R4 and the virtual-memory part of R12 (theme T1). Written
2026-11-27 against `origin/main` at f52fd83c. This is a design document. It changes no code, and E2 starts
coding only after it merges (DEV-37(2)).

E2's finish line: every syscall and kernel worker runs in a task context that may sleep. Interrupts are
masked only around bounded sections. Every blocking primitive goes through one park contract with an atomic
check-and-park. Page faults can sleep and restart without PROCESS_MANAGER. Block I/O has one caller rule.
BreenFS does no device wait while holding its mount lock. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

## 0. Provenance

Clean room (O6, DEV-34(5), DEV-36(2)). The waitqueue (assessment area A1) is SUBSTANTIVE. For it, this
design used only brief B1 (`docs/cleanroom/derivation-assessment.md`, Part B), the public signatures of
`kernel/src/task/waitqueue.rs` (its `pub` lines) and its callers. It did not read that file's function
bodies, the bodies of `block_current_for_io_publish`, `block_current_for_io_with_timeout` or
`wake_waitqueue_thread` in `kernel/src/task/scheduler.rs`, the `docs/planning/f32*` documents, or Part A of
the assessment. Everything else here comes from:

- our own code, which was read freely apart from the bodies listed above;
- the Intel 64 and IA-32 SDM and the Arm Architecture Reference Manual;
- the AHCI 1.3.1 specification and POSIX.1-2017;
- the textbooks and papers listed in §13.

No other operating system's source was read. The audit, its sched/SMP lane findings
(`~/Library/Caches/fastr-rollout/2026-11-27/toy-os-audit/sched-smp-findings.md`) and rulings DEV-23 to
DEV-37 set the scope.

## 1. What the code does today

### 1.1 Syscall entry

- **x86-64.** Both the SYSCALL path (FMASK clears IF) and the INT 0x80 path (`cli`,
  `kernel/src/syscall/entry.asm:17-43`) enter Rust with interrupts masked. `rust_syscall_handler` takes a
  preemption level first (`kernel/src/syscall/handler.rs:189`) and drops it as the last step of the
  return path (`handler.rs:627`). Interrupts stay masked for the whole syscall unless the code unmasks them
  itself. Fork does this for its page-table copy, still under the brake
  (`kernel/src/syscall/handlers.rs:1697-1707`).
- **ARM64.** Entry unmasks IRQ and FIQ once the frame is saved
  (`kernel/src/arch_impl/aarch64/syscall_entry.S:203`). It then takes the same syscall-wide level
  (`kernel/src/arch_impl/aarch64/syscall_entry.rs:101`, released at `:247`).
- **Where the count lives.** The preemption count is per CPU (`kernel/src/per_cpu.rs:1026-1108`). A
  syscall that sleeps must drop its level before it switches and take it back afterwards. Each blocking
  primitive does that arithmetic itself (§1.2).
- **Kill handling.** This no longer depends on the brake: `KillCustody::enter_syscall`
  (`kernel/src/task/thread.rs:1278-1300`) keeps SIGKILL out of a running syscall on its own. E2 keeps it. E2.10a replaces only its kill-claimed
  wait, which does level arithmetic, with a park (§4.4).

### 1.2 Each blocking primitive guesses whether its caller may sleep

| Primitive | Its predicate | What it does when the answer is no |
|---|---|---|
| AHCI port admission | `can_park_for_port_io`: a current non-idle thread, outside interrupt context, at most one preemption level, and the ARM64 timer running (`kernel/src/drivers/ahci/mod.rs:427-436`) | x86 returns `CannotPark` (`:474`). ARM64 spins with `yield` (`:477`). A caller at level 0 first *adds* a level so the wait can remove one (`:487-490`) |
| AHCI completion mode | `scheduler_sleep_ready` and the poller test (`mod.rs:1228-1248`, `:1985-1986`) | Polls |
| `Completion` wait | `syscall_sleep_path_available` (count > 0 means "a syscall") and `idle_halt_wait_available` (`kernel/src/task/completion.rs:150-179`) | Polls, or halts as idle (`:190-260`) |
| BreenFS `MountLock` | `can_sleep` (`kernel/src/fs/breenfs/mount_lock.rs:222-238`) | Spins for good (`:136-138`) |
| BreenFS preemption points | `can_yield`: count exactly 1 (`kernel/src/fs/breenfs/preempt.rs:26-34`) | Does not yield |
| Syscall prepared waits | `release_preempt`: "a level is held" (`kernel/src/syscall/blocking_io.rs:40-41`) | Sleeps without releasing a level |
| ext2 lock park (#728) | Its own conditions (`kernel/src/fs/ext2/mod.rs:1580`, `:1677`) | Spins |
| Wait-loop halt | `arch_halt_while_blocked`: count 0 (`kernel/src/lib.rs:630-700`) | Halts or WFIs as the current thread and keeps the CPU |

Identical storage work can therefore succeed, park, poll or fail depending on who is asking. DEV-24 was
the result: init, at preemption level 2, was refused while `breenfs-commit` owned port 0, and the box came
up with no `/sbin/init`. #3228 moved the x86 init load into a kthread (`kernel/src/main.rs:2140`). ARM64
boot still holds a level from `kernel_main` (`kernel/src/main_aarch64.rs:1277`) until init is launched,
and the AHCI comment at `mod.rs:424-426` says this is why ARM64 has to poll. Two more ARM64 boot paths
read storage at a level:

- the production fallbacks run on the boot continuation after SMP bring-up: a late `read_init_from_ext2`
  and `run_userspace_from_disk("init_shell")` (`main_aarch64.rs:2039-2109`), and PID 1 is then launched
  from that context rather than through the scheduler;
- the testing profile's `test-loader` kthread takes a level on purpose across its ELF reads
  (`main_aarch64.rs:2138-2154`), so that the test processes it creates cannot take its CPU between
  binaries. Its reads park today because the AHCI and completion predicates accept one level.

### 1.3 Page faults

- Faults find their process by a linear scan on the page-table root
  (`kernel/src/memory/anon_map.rs:45`, `:71`; `kernel/src/memory/file_map.rs:2304`, `:2312`), under
  PROCESS_MANAGER.
- The fault paths take that lock through a bounded poll. When the poll runs out they report the fault
  `Resolved`, and the instruction faults again (`kernel/src/interrupts.rs:884-901`, `:980`). That is the
  audit's T8 "failure becomes success" pattern.
- A fault while a fork copies the address space also returns `Resolved` and faults again
  (`anon_map.rs:34-36`).
- File faults never do I/O, by rule (`file_map.rs:9-14`, `:27-30`). That is why mmap reads the whole range
  before it returns (R6). <!-- claim-lint:ok: states the rule in kernel/src/memory/file_map.rs at f52fd83c. -->
- Signal frames are written under PROCESS_MANAGER with interrupts masked (`file_map.rs:23-24`).
- The x86 #PF handler runs on the faulting thread's own kernel stack, not an IST stack
  (`interrupts.rs:122-133`). So a fault can sleep without moving its frame.

### 1.4 BreenFS

A mount has one `MountLock<MountState>` (`kernel/src/fs/breenfs/vfs.rs:291`). Its holder reads nodes and
data through the buffer cache, and sleeps on device reads while holding it (`mount_lock.rs:1-9`). Every
getattr, lookup, open, readdir and read takes that lock (audit R10). A contender that cannot sleep spins
for good (`mount_lock.rs:136-138`). `vfs.rs:4945` turns an AHCI `CannotPark` into `EAGAIN` for userspace. <!-- claim-lint:ok: read from kernel/src/fs/breenfs/mount_lock.rs at f52fd83c. -->

### 1.5 What already holds and stays

- **`KillCustody`.** Kill custody is already separate from the lock depth (`thread.rs:1253-1300`).
- **Durable interrupt wakes (R17, #3238).** Each task has a preallocated wake node and a pending bit, and
  wakes publish to the owner CPU's inbox. `isr_unblock_for_io` stays the interrupt-context wake. The new
  event queue calls it (B1 "What it replaces").
- **Two existing orderings.** The completion token's publication and recheck, and `IrqSafeMutex`'s rule of
  unlocking before it restores the interrupt state (`kernel/src/irq_safe_mutex.rs:1-10`). The sched lane
  rated both sound.
- **x86 run-queue ownership.** Owner-local run-queue locks with remote try-lock and revalidation. E9
  generalises them, and E2 does not touch them.

## 2. Invariants E2 establishes

Each invariant is checked by a deterministic test named in §9.

- **I1, explicit sleepability.** One function decides whether the current context may park (§3.1). No
  primitive computes its own answer.
- **I2, atomic check-and-park.** A waiter evaluates its condition and publishes its parking state in one
  critical section with respect to the queue's wakers (B1 requirements 1-2).
- **I3, switches happen at depth 0.** A context switch out of a task happens only when that task's
  preemption depth equals its baseline (§3.2). The scheduler's own lock hand-off is the one exception.
- **I4, preemption never commits a sleep.** If a task is preempted after publishing a parking state but
  before committing it, it stays runnable. Only the park itself dequeues it (§3.5). <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **I5, no silent success.** A wait that cannot happen returns an explicit error. It never returns zeros,
  `Resolved`, `EAGAIN` or a polled success standing in for a sleep. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **I6, faults are off PROCESS_MANAGER.** A page fault reaches its address space directly and takes only
  that address space's locks (§5).
- **I7, no device wait under a filesystem-wide lock.** BreenFS submits and waits for device requests with
  no `MountLock` held. The exceptions are a mount that has not been published yet and the commit workers'
  `PublishToken`, which is not a `MountLock` and which no foreground operation takes (§7.3a).
- **I8, polling only at early boot.** A caller polls only while it holds the early-boot I/O token, which
  stops being issued when the scheduler first dispatches (§6). A device with no usable interrupt still has
  a poller thread, but that thread is the device's completion source. It is not a mode the caller picks.

## 3. The park contract

### 3.1 Contexts

```
enum ParkContext {
    Sleepable,                // may park
    Atomic(AtomicReason),     // must not park; a park here is a contract violation
    EarlyBoot,                // no scheduler yet; only the early-boot I/O token may poll
}
enum AtomicReason { HardIrq, SoftIrq, Nmi, Idle, NoThread, LocksHeld(u8), IrqsMasked }
```

`park::context()` is the only implementation of this decision. Its inputs are:

- whether a current thread exists, and whether it is idle;
- the HARDIRQ, SOFTIRQ and NMI fields;
- the CPU's preemption depth against the thread's baseline (§3.2);
- the interrupt mask against the baseline's mask;
- whether the scheduler has started dispatching.

It replaces the eight predicates in §1.2. Callers name what they need: `park::wait*` for a sleep, and
`park::require_sleepable()` for code that is about to do something that may sleep, such as block I/O or a
user copy that can fault. A violation writes a fixed-size receipt (the site's return address, the reason,
the thread ID and the depth) into a per-CPU ring, and the call returns an error. Diagnostics stay on in
every build and are written out only when something fails (operator, 2026-11-05). The receipt never
substitutes polling for the sleep. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 3.2 Depth and baseline

Until §4.4 removes the syscall brake, a syscall that still carries it legitimately holds one level. Today
that is inferred from "count == 1" or "count > 0", which also matches a kthread holding one spin lock. E2
records it instead. Each thread gets `park_baseline: u8` and `baseline_irqs_masked: bool`:

- syscall entry sets the baseline to 1 for a braked class and 0 for a preemptible one (§4.4);
- kthreads and workers run with baseline 0;
- a kthread that needs a syscall's shape, holding its CPU except while it sleeps, takes a
  `park::Braked` guard. The guard raises the depth and the baseline together by one, and dropping it
  lowers both. A park inside the guard releases that level as it does for a braked syscall. The ARM64
  `test-loader` pin (§1.2) becomes this guard in E2.4. Without it, the loader's reads would be
  `LocksHeld(1)` at baseline 0 and would fail;
- the mask flag is true for an x86 syscall until §4.3.

`LocksHeld(n)` means the depth exceeds the baseline by n. A park releases exactly the baseline before it
switches and restores it after (B1 requirement 9). Callers do no further arithmetic. The "add a level so
the wait can remove one" step (`ahci/mod.rs:487`, `mount_lock.rs:141-144`) leaves the callers.

Until E2.3, one copy of that step lives inside `park::wait`. Today's publication protocol publishes a
blocked state only at depth 1 or more. The reason is that ARM64's interrupt exit switches out a thread
it sees blocked at depth 0 and leaves it off the run queue as a blocked thread (§3.5). So in E2.2 a baseline-0 caller's `park::wait`
raises one level from publishing `Parking` to its switch, and the switch releases it. E2.3 makes an
involuntary switch requeue a `Parking` thread, and deletes that internal level. Because of this,
E2.4 and E2.5, which depend only on E2.2, do not publish `Parking` at depth 0 on a scheduler that would
strand it. Because the baseline is
stored in the thread and not in the per-CPU count, it survives migration across a sleep. E9 owns the count
layout (brief B3), and E2 reaches the count only through `preempt::depth()` and guard types.

### 3.3 The protocol

The event-queue half is brief B1, rewritten in PR E2.1. The park half wraps it.

```
park::wait(queue, cond, deadline, mode) -> WaitEnd
  0. ctx = context(); if ctx != Sleepable -> receipt, return Err(NotSleepable)
  1. reserve the waiter node; failure -> return NoMemory (nothing published; B1 req. 3)
  2. seq := ++thread.wait_seq
     lock queue; if cond() -> unlock, return Condition
     link the node tagged seq; state := Parking(seq, mode, deadline); unlock   (B1 req. 1-2)
  3. commit, one scheduler transaction (the owning CPU's run-queue lock, interrupts masked):
       state is Runnable(seq, reason)      -> end reason   (a wake, signal or deadline landed first)
       deadline has passed                 -> end Timeout
       mode admits a pending signal        -> end Interrupted or Killed          (#1230 order)
       otherwise: state := Parked(seq); arm the one-shot deadline;
                  release the baseline (and E2.2's publication level, §3.2); switch out
  4. on every end, from step 2 on: unlink each node tagged seq, cancel the deadline, state := Running,
     restore the entry depth and the entry interrupt mask                         (B1 req. 6, 9)
  -> WaitEnd::{Condition, Woken, Timeout, Interrupted(EINTR | ERESTARTSYS), Killed, NoMemory}
```

- **Wait identity.** `wait_seq` is a per-thread counter, distinct from the process-manager `WaitEpoch`
  that `tests/wait_epoch.rs` pins. Each node, deadline and wake carries the `seq` it was made for. Every
  end of a wait is one transition, `Parking(seq)` or `Parked(seq)` to `Runnable(seq, reason)`, made under
  the owning CPU's run-queue lock. A transition whose `seq` does not match the thread's current one, or
  that finds the thread already `Runnable` or `Running`, does nothing. So the first of wake, deadline,
  signal and kill to reach the thread wins and records its reason, and the rest are no-ops. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Arrivals before the commit.** A signal sender, the deadline and a waker all make that transition
  whether the thread is `Parking` or `Parked`. A signal sent between step 2 and step 3 is therefore seen
  either as `Runnable(seq, Interrupted)` when the commit looks, or by the commit's own pending-signal
  check. The two orders end the same way. The same holds for a deadline that passes before the commit:
  either the commit sees it has passed, or the timer, armed in the commit, fires at once and makes the
  transition. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Rollback.** Step 4 runs on every exit after step 2, including `Condition` from step 2 itself and the
  ends from step 3 that never switched. It restores the entry depth and the entry interrupt mask, not
  merely the baseline, so a caller that entered with interrupts masked (an x86 syscall until §4.3) leaves
  with them masked. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Assertions.** In testing builds, the commit and each transition write a receipt if they find a
  state other than the ones above for their `seq`. They sit at the scheduler boundary, in the run-queue
  code, not in the callers.

- **Wakers.** A waker makes the condition true with a release store, then takes the queue lock. There
  are three kinds, and each keeps its own lock order (B1 requirement 5):
  - *Task context.* It hands each chosen node to the scheduler, which makes the transition to
    `Runnable`. It unlinks the node only after the scheduler has accepted the wake (B1 requirement 7).
  - *Deferred.* For callers that hold the process manager or otherwise may not enter the scheduler lock,
    such as BreenFS's commit signal (`kernel/src/fs/breenfs/commit_signal.rs:33-36`) and the 19
    `wake_up_deferred` sites B1 counts. It takes the queue lock, selects the nodes, and hands each one's
    TID to the durable per-task inbox (R17), the same transport `isr_unblock_for_io` uses. It never takes
    the scheduler lock. The owner CPU's inbox drain makes the transition. It is not routed through the
    task-context waker. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - *Hard IRQ.* It try-locks the queue. On success it selects the nodes and hands their TIDs to
    `isr_unblock_for_io` (B1 requirement 4). On failure it has selected nobody, so it has no TID to hand
    over. Instead it adds its count to the queue's `irq_wakes_owed` counter, then tries the lock once
    more. Every queue unlock reads `irq_wakes_owed` after releasing the lock and, if the count is non-zero,
    takes it and performs those wakes as a task-context waker. The counter's store and the unlocker's load
    are the §3.4 store-buffering shape, so both sides use `park::announce` and `park::should_wake`. The
    task that holds the lock when the try-lock fails therefore owns the retry. The obligation lives in the
    queue, not in a per-driver fallback. B1's try-lock contract, "wake up to `max` or leave the queue
    unchanged", still holds: the queue's links are unchanged, and only the counter moves. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

  The GPU interrupt's second wake path, a direct wake of its reaper thread after the try-lock
  (`kernel/src/gpu/mod.rs:2731-2735`), exists because a failed try-lock wakes no waiter today. E2.1 keeps it
  until its own tests show the owed-wake counter covers that case, then removes it in a separate commit.
  The run-queue lock in step 3 is the owner-local run-queue lock on x86 and, until E9, the global
  `SCHEDULER` lock on ARM64 (audit R18).
- **Waiting on several queues.** `park::wait_any` takes one `seq` for the whole wait. It registers a node
  tagged `seq` on each of its queues, checking that queue's condition under that queue's lock, and
  publishes `Parking(seq)` once. It then runs the same step 3 once and the same step 4, which unlinks
  every node tagged `seq`. A wake through any one queue makes the transition, and wakes through the
  others find it made. It is the `add_waiter` / `take_waiter` shape from B1's caller table, used by poll,
  select, epoll and the GPU and console multi-waits. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Timeouts.** A timeout and a wake race to make the transition. Whichever makes it first records the
  end reason, so the wait ends once (B1 requirement 8).
- **Signals and kill.** An `Interruptible` wait ends on any pending unblocked signal. A `Killable` wait ends
  only on SIGKILL. An `Uninterruptible` wait ends only on its condition or its deadline. Kill delivery stays
  with `KillCustody`.

### 3.4 Memory ordering

The queue lock carries the waker-before-waiter ordering. The lock's acquire and release order the
condition store and the waiter's load, on x86 under the SDM's memory-ordering rules and on Arm under the
Arm ARM B2 memory model.

Some wakers skip the lock when nobody is waiting, as `MountLock::release` does
(`mount_lock.rs:116-125`). That shape is the store-buffering litmus test:

- the waker stores the condition and then loads the waiter count;
- the waiter stores the count and then loads the condition.

Both sides need a full barrier between their store and their load. x86-TSO lets a load pass an older store
to a different location, so x86 needs a locked instruction or MFENCE. Arm needs `DMB ISH`. Today
`MountLock` gets this from `SeqCst`. The contract gives one helper, `park::announce` with
`park::should_wake`, which carries the barrier on both architectures, so no caller places it by hand.

### 3.5 Parking and parked (invariant I4)

- **Today.** ARM64's interrupt exit switches out a thread whose state is blocked at depth 0, whether or not
  that thread has reached its own switch (`kernel/src/arch_impl/aarch64/context_switch.rs:5405-5420`). A
  spin lock taken between publishing and committing is then held by a thread that may never run again
  (#1163, repaired case by case in #1155). x86's `can_schedule` leaves BlockedOnIO out for a related
  reason (`kernel/src/interrupts/context_switch.rs:243-281`). <!-- claim-lint:ok: read from kernel/src/arch_impl/aarch64/context_switch.rs at f52fd83c. -->
- **Under E2.** `Parking` is a published but uncommitted state. An involuntary switch out of a `Parking`
  thread treats it as runnable and requeues it. A wake that lands meanwhile turns it into `Runnable` and
  `Woken`. Only the commit, step 3 of §3.3, dequeues the thread.
- **Halting as current.** Halting or WFI-ing as the current thread survives only in idle, in the early-boot
  polled path and in x86's existing "pinned here and nothing else is queued" case
  (`kernel/src/lib.rs:638-644`). That case is equivalent to running idle. Non-preemptible callers of
  `arch_halt_while_blocked` become contract violations. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

## 4. Sleepable syscalls and workers

### 4.1 Lock types

| Type | Acquire does | For |
|---|---|---|
| `sync::SpinLock<T>` | Raises preemption depth, then spins | Short sections not taken in hard IRQ |
| `sync::IrqSpinLock<T>` | Masks local interrupts, raises depth, spins; releases before restoring the mask (today's `IrqSafeMutex` ordering) | Sections shared with hard IRQ |
| `sync::SleepLock<T>`, `sync::SleepRwLock<T>` | Parks on contention through `park::wait`; holding one does not raise depth | Sections that may block: filesystem state, the address-space mapping lock |

Raw `spin::Mutex` and `spin::RwLock` do not change the preemption depth. They appear in 126 files under
`kernel/src` (233 references at f52fd83c; `IrqSafeMutex` appears in 31 files). Under the brake this hides a
hazard: a kthread at depth 0 holding a raw lock can be preempted while another task spins on it on the same
CPU (the single-CPU x86 deadlock found in #1394). Once the brake goes, the same hazard reaches syscalls.
E2.6 makes the three types above the only kernel locks.

### 4.2 The lock validator

Testing builds give each lock a class (its static, or its type's field) and record two things in fixed
tables:

- whether the class has been taken in hard IRQ, and whether it has been taken with interrupts enabled;
- each held-to-acquired edge between classes.

A class taken both in hard IRQ and with interrupts enabled raises a receipt. So does an edge that closes a
cycle (ordered resources: Havender 1968; OSTEP "Common Concurrency Problems"). So does a `SpinLock` or
`IrqSpinLock` held across `park::wait`. The tables are preallocated and the checks allocate nothing. This
is our own design. The validator's logic is pure and gets host tests. In the kernel it runs wherever a
testing kernel runs, including the program's final suite run (DEV-30(2)). <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 4.3 x86 syscalls run with interrupts enabled

ARM64 syscalls already run with interrupts unmasked, so common code reached from syscalls is already exposed
to its own ISRs. The remaining risk is x86-only: x86 ISRs and the locks they share with syscall paths. That
covers:

- the LAPIC timer, the reschedule IPI and the TLB-shootdown NMI;
- AHCI, e1000, BCM5720, xHCI, the serial ports and PS/2 on x86;
- the virtio PCI devices and the GT 710.

E2.7 takes its inventory by reading the code. It lists each x86 ISR's lock set and requires each of those
locks to be an `IrqSpinLock`. A structure test pins the list. Then the Rust handler enables interrupts once
custody is open: frame saved, custody opened, interrupts enabled. The exit sequence in `entry.asm`
(`:117` onward) already masks them before the reschedule check and the register restore, and does not
change. Fork's explicit unmask (`handlers.rs:1703-1707`) is then redundant and goes.

This lands while the brake is still held, so preemption semantics do not change. The risk it retires is
the whole-syscall interrupt blackout on x86:

- device interrupt latency is no longer bounded by the longest syscall;
- the TLB shootdown no longer depends on NMIs because CPUs spin with IF clear in syscalls;
- E9 can later move it to ordinary IPIs (R19).

### 4.4 Removing the brake, one class at a time

A per-syscall attribute table marks each syscall `Braked` or `Preemptible`. Entry raises the level only for
`Braked` syscalls and sets the thread's baseline to match (§3.2). Classes that have not been converted keep
today's behaviour exactly, so each stage stands on its own. A class converts when four things hold:

1. its handlers reach only `sync::` locks (E2.6);
2. its per-CPU reads sit inside a `CpuPin` guard (§4.5);
3. its user copies run in sleepable context or use the nofault copy;
4. it holds no `SpinLock` across a call that may park, which the validator checks.

**Shared entry.** Both architectures call `KillCustody::enter_syscall` at entry for every class. Its
kill-claimed path waits by dropping a level and taking it back (`kernel/src/task/thread.rs:1287-1299`).
A preemptible entry reaches it at depth 0, where that release would underflow. So 10a replaces the loop
with a park: the thread waits on its own custody word, in `Uninterruptible` mode, with the condition
"the claim is withdrawn". The code that withdraws a claim wakes it. A termination finds it parked and
owning nothing, as today's loop leaves it, and switches it away. `park::wait` releases exactly the entry's baseline,
1 for a braked class and 0 for a preemptible one, so custody does no arithmetic and behaves the same for
both classes. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

**Every dispatch arm has a class.** The table is keyed by the dispatchers' `SyscallNumber` arms on both
architectures, not by a hand list. A structure test fails if any arm, x86 or ARM64, has no entry. An arm
whose blocking depends on its arguments is classed by the most blocking form it can take. getrandom
blocks until the pool is ready unless `GRND_NONBLOCK` or `GRND_INSECURE` is set
(`kernel/src/syscall/random.rs:37-44`), so it goes with the waits in class c as a whole. No per-argument
classifier is added. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

Order, least risk first:

- **a. Leaf syscalls:** getpid, gettid, getppid, clock and time reads, and uname: handlers that cannot
  wait for any arguments. They exercise the mechanism first on both architectures.
- **b. File and pipe I/O:** read, write, pread, pwrite, lseek, readv, writev, fsync, getdents, the stat
  family, open, close, pipes and FIFOs.
- **c. Waits:** sockets, poll, select, epoll, futex, nanosleep, clock_nanosleep, the SysV and POSIX IPC
  waits, TTY and PTY, and getrandom.
- **d. Memory:** mmap, munmap, mprotect, brk, madvise and mlock. These follow E2.8 and E2.9, because they
  take the address-space lock.
- **e. Process:** fork, clone, execve, exit, exit_group, wait4, waitid, kill and the signal syscalls. These
  touch PROCESS_MANAGER and the stop and signal machinery.
- **f. The rest.** Each arm still `Braked` after e, which the structure test lists: ioctl and its device
  handlers, mount, umount2, sync, statfs, truncate and ftruncate, and any other arm with no class above.
  These go last, after their handlers are read one by one.

E2.11 starts only when the `Braked` set is empty, which the structure test checks. It then deletes the
`Braked` class, the entry increment, `park_baseline` and the `park::Braked` guard (§3.2, §6). Both
syscall entries run at depth 0.

### 4.5 Per-CPU state across preemption

Once a syscall can be preempted, a CPU index read before a preemption point can be stale after it. Outside
`kernel/src/task`, `kernel/src/arch_impl` and the interrupt handlers, `cpu_id()` is called 9 times in 6
files under `syscall/`, `fs/`, `ipc/`, `net/`, `socket/`, `memory/`, `process/` and `block/`. The
user-copy nofault flag is one of them: `NOFAULT` is indexed by CPU (`kernel/src/syscall/userptr.rs:292`,
`:302`, `:314`). Each of those reads either moves into a `CpuPin` guard, which raises depth for its scope,
or becomes per-thread state. The nofault flag becomes per-thread, because the copy it covers may now
migrate. A structure test pins the remaining unguarded sites with an allowlist that shrinks to zero. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

`cpu_id()` is not the only per-CPU read. The audit covers each per-CPU accessor (GS-relative loads on
x86, `tpidr_el1` loads on ARM64) reached from a converted syscall, including the shared entry and return
code in `syscall/handler.rs` and `arch_impl/aarch64/syscall_entry.rs`, which the directory list above
leaves out. The worst of these is on the x86 return path:

- `rust_syscall_handler` reads this CPU's kernel stack top and then, inside `gdt::set_tss_rsp0`, this
  CPU's TSS pointer, and writes RSP0 through it (`handler.rs:616-621`, and `:310-313` on the sigreturn
  arm; `gdt.rs:253-260`). This runs before `entry.asm` masks interrupts (`:117`).
- Once the syscall is preemptible, it can migrate between loading the TSS pointer and the write. The old
  CPU's RSP0 then points at this thread's kernel stack, and the next user-to-kernel entry on that CPU,
  made by whichever thread runs there, pushes its frame onto this thread's stack.
- Dispatch already sets RSP0 for the thread it switches in (`per_cpu::update_tss_rsp0` at
  `interrupts/context_switch.rs:1037`, `:1277` and `:1354`).

So E2.10a deletes both syscall-return TSS writes, in its Tier-1 commit, before the first class converts.
If its code reading finds a path where only the syscall return sets RSP0, that write moves after the
exit `cli`, where nothing can preempt it. The same audit covers `irq_log::flush_local_try()` on that
return path (`handler.rs:624`). <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 4.6 What stays masked

These sections keep interrupts masked, each bounded by code that does no waiting:

- exception and syscall frame save and restore;
- the exit path from the reschedule check to `iretq`, `sysret` or `eret`;
- `IrqSpinLock` sections;
- the scheduler's switch transaction;
- the idle halt or WFI with its masked recheck (B1 requirement 11);
- the early-boot polled loop.

`softirq` processing on the last preemption release (`per_cpu.rs:1098`) keeps its current behaviour.
Brief B3 redesigns it in E9.

## 5. Restartable sleeping faults and the address space

### 5.1 The address-space object

This is the R12 part that E8 does not cover. E2.8 moves each process's page-table root, VMAs, `brk` and
mmap bookkeeping, and its file-mapping bindings out of the `Process` row (`kernel/src/process/process.rs`,
fields `page_table`, `vmas`, `heap_*`, `mmap_*`) into `Arc<AddressSpace>`.

- **Ownership.** CLONE_VM siblings share one `AddressSpace`. Today they reach it through `inherited_cr3`
  and a scan for the owner row.
- **Direct reference.** Each thread holds an `Arc<AddressSpace>`. It is set before the thread first runs
  (fork, clone, spawn) and replaced only by that thread's own exec commit, so the thread reads it without
  a lock. The `Process` row keeps a reference for teardown and /proc.
- **The mapping lock.** Each `AddressSpace` has a `SleepRwLock` over its VMA set and mapping metadata.
  Faults and /proc readers take it shared. mmap, munmap, mprotect, brk, the exec teardown and the fork
  copy take it exclusive.
- **The page-table lock.** Each `AddressSpace` also has a `SpinLock` over installing and changing its page
  table entries, so shared-mode faults on different pages proceed in parallel.
- **Lock order:** filesystem locks, then the mapping lock, then the page-table lock, then the VNode table,
  then `MapState`, then the frame ledger and allocator. PROCESS_MANAGER is never acquired while the mapping
  lock is held, and never held while acquiring it, because a sleeping lock cannot be taken under a spin
  lock. Exec builds the new address space outside PROCESS_MANAGER, swaps pointers under it and tears the
  old one down after releasing it. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Fork.** The fork copy takes the parent's mapping lock exclusive. From E2.9, a parent thread that
  faults during the copy sleeps until the copy ends, where today it faults again and again
  (`kernel/src/memory/fork_copy.rs`, `anon_map.rs:34`).
- **No sleeping in E2.8.** E2.8 changes which locks a fault takes, not whether it may sleep. Its faults
  take the mapping lock shared with a try-acquire and the page-table lock as a spin lock. On contention
  they keep today's behaviour, a bounded retry and then a refault. E2.9 replaces that with a park, once
  the exception paths can resume a thread that slept (§5.2).
- **Signal frames.** Delivery stops writing frames under PROCESS_MANAGER. The PROCESS_MANAGER section
  chooses the signal and records the disposition in the thread. Today the frame is written from two
  places: the syscall return and the return from an interrupt or exception taken in user mode
  (`kernel/src/interrupts/context_switch.rs:318-323`; `kernel/src/arch_impl/aarch64/context_switch.rs:5415-5422`).
  The second runs in the interrupt's atomic context, and moving the write out of PROCESS_MANAGER does
  not change that. So the change is staged:
  - in E2.8, both places write the frame outside PROCESS_MANAGER, in the context each runs in today,
    with the nofault copy. A copy that would need to wait keeps today's failure handling;
  - in E2.9, the interrupt- and exception-return paths stop writing frames. When one of them finds work
    owed to the thread before it reaches user mode (a signal frame today), it sets the thread's
    `user_work` flag and, after the interrupt's own work and its EOI, runs a task-context continuation
    on the thread's own kernel stack: depth at the thread's baseline, interrupts enabled. The
    continuation writes the frame, may fault, sleep and restart, and then takes the normal masked exit,
    which rechecks `user_work`. The syscall return reaches the same continuation, so only one path
    writes frames.

E8 keeps everything else in R12 and R25: the VMA tree, mremap, real vfork and posix_spawn, sharded PID
lookup, and fd tables off PROCESS_MANAGER. E8 starts from this object and does not need to create it.

### 5.2 The fault protocol

Both #PF on x86 and a data or instruction abort on Arm are fault-class exceptions. The saved instruction
pointer (RIP on x86 per the SDM's exception classes and Interrupt 14; ELR_EL1 on Arm per the Arm ARM's
exception entry) names the faulting instruction. Returning from the handler without changing it
re-executes the access. E2 uses that for restart:

```
fault(addr, access):
  re-enable interrupts if the interrupted context had them enabled   (x86 gate cleared IF; Arm masked DAIF)
  as = current thread's AddressSpace                                (no PROCESS_MANAGER, no root scan)
  ctx = park::context()
  take as.mapping shared: Sleepable -> park on contention; Atomic -> try once, else take the nofault exit
  look up the VMA, check access, find or allocate the page:
    resolved          -> take as.pt; recheck that the PTE is still the one the lookup saw;
                         install, or leave the other fault's entry; return (the access re-executes)
    must wait for X   -> take a reference to X and read X.seq, both under as.mapping;
                         drop as.mapping;
                         end = park::wait(X.queue, || X.seq != seen, None, Killable)
    permanent failure -> SIGSEGV or SIGBUS, or the uaccess fixup and EFAULT
  after a park, by its end:
    Condition, Woken  -> return; the access re-executes and the fault starts again from the top
    Killed            -> install nothing; a user-mode fault returns to the kill at the exit, and a
                         kernel user copy takes the uaccess fixup, so its syscall unwinds with EFAULT
    NoMemory          -> the waiter node could not be reserved: a permanent failure, as above
    NotSleepable      -> cannot follow a Sleepable check; receipt, then the nofault exit
```

- **Owned wait target.** X's reference and its sequence number are taken while the mapping lock is still
  held, so X cannot be freed and its progress cannot be missed. `park::wait` checks `X.seq != seen`
  under X's queue lock (§3.3 step 2), so a fill that completes, an exclusive holder that leaves, or an
  unmap that happens between dropping `as.mapping` and registering is seen and ends the wait at once
  as `Condition`. Each target bumps its `seq` whenever it changes: a fill completing or failing, the
  exclusive mapping lock being released, an allocation retry being worth making.
- **Memory waits.** For frame allocation, X is the E1 reclaimer's progress sequence. When E1 reports that
  reclaim cannot make progress for this request, the fault fails as permanent (SIGBUS, or EFAULT for a
  kernel copy) instead of waiting again. A request that can never be satisfied therefore ends, rather
  than looping. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

- **Wait targets.** X is one of:
  - an exclusive holder of the mapping lock (munmap, mprotect, fork copy, exec teardown);
  - a page in a filling state (E3 fills pages from the page cache; in E2 the hook is exercised by the
    mapping cache's in-flight pin from #3232);
  - frame allocation waiting for memory (E1 returns allocation failure as an error, and its reclaim
    wakes).
- **Revalidation.** A fault that slept carries no lookup result across the sleep, and it drops its
  reference to X when the wait ends. The re-executed access faults afresh and repeats the whole lookup under `as.mapping` shared:
  - the VMA that covers the address;
  - the access against the VMA's current protection;
  - the mapping's identity: the VMA's backing object and its generation, so a VMA replaced by a new
    mapping at the same address is not taken for the old one;
  - the PTE, read again under `as.pt` immediately before the install.
  An unmap, protection change, remap or exec in between is therefore seen. A counter of retries per
  fault feeds the receipts, so a livelock shows up by name. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Atomic contexts.** A kernel access to user memory from an atomic context must use the nofault copy,
  which turns any fault that would need to wait into EFAULT (`write_user_bytes_nofault` today). It never
  parks. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **The ARM64 synchronous-exception return.** Today's epilogue cannot resume a fault that unmasked
  interrupts or slept:
  - entry stores the interrupted kernel SP (`sp + 272`) in the per-CPU `user_rsp_scratch` slot
    (`kernel/src/arch_impl/aarch64/boot.S:564-579`), and an EL1 return reloads SP from that slot
    (`:668-672`);
  - an IRQ nested after the fault unmasks writes the same slot (`:736-750`), and a fault that slept may
    resume on another CPU, whose slot belongs to someone else. Either way the outer return lands on the
    wrong kernel stack;
  - the epilogue's single return sequence clears the saved SPSR's I and F bits (`:628-631`). A user-copy fault
    taken inside an interrupts-masked kernel section would come back with interrupts enabled.

  So E2.9 changes the epilogue before any fault unmasks. The return SP for an EL1 return is kept in the
  exception frame, which lives on the thread's own stack, and is reloaded from there. The per-CPU slot
  stays only for the redirect-to-idle path, which writes it on purpose. The saved SPSR is restored as
  saved. Interrupts are unmasked only where the code writes a new SPSR on purpose (the refusal and idle
  redirects at `:605-626`, which store `0x5`) and on EL0 returns, where user mode runs unmasked. This is a change to
  `boot.S`, in a commit of its own. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **What goes away:** `FaultManagerUnavailable::Busy` returning `Resolved` (`interrupts.rs:980`), the
  `HeldHere` direct paths, and `find_process_by_cr3*` on the fault path.

E3's lazy mmap, readahead and fault-around build on this protocol. E2 makes faults *able* to sleep. It
does not make mmap lazy, so the 157,671x `memory.driver-mmap` ratio (§10) stays until E3.

## 6. Block I/O: one caller rule (DEV-24)

- **The rule.** Block I/O requires `ParkContext::Sleepable` or the early-boot token. `begin_port_io` takes
  an `IoContext` argument, obtained from `park::require_sleepable()` or from `EarlyBootIo`. A busy port
  always parks. `AhciReadError::CannotPark`, the ARM64 `yield` loop, the level arithmetic and the
  `EAGAIN` mapping in `vfs.rs:4945` (and its host-test expectations in
  `tools/breenfs-host-tests/src/metadata_failure.rs:109` and
  `tools/vfs-host-tests/src/trait_adapter.rs:53`) all go. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Atomic callers.** An atomic caller is a contract violation. It gets a receipt and
  `BlockError::NotSleepable`, which surfaces as EIO, never as success. A caller that truly runs atomically
  defers its I/O to a worker. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **The early-boot path.** `kernel_main` mints the `EarlyBootIo` token once, before the scheduler first
  dispatches, and revokes it at that point. While it holds the token, the boot thread owns the controller
  exclusively: it asserts the port has no owner, issues one command at a time, polls the port's
  command-issue and interrupt-status registers (AHCI 1.3.1, PxCI and PxIS) against a deadline, and returns
  `Timeout` when the deadline passes. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Boot order.** The boot thread reads no storage after the scheduler first dispatches. On ARM64 that
  takes three moves, which E2.4 makes together: <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - the production init path moves into a sleepable `init-loader` kthread, as #3228 did for x86. It
    takes the pre-loaded image if the early-boot read got one. Otherwise it does the late
    `read_init_from_ext2` and the `init_shell` fallback itself (§1.2), and it starts PID 1 through the
    scheduler. `spawn_as_current` from the boot continuation goes. The boot continuation drops its
    level and becomes CPU 0's idle loop, as the testing path already does (`main_aarch64.rs:2005-2025`);
  - the testing `test-loader` keeps its pin's purpose under a `park::Braked` guard (§3.2). E2.11 deletes
    baselines, so it also deletes this guard. The loader then creates its test processes unqueued and
    enqueues them once its reads end, which keeps them off its CPU without a level;
  - the pre-timer read of `/sbin/init` in `kernel_main` (`main_aarch64.rs:1405-1420`) stays. It runs
    before the first dispatch, so it is early-boot I/O and uses the token.

  This may also bear on the ARM64 `-smp 1` boot wedge (#617, #620), because CPU 0 stops being held
  through init. That PR's code reading has to settle whether it does. This plan makes no claim. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Completion readiness before revocation.** A sleepable caller can park only if something will
  complete its command. So the token is revoked at the first dispatch, and before that point E2.4 puts
  two things in place on both architectures: the timer is running (the ARM64 predicate checks this
  today, `ahci/mod.rs:428`), and each attached block device's completion source is armed, meaning its
  interrupt is routed and unmasked or its poller kthread exists. E2.4 reads the boot order on each
  architecture and moves whatever is late ahead of the first dispatch. At revocation it asserts
  readiness for each attached device. A device that is not ready gets a receipt naming it, and its
  requests return an error until it is ready, rather than polling. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Completion mode.** The mode collapses to one question: does the device raise a usable interrupt? If
  it does, the waiter parks on its `Completion`. If it does not (VMware ARM64 with no GICv2m), the CPU's
  poller kthread completes the command (`mod.rs:1382-1400`) and the waiter still parks. The caller's
  context no longer picks the mode. `Completion`'s own context-selected branches (`completion.rs:150-260`)
  reduce to park, plus the early-boot poll.
- **Other drivers.** virtio-blk (PCI and MMIO), NVMe and the virtio-sound and GPU waits move to the same
  contract in that PR. E4 later replaces slot-0 queueing with request-owned tags. E2 changes only who may
  call and how they wait.

Risk retired: the DEV-24 class. A disk read can no longer be refused because of who is asking, and
#3209's watchdog sensitivity to that refusal goes with it.

## 7. BreenFS: no device wait under the mount lock (I7; coordinated with E5)

### 7.1 Why the format allows it

BreenFS is copy-on-write:

- a node written at generation G never changes in place (Rodeh 2008, which `docs/fs/breenfs-design.md`
  already cites); <!-- claim-lint:ok: format property stated in docs/fs/breenfs-format.md. -->
- its blocks return to free space no earlier than G+2 (the PENDING quarantine,
  `docs/fs/breenfs-format.md`);
- since #3245, the decoded-node cache is keyed by (address, generation).

The quarantine alone does not make a lock-free read safe. The format's reuse rule
(`docs/fs/breenfs-format.md` §2.5, lines 139-142) protects the last two durable commits, and
`promote_pending` applies it by generation arithmetic alone (`kernel/src/fs/breenfs/space.rs:235-258`). A
reader that drops the lock while reading generation P, and is still reading when commits P+1, P+2 and
P+3 have published, can be reading a block that P+3 converted to FREE and reallocated. Neither a pinned
root in memory nor a later CRC check prevents that: the CRC is checked after the device has already
returned the new owner's bytes. So E2.5 makes the pin govern reuse:

- **A pin table per device.** `DeviceMounts` (`vfs.rs:208-218`), which every mount of the device shares,
  read-only mounts included, gets a table of pinned generations and their counts. A reader registers its
  pin under its mount's state lock, at the mount's current durable generation, before it releases the lock. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Promotion honours the oldest pin.** A block released by commit R is reachable from generation R-1
  and older. So a PENDING extent with `released_generation` R becomes FREE only when both R ≤ T−2 (the
  format's rule) and R ≤ the device's oldest pinned generation. `promote_pending` takes that bound as an
  argument, and so does any other path that converts PENDING space. The format does not change. Pins
  delay reuse, and they do not reorder it.
- **The race with a commit in preparation.** Preparation runs without the mount lock (`vfs.rs:2780-2785`)
  and may allocate while a reader is registering. That is safe: a reader pins the durable generation D,
  and the generation in preparation is D+1, which frees only R ≤ D−1, blocks D does not reach. Every
  later preparation starts after D+1 publishes, and it reads the pin table when it computes its bound.
  The publication hold in §7.2 step 4 is therefore a progress rule, not a safety rule. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

So a node or data extent read without the mount lock stays valid for as long as the reader's generation
is pinned. Only the open generation's in-memory state (dirty pages, the replay log and the inode edits)
needs the lock. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 7.2 Read and lookup operations

A foreground operation runs in passes (an optimistic read with validation: Kung and Robinson 1981):

1. Under the lock, the operation runs in *no-wait* mode. Each cache access that would need the device
   records the request (block address, generation, kind) and makes the pass return `NeedsIo` instead of
   waiting. A pass that needs nothing completes as today. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
2. The operation pins the current committed generation and releases the lock. It submits the recorded
   reads into the buffer cache, verifies them by CRC, address and generation as today, pins the cache
   entries for itself, and parks on their completion.
3. It takes the lock again and repeats the pass from the current root. A pass that sees the same
   generation now hits the pinned entries. A pass that sees a newer generation may need new nodes and
   repeats steps 2 and 3.
4. Progress: after a bounded number of passes, the operation takes a publication hold. The commit worker
   waits for the holder's next pass before it publishes, so a stream of commits cannot starve a reader.
   The limit is a constant in the PR, chosen by the host test's interleavings.
5. Output: a pass exposes nothing to its caller until it completes. Today `read_directory` calls its
   sink for each entry as the walk reaches it (`vfs.rs:4645-4662`), and `getdents64` appends each record
   to its reply and advances its cookie in that callback (`kernel/src/syscall/fs.rs:1171-1199`). A pass
   that emitted entries and then hit a cold node would, on restart from the root, emit them again,
   possibly from a different generation. So a pass collects its results (directory entries with their
   cookies, a stat result, a lookup's inode) in a per-pass buffer. A pass that ends in `NeedsIo` throws
   its buffer away. Only the pass that completes hands its buffer to the caller's sink, in order, so one
   reply comes from one generation and has no duplicates. The buffer is bounded by the caller's request:
   for `getdents64`, the record bytes that fit the user buffer. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 7.3 Write paths and the commit worker

- **Writes** stage dirty data in memory under the lock, as today.
- **A full dirty budget.** Today the writer commits inline (audit R11). Instead, a foreground writer
  releases the lock, signals the commit worker and parks on commit progress, then retries. It never waits
  on a FLUSH while holding the lock. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **The commit worker never waits for itself.** `admit` also runs on the commit worker: `commit_due`
  stages mapped dirty pages, and their writeback goes through `admit` (`vfs.rs:2508-2569`, `:4320-4347`).
  Today's inline commit in `admit` exists so that the worker does not sleep for progress only it can make
  (`vfs.rs:2188-2196`). A worker that parked on commit progress there would stop commits, and fsync would
  wait for good. So admission on a full budget branches on who is asking: <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - a foreground writer parks, as above;
  - the commit worker does not park. If no other publisher owns the detached generation, it publishes
    the generation it already holds, as today, with the mount state lock released across the writes and
    FLUSH. If another publisher does own it, the worker stops staging mappings for this pass, records
    the remaining mappings as still dirty and returns to its loop, where that publisher's completion wakes
    it.
  Admission learns who is asking from the worker's own thread identity, recorded when the worker starts,
  and not from a flag a caller passes. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **The commit worker** freezes the generation under the lock and writes and flushes without it. It takes
  the lock again only to publish. E2.5 takes an inventory of every device wait currently reached under the
  lock and lists what each becomes in that PR's body, checked by the guard in §9. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Contenders.** They always park, because every caller is sleepable (§3). The spin at
  `mount_lock.rs:136-138` goes, and so does the `EAGAIN` path. `preempt.rs`'s cooperative points stay
  until E2.10b makes the BreenFS syscalls preemptible, and then go. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 7.3a Device waits that are not read passes

The passes in §7.2 cover lookups and reads. Several other paths wait on the device under a
`MountLock` today. Two of those locks are device-wide gates in `DeviceMounts` (`vfs.rs:208-218`), so the
unpublished-mount exception in I7 cannot cover them: a published mount takes them too. E2.5 moves each
path as follows.

| Path today | Device wait under | Under E2.5 |
|---|---|---|
| A cold partial-page write or truncate: `edit_pages` reads the old page through `files.read_verified` (`vfs.rs:3303-3339`) | the mount state lock | Two phases. Under the lock, a no-wait pass finds the partly covered pages that are cold and returns `NeedsIo` with their (inode, index, generation). The writer pins, releases, reads those pages into a staging buffer and verifies them, then retakes the lock. If the file's extent identity and generation are unchanged it applies the edit from the staged pages, and otherwise it repeats the pass |
| Scrub (`scrub_pass`, `vfs.rs:3160-3177`) | the mount state lock | Under the lock it copies `space_root` and the generation and pins it. It walks that generation's nodes with no lock held, then retakes the lock only to record its progress |
| The durable-root check (`check_durable_roots`, `vfs.rs:2019-2032`) and snapshot status | the mount state lock | The same: pin the durable generation under the lock, release, read, and validate against the pin |
| The commit worker's writes and FLUSH (`vfs.rs:2886-2917`) | `DeviceMounts::publication`, a `MountLock` | `publication` stops being a `MountLock`. It becomes a `PublishToken` that serialises the publishers of one device's mounts, and only commit workers take it. Foreground operations no longer take it |
| A read-only snapshot read, which holds `publication` through its read so that reused SPACE nodes cannot change under it | `publication` | It pins its generation in the device's pin table (§7.1) instead. Pinned blocks are not reused, so it no longer needs to exclude publication |
| Snapshot deletion, which holds `selection` across its sync (`vfs.rs:3384-3398`) | `DeviceMounts::selection`, a `MountLock` | Under the state lock, deletion marks the root `Deleting`. A constructor that would select a `Deleting` root fails with `EBUSY`, as it would during today's hold. Deletion releases the lock, syncs, then clears the marker under the lock. `selection` becomes that marker and is held across no I/O |

I7's exception list then names exactly two things: a mount not yet published, and the `PublishToken`,
which is not a `MountLock` and which the I7 guard checks no foreground thread ever takes. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 7.4 The boundary with E5

E2.5 is the first step of E5 ("release the mount lock before I/O") and carries the I7 guard. E5 then adds
per-inode locks, per-node tree locks, N+1 copy-on-write over N with no replay, and the dentry cache, and
keeps I7 throughout. The PR belongs to E2's sequence. If E5's lane starts first, the coordinator decides
which lane carries it (§12, Q3). It must not land twice.

ext2's lock (#728) moves onto the park contract in E2.2. It keeps holding its lock across device reads,
because the ruling names only BreenFS. Since #3250 the QEMU rig's disk rows run on a BreenFS root
(`userland/bbench/RIG.md:139-146`), so ext2's lock no longer bears on them (§10).

## 8. PR sequence

Each E2 PR follows the same rules:

- its check is its own first commit and is red on the code before it;
- a model reaches production code. A standalone model of the intended design can show that the design
  is consistent, but it cannot be red on the old tree, because it does not run the old code. So each check
  that claims a red result drives the production function through a seam: the decision under test is
  factored out of its caller into a function the production path calls, the check commit introduces
  that seam with no behaviour change, and the check then fails against today's logic with an observed
  wrong result. Where the effect lives in an exception return or a real switch, the check is a kernel
  test in the testing feature driven by a fixed point instead. A model may still enumerate
  interleavings on top of the seam. A check that cannot be red on the old tree, because it guards a
  mechanism the PR adds, says so;
- it runs its tests once, at its final head;
- it gets the other model family's review, including a clean-room provenance check (DEV-34(5));
- it runs claim-lint;
- it adds no warnings on x86-64 or ARM64.

There are no effort-suite runs until the program's final run (DEV-30). The coordinator records a QEMU bbench
series before and after each PR that §10 says could move a row (DEV-31(3)).

| PR | Work | Needs | Tier-1/2 files |
|---|---|---|---|
| E2.1 | Clean-room event queue from B1 | — | — |
| E2.2 | Park contract: `park::context`, baselines, `park::wait`, receipts; the eight predicates in §1.2 converted | E2.1 | — |
| E2.3 | Parking and parked: preemption does not commit a sleep; non-preemptible halts become violations | E2.2 | `interrupts/context_switch.rs` (Tier 2) |
| E2.4 | Block I/O caller rule: CannotPark removed, early-boot token, ARM64 init load in a kthread, completion modes collapsed | E2.2 | — |
| E2.5 | BreenFS: no device wait under `MountLock` (shared with E5) | E2.2 | — |
| E2.6 | `sync::` lock types and the validator (6a); raw spin locks migrated by subsystem (6b onward) | E2.2 | `per_cpu.rs` (Tier 2) only if the guard needs it |
| E2.7 | x86 syscalls with interrupts enabled, under the brake | E2.6 (x86 ISR inventory) | `syscall/handler.rs` (Tier 1) |
| E2.8 | `AddressSpace` object, mapping lock and page-table lock; faults and the mmap family off PROCESS_MANAGER; signal frames out of PROCESS_MANAGER | E2.2, E2.6 | `interrupts.rs` (Tier 2) |
| E2.9 | Restartable sleeping faults | E2.8 | `interrupts.rs` (Tier 2) |
| E2.10a-f | Preemptible syscalls by class (§4.4), with the `CpuPin` audit and the custody park | E2.6; x86 also E2.7; 10d needs E2.9 | `syscall/handler.rs` (Tier 1), once in 10a: the class table and the return-path TSS write |
| E2.11 | Retire the brake, the `Braked` class and the baselines | E2.10a-f; an empty `Braked` set | `syscall/handler.rs` (Tier 1) |

E2.3, E2.4 and E2.5 are independent of each other after E2.2, and E2.6 can run beside them. One lane
should own each, not several agents on one PR (DEV-30(3)).

### E2.1: clean-room event queue (B1)

- **Scope.** Replace `WaitQueueHead` and its free functions, and the two scheduler hooks B1 names, with a
  new implementation written from B1 alone. Update callers in the same change. Semantics are unchanged: B1
  requirements 1-11, including today's preemption contract (requirement 9) and the x86 halting waiter
  (requirement 11). The one addition is the owed-wake counter for a failed IRQ try-lock (§3.3). Later
  PRs change the rest.
- **Who writes it.** A lane that reads B1, the specifications B1 names, the public signatures and the
  callers. It does not read the old bodies or the documents §0 excludes.
- **Tests, check-first.** These are B1's list. The model drives a production queue, not a copy of the
  design. Its first commit puts the caller-facing operations from B1's table behind one trait that both
  today's `WaitQueueHead` and the new queue implement, with the scheduler hooks replaced by a recording
  test scheduler under `cfg(test)`. The same cases then run against both. For the semantics E2.1
  preserves, they are characterization checks: green on today's queue, which shows that the harness
  reaches production code, and green on the new one. The owed-wake case is the red one:
  - a host model that enumerates the interleavings of the waiter's register, check, sleep and finish
    steps against the waker's set-condition and wake steps, asserting requirement 1;
  - the same enumeration for timeout against wake, signal against wake, wake-one with two waiters, and a
    waker in interrupt context;
  - out of memory at reservation;
  - the IRQ try-lock under a held lock: the queue's links are unchanged, and the waiter is woken when
    the holder unlocks. The second half is red on today's queue, whose try-lock returns 0 and leaves
    no record of the wake;
  - a deferred wake from a context that holds a lock the scheduler path also takes: it completes
    without entering the scheduler lock, and the waiter is woken by its CPU's inbox drain;
  - requirement 6 after each way a wait can end;
  - the existing queue unit tests, reworded to the new names;
  - the structure tests B1 lists, unchanged.
- **Risk retired.** Assessment A1's provenance. The primitive's invariants become written down and checked
  against a model.
- **Rows.** No row is predicted to move. A before-and-after series shows it is neutral on
  `process.fork-exec`, `ssh.round-trip`, `network.tcp-*` and `disk.*`.
- **Boot.** One ARM64 QEMU boot with the standard boot test. The host model is sequentially consistent, and
  where the barriers sit on a weakly ordered CPU is a publication-order effect it cannot settle.

### E2.2: the park contract

- **Scope.** §3.1-§3.4: `park::context`, `park_baseline` set at syscall entry and in kthreads,
  `park::wait` and `park::wait_any` on the E2.1 queue, `park::announce` and `park::should_wake`, and the
  violation receipts. The eight predicates in §1.2 are converted to `park::context()`.
- **Behaviour change.** Only one: a kthread holding a raw spin lock (depth 1, baseline 0), which today parks
  while holding the lock, now gets a receipt and an error. Publication keeps today's protocol: a
  baseline-0 waiter's `Parking` state is published at depth 1, raised inside `park::wait`, until E2.3
  (§3.2).
- **Tests:**
  - a host table test of `context()` across its input space;
  - a host model of the baseline: each `WaitEnd` returns with the caller's entry depth (B1 requirement 9);
  - a host model of the commit transaction, for `wait` and for `wait_any` over two queues. It places a
    wake, a signal of each mode, a kill and a passed deadline at each point from step 2 to after the
    switch, including between step 2 and step 3. It asserts that each wait ends once with the first
    arrival's reason, that no node tagged with its `seq` stays linked, that a stale-`seq` wake or
    deadline does nothing, and that entry depth and mask are restored on every end; <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - a host model of publication: from the moment a baseline-0 waiter publishes `Parking` until its
    switch, no interrupt exit in the model reads depth 0;
  - a structure test that no file outside `kernel/src/task/park*` compares `preempt_count()` with 1 or 0
    to decide whether to sleep, or adds a level for a wait. On today's tree it finds the sites in §1.2.
- **Risk retired.** "Whether code may sleep is an unstated property of its caller" (R4). The
  level-arithmetic hazard (the #672 underflow guard's class).
- **Rows.** No row is predicted to move. **Boot.** Not needed: the predicate is pure, and the host tests settle it.

#### E2.2 as built: deviations (ruling DEV-39(2))

The builder resolved these details against the code. Each has one sentence of reason.

- **Where the wait word lives.** Each `Thread` holds its wait word (`park::ParkState`), because ARM64 threads
  have no wake record until the wake index is published and so cannot hold it there.
- **How wakes carry the sequence.** A queue waker ends the word's current wait while it holds the queue
  lock with the waiter linked, and the timer heap ends a live I/O deadline as `Timeout`. The R17 inbox
  carries no sequence, so a late inbox wake makes the thread runnable with its word unchanged, and
  `park::wait` publishes again. This keeps the inbox's interface as it is. Signals and kills are seen at
  the waiter's own checks, which §3.3 already allows.
- **Where the baseline is set.** `KillCustody::enter_syscall` sets it and `resume_user_time` clears it.
  Both syscall entries call the first, and both returns to user mode call the second, so the Tier-1
  `syscall/handler.rs` is not edited.
- **"Dispatching".** Until E2.4's early-boot token, this is a current thread and, on ARM64, the timer
  running, the conditions the converted predicates tested.
- **More `park::Braked` sites.** The BreenFS commit and scrub workers, ext2 writeback and the input
  release service hold a level across their waits the way a syscall does, as the test-loader does. They
  take the guard in E2.2, and so does the test-loader (the design had it in E2.4). Otherwise the converted
  predicates would refuse them. The test-loader's boot-tests step that lets its CPU dispatch uses
  `park::without_braked`.
- **The census is wider than §1.2's table.** `tests/park_contract_structure.rs` also found the serial
  console writer's predicate, the commit signal and block cache waits' added level, and the input
  service's level. E2.2 converts those. It classifies the other drivers' predicates (virtio-blk,
  virtio-sound, NVMe, GPU) as E2.4's (§6). It classifies the syscall waits that release the brake by hand
  (futex, signals, sleep, wait4, poll and select, sockets, pipes and FIFOs) as E2.10's, by class, and the
  custody loop as E2.10a's.
- **Sleeps not yet on `park::wait`.** `schedule_current_wait`, `Completion`, the syscall prepared waits
  and the retry pause release one level, if one is held, through `park::release_for_sleep`, as they did
  before. Where they chose a sleep path by depth, they now ask `park::sleepable_braked()`: sleepable at a
  baseline of exactly 1, because that path releases exactly one level. So `Completion` still polls for a
  depth-0 kernel thread until E2.4 collapses its modes.
- **The boot continuation.** On ARM64 it runs as CPU 0's idle thread and reaches the BreenFS mount lock,
  the commit signal and the block cache when it resolves init's path. Those waits keep their pre-E2.2
  sleep for it, with a receipt, through `park::wait_allowing_boot_idle`, until E2.4 moves its storage
  reads into `init-loader`.
- **Refusals in callers with no error channel.** MountLock spins as before, the block cache wait returns
  to its caller's recheck, the commit signal returns EINTR, and AHCI keeps `CannotPark` on x86 and its
  polling on ARM64 (E2.4 removes both). The "nested preemption guards" assertions in the block cache and
  commit signal waits are replaced by the refusal.
- **Smaller details.** A depth below the baseline is `LocksHeld(0)`, because the design names no variant
  for it. `wait_any` takes at most 8 queues (`WAIT_ANY_MAX`), so the wait path does not allocate. AHCI's
  recovery deferral, written for a masked caller, admits `IrqsMasked` as well as `Sleepable`. On ARM64 a
  `park::wait` keeps the wait-site PC its publication recorded, because the generic path cannot pass its
  caller's return address.

### E2.3: parking and parked

- **Scope.** §3.5: the `Parking` state, requeueing on an involuntary switch, and committing only in
  `park::wait`. The non-preemptible branches of `arch_halt_while_blocked` become violations. The
  "yield before parking" workarounds go, because a committed park switches away. The level that
  `park::wait` raises around publication in E2.2 (§3.2) goes in this PR, in the same commit as the
  requeue, so no tree has one without the other.
- **Tests:**
  - a host scheduler-state check through a seam: the first commit factors the interrupt-exit decision
    on ARM64 (`arch_impl/aarch64/context_switch.rs:5405-5420`) and x86 (`can_schedule`,
    `interrupts/context_switch.rs:243-281`) into one function of (thread state, depth, voluntary or not)
    that both exits call, with no behaviour change. The check calls it. Publish, then an involuntary
    switch at depth 0, leaves the thread runnable: red on today's ARM64 decision, which switches the
    published thread out as blocked. Publish, wake, commit ends `Woken` with no sleep. Publish, commit,
    wake ends `Woken` once;
  - a kernel test in the testing feature, driven by a fixed point: a kthread at baseline 0 publishes
    `Parking` at depth 0, through a test-only entry that skips E2.2's publication level, the test forces an interrupt exit with `need_resched` set at a named point before the
    commit, and checks that the thread runs again without a wake. This is the real exception return,
    which the seam cannot reach;
  - a structure test that no task-context wait halts as current outside idle, the early-boot path and the
    pinned-idle case.
- **Risk retired.** The stranded lock holder of #1163 and #1155. x86 `can_schedule`'s BlockedOnIO gap
  (#666, #508).
- **Rows.** `process.fork-exec` is the candidate, because the parent's wait4 sleeps and wakes. It is 11.58x
  Linux today, and most of that is fork and exec work, so the prediction is a small move. The series
  decides.
- **Boot.** One ARM64 QEMU boot: the interrupt-exit switch path is the hardware exception return.

#### E2.3 as built: deviations (ruling DEV-39(2))

The builder resolved these details against the code. Each has one sentence of reason.

- **The seam's inputs.** `park_core::exit_switch` decides from the thread's state, its wait word's phase and
  whether a reschedule is pending. Each exit calls it once its own depth gate has admitted a switch, and
  those gates stay where they are, because ARM64's runs before the deferred-requeue drain and the scheduler
  lock and x86's before its PREEMPT_ACTIVE check.
- **"Voluntary" is the word's phase.** A thread on `park::wait` switches by its own choice only once it has
  committed (`Parked`), so the phase carries what a voluntary flag would. A word that is `Idle` belongs to
  a wait not on `park::wait` and keeps the rule from before E2.3, and so does an ended word, whose
  scheduler wake follows its arrival.
- **How the requeue works.** The exit's new `ExitSwitch::Requeue` returns the thread to running, as ending
  a wait does (no live deadline, `blocked_in_syscall` clear), under the lock that owns it
  (`park::apply_exit_switch`). It keeps the queue entry and the word, so a wake still ends the wait.
  E2.2's commit is a compare-and-swap on the word, not the scheduler transaction §3.3 describes, so
  the commit cannot see the requeue. Instead the commit's switch finds the thread running and returns at
  once, and the wait publishes again and redoes its signal and deadline checks. A preempted wait pays one
  extra pass.
- **x86's `can_schedule` is unchanged.** It admits a thread published blocked on I/O only when a
  reschedule is pending, which is the runnable rule for an uncommitted wait. The gap its comment records
  (#666, #508) is closed by the mandatory check's decision and by the halt rule below. Admitting
  `BlockedOnIO` there would also have changed the switch of the x86 waits not on `park::wait`, which no
  ARM64 boot can show.
- **Violations without an error channel.** `arch_halt_while_blocked` returns `()`, and its callers loop
  on their own state. A caller that may not switch away gets a receipt (`park::halt_as_current`), naming
  the levels it holds, and keeps its halt. This is the same treatment E2.2 gave its refusals in such
  callers. E2.4 and E2.10 move those waits onto `park::wait`, where the refusal is an error.
- **Which "yield before parking" steps went.** The park's own x86 switch (`halt_blocked_current`) no
  longer sets a reschedule first. The 8 waits the new HALT census found also drop theirs: each published
  a blocked state and halted as the current thread itself, and each now sleeps through
  `arch_halt_while_blocked`. The 17 `yield_current(); arch_halt_while_blocked()` pairs in syscall and
  driver waits not on `park::wait` stay. Those waits have no committed park yet, and their classes move
  in E2.4 and E2.10.
- **The kernel test's feature.** It is a `boot_tests` case, because the test registry is built under that
  feature.
- **E2.2's publication check.** Its host check that a baseline-0 publication reads depth 1 or more is
  replaced by the I4 check, because this PR removes that level.

### E2.4: block I/O caller rule

- **Scope.** §6.
- **Tests:**
  - the DEV-24 regression, through a seam. An ordinary sleepable caller at depth 0 or 1 already parks
    today (`can_park_for_port_io`, `ahci/mod.rs:427-436`), so a model of "a sleepable caller arrives
    while the commit worker owns the port" would be green on the old code and could not show the fix. The
    failing case was a specific context: PID 1's image load issued from the boot continuation at depth 2
    while `breenfs-commit` owned port 0 (`tid=Some(1) preempt_count=2 port_owner=21` in DEV-24's
    serial). The check's first commit factors admission's decision out of `begin_port_io` into a
    function of (current thread, idle or not, depth, interrupt context, timer running, port owner) that
    `begin_port_io` calls, with no behaviour change. The check then has three parts:
    - it feeds that function the recorded DEV-24 context and an owned port, and gets the old answer,
      `CannotPark` on x86 and the `yield` poll on ARM64. That shows the seam reaches the old path;
    - it runs the boot handoff the same way: each init-image load that E2.4 moves (§6 boot order) is
      issued from its new context, the `init-loader` kthread at baseline 0 and depth 0, against the same
      owned port. It must park and get the port after `end_port_io`. On the pre-change tree the ARM64
      post-SMP fallback reads still come from the boot continuation, CPU 0's idle context, where the
      predicate answers no and ARM64 polls instead of parking. So this part is red there;
    - the same function, given the DEV-24 context under the new rule, returns `NotSleepable` with a
      receipt, not a poll or a silent retry;
  - an atomic caller gets `NotSleepable`, a receipt and no data;
  - the early-boot token is refused after revocation, and polled I/O with the token while the port has an
    owner is refused;
  - the updated expectations in `tools/breenfs-host-tests` and `tools/vfs-host-tests`;
  - a structure test that `CannotPark`, the admission `yield` loop and the level arithmetic are gone;
  - a `park::context` table case for the testing loader: one level held at baseline 0 is
    `LocksHeld(1)`, and the same level held as `park::Braked` is `Sleepable`;
  - a structure test that `kernel_main` on ARM64 reaches no storage read and no `spawn_as_current`
    after the first dispatch, and that `run_test_loader` takes `park::Braked` and no bare
    `preempt_disable`;
  - the revocation assertion: a host test that revoking the token while an attached device's
    completion source is unarmed yields that device's receipt and an error for its requests, not a poll.
- **Risk retired.** DEV-24 and its family (#3209). Context-dependent I/O modes.
- **Rows.** The rig's disk is virtio-blk, so its rows cannot show the AHCI change. They can move
  through E2.4's virtio-blk change, where the completion mode collapses to a park. The before-and-after
  series reads the rig's disk rows (sequential, small-files, walk and fsync, on the BreenFS root since
  #3250) for that, and attributes any move to virtio-blk waits, not to AHCI. On the R240 the effect is
  boot and dev-loop reliability, read from the next kernel-only flash.
- **Boot.** Boot order is a hardware-timing effect the host tests cannot settle, and E2.4 changes it on
  two ARM64 paths. So it gets two ARM64 QEMU boots with the repo's standard scripts, one per path:
  - the testing boot (`test-loader` under `park::Braked`), which must load its whole roster;
  - the production-profile boot (`docker/qemu/run-aarch64-prod-profile-boot-test.sh`), where
    `init-loader` starts PID 1.

  x86's part reaches hardware at the coordinator's flash.

### E2.5: BreenFS lock order

- **Scope.** §7.1-§7.3.
- **Tests,** in `tools/breenfs-host-tests`:
  - **the I7 guard:** the test block device fails when a device request is submitted or waited on while
    any `MountLock` is held, counted per thread from the existing `Event::Acquire` and `Event::Release`
    hooks (`mount_lock.rs:107`, `:119`). It is red on the old code at the first node-cache miss
    under the lock;
  - **a fixed interleaving:** reader A misses a node and blocks in the test device on a barrier. Reader B's
    stat of a cached inode completes before A's device is released. Red on the old code;
  - **commit races:**
    - a commit publishes G+1 while A waits on a node of G, and A's retry returns results consistent with
      G+1, not a mix of G and G+1;
    - a publication hold after the bounded number of passes lets A finish while commits keep arriving;
    - **reuse past the horizon:** A pins generation P and blocks in the test device on a read of a
      block that P+1 releases. With the publication hold disabled, commits P+1 to P+4 publish and
      allocate. The released block stays PENDING, and A's read returns P's bytes. Red on today's
      `promote_pending`, which frees it at P+3. The same case with A on a read-only mount of the
      writer's device;
  - **the write path:** a full dirty budget makes the writer park on commit progress without holding the
    lock;
  - **a late miss in readdir:** a `getdents64`-shaped sink reads a directory whose first leaf is
    cached and whose second is cold, so the pass emits at least one entry before it needs the device.
    A commit that adds an entry lands while the read is out. The reply has no duplicate names or
    cookies, and every entry in it comes from one generation. Today's tree never restarts a walk, so
    this check passes there: it guards the new mechanism, not an old defect. In E2.5 it goes in after
    the commit that adds passes and before the commit that adds the per-pass buffer, and it is red
    between the two; <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - **the worker path:** a full budget reached while the commit worker stages mapped dirty pages. In
    one case the worker owns the detached generation, and in the other another publisher does. In both,
    a concurrent fsync completes and the worker is never parked on commit progress. A variant of the
    check against §7.3's earlier rule, where every full-budget caller parks, deadlocks in the model,
    which shows the case is reached; <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - **the §7.3a paths, each with a cold cache under the I7 guard:** a partial-page write, a truncate to
    the middle of a page, a scrub pass, the durable-root check, snapshot status, a read-only snapshot
    read and a snapshot delete. Each is red on the old code at its first device read, or at its
    `publication` or `selection` hold. The guard also fails if a thread outside the commit worker takes
    the `PublishToken`. The partial-write case adds a commit between the staging read and the retake,
    and checks that the edit is redone against the new generation.
- **Risk retired.** The BreenFS half of DEV-24 (R10). One reader's device wait stalls the whole mount,
  including commit publication.
- **Rows.** On the QEMU rig, the disk rows, which run on a BreenFS root since #3250 (§10), and the
  Breenix compile rows, whose cargo jobs share one mount. On the R240, `disk.stat`, `disk.create` and
  `disk.unlink` while the commit worker is active, and `compile.cargo-noop`. DEV-23's ban still keeps
  `disk.walk` and `disk.sequential-*` off the R240 until #3226 closes.
- **Boot.** Not needed. The host harness runs the production BreenFS code and settles lock order and the
  interleavings.

### E2.6: lock types and the validator

- **6a.** `sync::{SpinLock, IrqSpinLock, SleepLock, SleepRwLock}`, with `IrqSafeMutex` and `MountLock`
  becoming instances. The §4.2 validator. A structure guard bans `spin::Mutex` and `spin::RwLock` outside
  `kernel/src/sync/`, with an allowlist holding today's 126 files.
- **Before per-CPU init.** Some of these locks are taken before the per-CPU area exists. ARM64 runs
  `drivers::init()`, which sets up the VirtIO MMIO network queues behind `IrqSafeMutex`
  (`kernel/src/drivers/virtio/net_mmio.rs:433-474`), at `main_aarch64.rs:998`. `per_cpu_aarch64::init()`
  runs later, at `:1262`. Both architectures' `preempt_disable` assert that per-CPU init has happened
  (`per_cpu_aarch64.rs:728-743`, `per_cpu.rs:1026-1031`). So a guard that always raised the depth would
  break the ARM64 boot as soon as `IrqSafeMutex` became an `IrqSpinLock`. The rule: <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - before per-CPU init there is one CPU, no scheduler and nothing to preempt. A `SpinLock` or
    `IrqSpinLock` guard then masks interrupts as usual but leaves the depth alone; <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - each guard records whether it raised the depth, and its drop lowers the depth only if it did. A lock
    taken before per-CPU init and released after it therefore cannot underflow the count;
  - the validator records pre-init acquisitions under the class's name with no CPU attribution.
  This leaves today's boot order unchanged. E2.6a does not move driver init.
- **6b onward.** Migration by subsystem (drivers, fs, net and socket, ipc, memory, process, gpu). Each PR
  shrinks the allowlist and is safe alone, because a preempt-raising lock excludes what a raw one does and also keeps its holder on the CPU.
  The x86 ISR lock inventory (§4.3) lands with the driver migration.
- **Tests:**
  - host tests of the validator: an inverted pair is flagged; a class taken in IRQ and with interrupts on
    is flagged; a `SpinLock` across `park::wait` is flagged;
  - host tests that each guard raises and restores depth and mask in order;
  - host tests of the pre-init rule: a guard taken with per-CPU state absent masks and does not touch
    the depth; one taken before init and dropped after leaves the depth at its value from before the
    acquisition;
  - the structure guard, red on today's tree.
- **Risk retired.** A preempted holder with its contender spinning on the same CPU (#1394's class). Locks
  shared with ISRs without masking, which E2.7 needs gone first.
- **Rows.** No row is predicted to move.
- **Boot.** 6a and most of the migration PRs need none: the migration is mechanical, and the validator
  carries the runtime evidence into the final run. The drivers PR is the exception. It converts the locks
  that ARM64 takes before per-CPU init, and which locks those are is a property of the real boot order
  that the host tests do not see. So it gets one ARM64 QEMU boot with the standard boot test. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### E2.7: x86 syscalls with interrupts enabled

- **Scope.** §4.3. This is a Tier-1 edit to `syscall/handler.rs`: the interrupt enable after custody is
  open, in a commit of its own. The PR body gives the Tier-1 explanation CLAUDE.md requires. The fork
  unmask goes in a separate commit.
- **Tests:**
  - a structure test pinning the entry order (frame saved, custody, interrupts enabled) and that the exit
    still masks before the reschedule check (`entry.asm:117`);
  - the x86 ISR lock inventory as a structure test: each listed ISR module locks only `IrqSpinLock`s.
- **Risk retired.** The whole-syscall interrupt blackout on x86.
- **Rows.** No QEMU row, since ARM64 already works this way. On the R240: `ssh.round-trip`, `network.*`
  and `disk.*` under concurrent syscalls.
- **Boot.** The change is x86-only, and x86 QEMU is not used (DEV-31(2)). Its boot evidence is the coordinator's
  kernel-only flash, with a staging boot and an SSH check (§12, Q1).

### E2.8: the address space off PROCESS_MANAGER

- **Scope.** §5.1.
- **Tests:**
  - a host model of the address-space lifetime: a fault holding a reference across a concurrent munmap
    and an exec teardown frees frames only after the last reference;
  - a host model of the fork copy: a parent fault during the copy does not sleep and does not install
    an entry, and refaults until the copy ends (today's behaviour, now under the mapping lock);
  - a structure test that no E2.8 fault path calls `park::wait` or takes the mapping lock with a
    sleeping acquire;
  - a structure test that the fault paths in `interrupts.rs`, `arch_impl/aarch64/exception.rs`,
    `memory/anon_map.rs` and `memory/file_map.rs` do not name PROCESS_MANAGER, `poll_manager` or
    `find_process_by_cr3*`;
  - a structure test that signal-frame writes are not reached under PROCESS_MANAGER;
  - page-table updates under the new locks in `tools/pagetable-host-tests`.
- **Risk retired.** Faults on different CPUs serialising on a lock that dispatch also takes (R12). The
  linear root scan (T6). Signal frames written with interrupts masked under PROCESS_MANAGER.
- **Rows.**
  - `memory.anonymous-touch`, 2.57x today, should move most: each first touch currently scans for the
    process under PROCESS_MANAGER twice.
  - `memory.driver-touch` (3.87x).
  - `memory.malloc-4t` (602x) only in part: libc's owner word dominates it until Phase 1.3 and E7.
  - `process.fork-exec`.
- **Boot.** One ARM64 QEMU boot: the new fault path runs on real exception entry.

### E2.9: restartable sleeping faults

- **Scope.** §5.2, and the return-to-user continuation for signal frames (§5.1). The continuation lands
  in this PR, before or with the first fault that may park. The interrupt- and exception-return paths
  stop writing frames in the same commit.
- **Tests,** as host fixed interleavings, each red on the old code, which returns `Resolved` without
  sleeping. They run against production code through a seam. E2.8 has already put the fault's lookup
  behind the `AddressSpace` object. E2.9's first commit factors the resolution step into
  `resolve_fault(as, addr, access, ctx) -> FaultOutcome`, which both architectures' fault handlers call,
  with no behaviour change, and `tools/pagetable-host-tests` drives it with a test `AddressSpace`. On
  the pre-change tree that function answers a contended or waiting case with today's `Resolved` and an
  immediate refault, which each case below observes as its red result:
  - a fault sleeps behind a munmap holding the lock exclusive; after the wake the VMA is gone, the result
    is SIGSEGV, and no stale entry is installed;
  - a fault sleeps; mprotect changes the protection; the retried access gets the new protection;
  - a fault sleeps during a fork copy, and no entry is written into the copying address space before the
    copy completes;
  - SIGKILL during a fault's sleep: the thread exits, and no entry is installed. The same during a
    kernel user copy: the syscall returns EFAULT and unwinds;
  - completion before registration: a fill completes, or a munmap ends, after the fault drops
    `as.mapping` and before it registers. The wait ends as `Condition` without sleeping, and the retry
    sees the new state;
  - a failed park: waiter reservation fails, and the fault fails as permanent and does not re-execute;
  - an allocation that reclaim reports it cannot satisfy: the fault ends in SIGBUS after a bounded
    number of waits, and the retry counter shows the count;
  - a VMA replaced at the same address while the fault sleeps: the retry finds the new mapping's
    identity and does not install the old one's page;
  - two faults on one page: one frame, and the second fault sees it;
  - an atomic-context user access that would need to wait takes the EFAULT fixup and does not park;
  - a signal posted during an interrupt taken in user mode: the interrupt return sets `user_work` and
    writes no frame, and the continuation writes the frame. With the user stack page absent, the
    continuation's write faults, parks and completes. A structure test pins that the interrupt- and
    exception-return paths on both architectures do not call the frame writer;
  - ARM64 exception return, as a kernel test in the testing feature driven by fixed points, since
    the effect is the hardware's `eret`. Inside a syscall, a user copy faults at EL1. At a named point
    after the fault unmasks, the test overwrites this CPU's `user_rsp_scratch` as a nested IRQ would.
    The fault then parks and is woken on a second, named CPU. The test checks that the syscall resumes
    on its own stack with its locals intact. A second case takes the same fault inside an
    interrupts-masked section, using the nofault copy, and checks that DAIF.I and DAIF.F are still set
    after the return. Both are red on today's epilogue. A structure test pins that the sync epilogue's
    EL1 return does not load SP from the per-CPU slot outside the idle redirect, and that the epilogue
    clears no DAIF bit on an EL1 return.
- **Risk retired.** Faults that cannot wait, which T1 names as the reason for R1, R6 and R12. The busy
  refault loop (T8). It also unblocks E3's lazy mmap.
- **Rows.** `memory.anonymous-touch`, `memory.driver-touch` and `memory.malloc-4t` under contention.
  `memory.driver-mmap` waits for E3.
- **Boot.** One ARM64 QEMU boot of the cases that fault: restart depends on the hardware's exception
  return.

### E2.10a-f: preemptible syscalls by class

- **Scope.** §4.4-§4.5, one PR per class. 10a carries this group's Tier-1 commit: the class table, the
  baseline-setting entry change and the removal of the syscall-return TSS writes (§4.5).
- **Tests:**
  - a structure test that `set_tss_rsp0` and `update_tss_rsp0` are reached only from dispatch, AP
    start-up and boot, or after the exit `cli`, and not from `rust_syscall_handler`. It is red on today's
    tree at `handler.rs:313` and `:618`;
  - a structure test that the table lists exactly the converted set;
  - a structure test that the converted classes' modules have no raw spin locks or unpinned per-CPU reads
    left on their allowlists;
  - a host model of entry and exit for both classes: baseline 1 with the brake, baseline 0 without, and
    custody unchanged. It includes entry on a kill-claimed thread for each class: the custody park
    leaves the depth at the class's baseline and does not underflow. It is red on today's
    `enter_syscall` for the preemptible class;
  - a structure test that every `SyscallNumber` dispatch arm on x86 and ARM64 has a class, and that
    getrandom is not in class a; <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
  - a kernel test in the testing feature, driven by a fixed point: it sets `need_resched` at a named spot
    inside a converted syscall and checks that the thread is preempted and resumes with its result intact.
- **Risk retired.** Long syscalls that hold a CPU against runnable work. Latency to get running again then
  depends on the scheduler, not on syscall length.
- **Rows.**
  - `process.getpid` (4.19x) in 10a: each syscall drops a level take-and-release and the softirq check
    behind it. The prediction is small, because entry and exit carry other work.
  - `process.fork-exec` in 10e.
  - `network.tcp-*` and `memory.malloc-4t` from contended threads in 10c and 10d.
  - The compile rows are where the effect should show (cargo with four jobs). Since #3250 the QEMU rig
    runs them on Breenix with the hosted fork (`userland/bbench/RIG.md:130-137`), so they are read on
    the rig, and on the R240 after the batched flash.
- **Boot.** One ARM64 QEMU boot per class, of the cases that class reaches: preemption in the middle of a
  syscall is a timing effect. x86 gets its evidence at the flash.

### E2.11: retire the brake

- **Scope.** Delete the `Braked` class, the entry increment, `park_baseline` and
  `baseline_irqs_masked`. This is a Tier-1 commit of its own. In a separate commit, delete
  `park::Braked`, and have the ARM64 `test-loader` create its test processes unqueued and enqueue them
  once its reads end (§6).
- **Tests.** The structure test now requires the class table to be empty and gone. The baseline model
  reduces to depth 0 at entry.
- **Risk retired.** The last caller-dependent rule (R4 closed).
- **Rows.** As 10a. **Boot.** One ARM64 QEMU boot, plus the coordinator's flash.

## 9. Test catalogue

| Check | Where | First red in |
|---|---|---|
| Event-queue interleaving model over the queue trait (old and new queue), out-of-memory, IRQ try-lock and owed wake, deferred wake, wait ends | `#[cfg(test)]` module beside the new queue, as the existing queue tests are | E2.1 |
| `park::context` table and baseline model | `kernel/src/task/park` `#[cfg(test)]` | E2.2 |
| No private sleep predicates | `tests/park_contract_structure.rs` | E2.2 |
| Parking and parked, through the interrupt-exit decision seam; fixed-point interrupt-exit kernel test | `kernel/src/task` `#[cfg(test)]`; testing-feature kernel test | E2.3 |
| No halting as current outside idle and early boot | `tests/park_contract_structure.rs` | E2.3 |
| The DEV-24 context and the boot handoff through the admission seam; early-boot token and readiness | `kernel/src/drivers/ahci` `#[cfg(test)]`; `tools/breenfs-host-tests`; `tools/vfs-host-tests` | E2.4 |
| I7 mount-lock guard; reader, commit and writer interleavings | `tools/breenfs-host-tests` | E2.5 |
| Lock validator; raw-spin-lock ban; x86 ISR lock inventory | `kernel/src/sync` `#[cfg(test)]`; `tests/sync_lock_types_structure.rs` | E2.6, E2.7 |
| Syscall entry order | `tests/syscall_entry_order_structure.rs` | E2.7 |
| Address-space lifetime and fork copy; fault paths off PROCESS_MANAGER | host models; `tests/fault_lock_order_structure.rs`; `tools/pagetable-host-tests` | E2.8 |
| Restartable-fault interleavings through `resolve_fault`; ARM64 exception-return and signal-continuation kernel tests | `tools/pagetable-host-tests`; testing-feature kernel tests | E2.9 |
| Class table, `CpuPin` allowlist, fixed-point preemption kernel test | `tests/syscall_class_structure.rs`; testing-feature kernel test | E2.10 |

The structure tests B1 names (`tests/pipe_fifo_blocking_structure.rs`, `tests/teardown_structure.rs`,
`tests/durable_irq_wakes_test.py`, `tests/wait_epoch.rs`, `tests/timer_wake_dispatch_structure.rs`), along
with `tests/preempt_bracket_structure.rs`, `tests/breenfs_mount_lock_test.py` and
`tests/fork_lock_order_structure.rs`, are kept, or are updated only where they pin a name that changes.
E2 weakens no check in them.

## 10. bbench rows (DEV-31)

The baseline is the ARM64 QEMU series `qemu-20261127-731f1d00` (Breenix and Linux, same VM shape, 5 samples
per row). Ratio above 1 means slower than Linux (DEV-28(1)). That series predates #3250, which gave
the rig guests 2048 MiB, a BreenFS root on Breenix and an ext4 root on Linux, and Breenix compile rows
(`userland/bbench/RIG.md:20-25`, `:130-146`). So the figures below give the size of each gap, not the
comparison point. Each PR's before-series is recorded on the rig as it is when that PR lands.

| Row | Breenix | Linux | Ratio | E2 PRs that may move it |
|---|---|---|---|---|
| `process.fork-exec` (ms per child) | 2.526 | 0.218 | 11.58 | E2.3, E2.8, E2.10e |
| `process.getpid` (ms per 1M) | 455.4 | 108.8 | 4.19 | E2.10a, E2.11 (small) |
| `memory.anonymous-touch` (s) | 0.0795 | 0.0309 | 2.57 | E2.8, E2.9 |
| `memory.driver-touch` (s) | 0.00517 | 0.00134 | 3.87 | E2.8, E2.9 |
| `memory.malloc-4t` (ms) | 4662 | 7.74 | 602 | E2.8, E2.10d (in part; Phase 1.3 and E7 own the rest) |
| `network.tcp-guest-to-host` (MB/s) | 58.0 | 362.7 | 6.25 | E2.10c (contention only) |
| `network.tcp-host-to-guest` (MB/s) | 76.6 | 828.8 | 10.81 | E2.10c (contention only) |
| `memory.driver-mmap` (s) | 0.171 | 0.0000011 | 157,671 | No E2 PR. E3 (lazy mmap) on top of E2.9 |

Notes on the table:

- **Predictions, not results.** Each prediction comes from reading the code. The before-and-after series
  decides. A PR that predicts a move and does not show it says so in its body. That does not fail the PR, but it
  re-opens the attribution.
- **Neutral rows.** E2.1, E2.2, E2.6 and E2.7 are expected to be neutral on QEMU. Their series check that
  they are, chiefly on `ssh.round-trip` (0.53x) and `memory.copy-bytes` (1.03x).
- **Disk rows.** Since #3250 the rig's disk rows run on Breenix's BreenFS root over virtio-blk
  (`userland/bbench/RIG.md:139-146`). They can show E2.5 (BreenFS lock order) and E2.4's virtio-blk
  waits, but not AHCI, which only the R240 has. E2.4's AHCI part is read on the R240 after a batched
  kernel-only flash (DEV-31(2)), within DEV-23's limits.
- **Compile rows.** Since #3250 they run on Breenix with the hosted fork (`RIG.md:130-137`). They are where
  E2.10's preemption and E2.8's fault scaling should matter most (cargo with four jobs). They are read on
  the rig and on the R240.

## 11. Interfaces with other work

- **E1 (kernel memory).** A heap or frame allocation that fails returns an error. A sleepable caller may
  then park for reclaim (§5.2's memory wait target). E1 defines the wake source. E2 defines who may wait.
- **E3 (page cache).** Builds lazy mmap, readahead and fault-around on E2.9's fault protocol and its
  filling-page wait. E3 also makes exec text shared (R6), which needs faults that can sleep.
- **E4 (queued block layer).** Replaces slot-0 queueing and `PORT_IO_OWNER`. It inherits E2.4's caller
  rule and early-boot token unchanged.
- **E5 (BreenFS concurrency).** Starts from E2.5 and keeps I7 (§7.4).
- **E8 (process and VM ownership).** Starts from E2.8's `AddressSpace`, its mapping lock and the thread's
  direct reference (§5.1).
- **E9 (scheduler) and brief B3.** E9 owns the preemption-count layout and softirqs. E2 reaches them only
  through `preempt::depth()`, the guard types and `park::context()`. E9's shootdowns as ordinary IPIs
  depend on E2.7, so that CPUs no longer spin in syscalls with IF clear.
- **E6 (userspace ABI, `docs/plans/e6-userspace-abi.md`).** E6 changes what a signal frame holds: an
  XSAVE standard image, plus validation on sigreturn. E2 changes where and in what context the frame is
  written: E2.8 moves the write outside PROCESS_MANAGER, and E2.9 moves it onto the return-to-user
  continuation, where it may sleep (§5.1). The two efforts touch the same delivery code. Whichever lands
  second merges the other's change and keeps both properties. A larger XSAVE frame is also more likely to
  fault on the user stack, which E2.9 lets sleep and restart.
- **R17 (#3238, merged).** The durable interrupt wake transport E2.1 calls.

## 12. Decisions for the coordinator

- **Q1. Boot evidence for the Tier-1 changes on x86** (E2.7, E2.10a, E2.11). CLAUDE.md requires boot
  evidence for a Tier-1 edit. x86 QEMU is not used (DEV-31(2)), and this lane has no R240 access. The proposal is that
  each of these PRs merges on its host tests and review, and its boot evidence is the coordinator's next
  kernel-only flash (staging boot and SSH). The alternative is to hold the merge until a flash of the PR
  head. The coordinator decides which.
- **Q2. A BreenFS volume in the QEMU rig.** Settled by #3250: the rig's Breenix guest now runs on a
  BreenFS root, so E2.5, E5 and E11 can be judged on the primary rig (DEV-31(1)). No decision is needed.
- **Q3. Which lane carries E2.5.** If E5's lane opens first, the coordinator assigns E2.5 to one of the
  two lanes.
- **Q4. ext2's lock across I/O.** It stays out of scope (§7.4) unless the coordinator adds it. Since
  #3250 it no longer bears on the QEMU disk rows, which run on BreenFS.

## 13. References

- Intel 64 and IA-32 Architectures Software Developer's Manual, Vol. 3A:
  - "Interrupt and Exception Handling": exception classes (fault versus trap), interrupt-gate IF clearing,
    and Interrupt 14 (#PF), its error code and CR2;
  - the memory-ordering section of "Multiple-Processor Management".
- Arm Architecture Reference Manual for A-profile (DDI 0487):
  - B2, "The AArch64 Application Level Memory Model";
  - the AArch64 exception model: exception entry, ELR_EL1, ESR_EL1, FAR_EL1 and PSTATE.DAIF masking on
    entry.
- Serial ATA AHCI 1.3.1: port registers PxCI, PxIS and PxIE, and command completion.
- POSIX.1-2017 XSH §2.4, "Signal Concepts", and the EINTR and timeout rules of read, write, poll, select,
  sem_timedwait and mq_timedreceive.
- B. W. Lampson and D. D. Redell, "Experience with Processes and Monitors in Mesa", CACM 23(2), 1980.
- A. D. Birrell, "An Introduction to Programming with Threads", DEC SRC Report 35, 1989.
- C. A. R. Hoare, "Monitors: An Operating System Structuring Concept", CACM 17(10), 1974.
- J. W. Havender, "Avoiding Deadlock in Multitasking Systems", IBM Systems Journal 7(2), 1968.
- H. T. Kung and J. T. Robinson, "On Optimistic Methods for Concurrency Control", ACM TODS 6(2), 1981.
- O. Rodeh, "B-trees, Shadowing, and Clones", ACM TOS 3(4), 2008.
- A. T. Clements, M. F. Kaashoek and N. Zeldovich, "RadixVM: Scalable Address Spaces for Multithreaded
  Applications", EuroSys 2013 (address-space locking that scales with threads).
- R. H. Arpaci-Dusseau and A. C. Arpaci-Dusseau, *Operating Systems: Three Easy Pieces*: "Locks",
  "Condition Variables", "Common Concurrency Problems" and "I/O Devices".
