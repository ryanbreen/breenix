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
  (`kernel/src/task/thread.rs:1278-1300`) keeps SIGKILL out of a running syscall on its own. E2 keeps it.

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
boot still holds a level from `kernel_main` until init is launched (`kernel/src/main_aarch64.rs:2146`),
and the AHCI comment at `mod.rs:424-426` says this is why ARM64 has to poll.

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
  no `MountLock` held. The one exception is a mount that has not been published yet (§7).
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
- the mask flag is true for an x86 syscall until §4.3.

`LocksHeld(n)` means the depth exceeds the baseline by n. A park releases exactly the baseline before it
switches and restores it after (B1 requirement 9). It does no further arithmetic. The "add a level so the
wait can remove one" step (`ahci/mod.rs:487`, `mount_lock.rs:141-144`) goes away. Because the baseline is
stored in the thread and not in the per-CPU count, it survives migration across a sleep. E9 owns the count
layout (brief B3), and E2 reaches the count only through `preempt::depth()` and guard types.

### 3.3 The protocol

The event-queue half is brief B1, rewritten in PR E2.1. The park half wraps it.

```
park::wait(queue, cond, deadline, mode) -> WaitEnd
  0. ctx = context(); if ctx != Sleepable -> receipt, return Err(NotSleepable)
  1. reserve the waiter node; failure -> return NoMemory (nothing published; B1 req. 3)
  2. lock queue; if cond() -> unlock, return Condition
     link the node; state := Parking(mode, deadline); unlock          (B1 req. 1-2)
  3. if mode allows signals and one is pending -> end Interrupted     (#1230 order)
  4. commit: release the baseline, switch out if still Parking -> Parked
  5. on return: unlink if still linked, state := Running, restore the baseline (B1 req. 6, 9)
  -> WaitEnd::{Condition, Woken, Timeout, Interrupted(EINTR | ERESTARTSYS), Killed, NoMemory}
```

- **Wakers.** A waker makes the condition true with a release store, then takes the queue lock. From task
  context it hands each chosen node to the scheduler, which moves `Parking` or `Parked` to `Runnable`. It
  unlinks the node only after the scheduler has accepted the wake (B1 requirement 7). From hard IRQ it
  uses try-lock and the durable `isr_unblock_for_io` (B1 requirement 4).
- **Waiting on several queues.** `park::wait_any` registers on each of its queues, publishes once, rechecks each
  condition, then commits. It is the `add_waiter` / `take_waiter` shape from B1's caller table, used by
  poll, select, epoll and the GPU and console multi-waits.
- **Timeouts.** A timeout and a wake race to move the state from `Parked` to `Runnable`. Whichever moves it
  first records the end reason, so the wait ends once (B1 requirement 8).
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
  `Woken`. Only step 4 of §3.3 dequeues the thread.
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

Order, least risk first:

- **a. Leaf syscalls:** getpid, gettid, getppid, clock and time reads, uname, getrandom without blocking.
  They exercise the mechanism first on both architectures.
- **b. File and pipe I/O:** read, write, pread, pwrite, lseek, readv, writev, fsync, getdents, the stat
  family, open, close, pipes and FIFOs.
- **c. Waits:** sockets, poll, select, epoll, futex, nanosleep, clock_nanosleep, the SysV and POSIX IPC
  waits, and TTY and PTY.
- **d. Memory:** mmap, munmap, mprotect, brk, madvise and mlock. These follow E2.8 and E2.9, because they
  take the address-space lock.
- **e. Process:** fork, clone, execve, exit, exit_group, wait4, waitid, kill and the signal syscalls. These
  touch PROCESS_MANAGER and the stop and signal machinery, so they go last.

E2.11 then deletes the `Braked` class, the entry increment and `park_baseline`. Both syscall entries run
at depth 0.

### 4.5 Per-CPU state across preemption

Once a syscall can be preempted, a CPU index read before a preemption point can be stale after it. Outside
`kernel/src/task`, `kernel/src/arch_impl` and the interrupt handlers, `cpu_id()` is called 9 times in 6
files under `syscall/`, `fs/`, `ipc/`, `net/`, `socket/`, `memory/`, `process/` and `block/`. The
user-copy nofault flag is one of them: `NOFAULT` is indexed by CPU (`kernel/src/syscall/userptr.rs:292`,
`:302`, `:314`). Each of those reads either moves into a `CpuPin` guard, which raises depth for its scope,
or becomes per-thread state. The nofault flag becomes per-thread, because the copy it covers may now
migrate. A structure test pins the remaining unguarded sites with an allowlist that shrinks to zero. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

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
- **Fork.** The fork copy takes the parent's mapping lock exclusive. A parent thread that faults during
  the copy then sleeps until the copy ends, where today it faults again and again
  (`kernel/src/memory/fork_copy.rs`, `anon_map.rs:34`).
- **Signal frames.** Delivery stops writing frames under PROCESS_MANAGER. The PROCESS_MANAGER section
  chooses the signal and records the disposition in the thread. The frame is written on the
  return-to-user path in sleepable context, where a fault on the user stack can sleep and restart.

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
  look up the VMA, check permissions, find or allocate the page:
    resolved             -> install the entry under as.pt; return (the access re-executes and succeeds)
    must wait for X      -> drop as.mapping; park on X (killable); return (re-executes, faults again)
    permanent failure    -> SIGSEGV or SIGBUS, or the uaccess fixup and EFAULT
```

- **Wait targets.** X is one of:
  - an exclusive holder of the mapping lock (munmap, mprotect, fork copy, exec teardown);
  - a page in a filling state (E3 fills pages from the page cache; in E2 the hook is exercised by the
    mapping cache's in-flight pin from #3232);
  - frame allocation waiting for memory (E1 returns allocation failure as an error, and its reclaim
    wakes).
- **Revalidation.** A fault that slept keeps nothing from before the sleep. The re-executed access faults
  afresh, takes the lock again and repeats the whole lookup, so an unmap, protection change or exec in
  between is seen. A counter of retries per fault feeds the receipts, so a livelock shows up by name. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Atomic contexts.** A kernel access to user memory from an atomic context must use the nofault copy,
  which turns any fault that would need to wait into EFAULT (`write_user_bytes_nofault` today). It never
  parks. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
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
- **Boot order.** ARM64 moves its init load out of the braked boot continuation into a sleepable kthread,
  as #3228 did for x86, so nothing reads storage on the boot thread after the scheduler starts. This may
  also bear on the ARM64 `-smp 1` boot wedge (#617, #620), because CPU 0 stops being held through init.
  That PR's code reading has to settle whether it does. This plan makes no claim. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
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

So a node or data extent read without the mount lock stays valid for as long as the reader's generation
is pinned. Only the open generation's in-memory state (dirty pages, the replay log and the inode edits)
needs the lock.

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

### 7.3 Write paths and the commit worker

- **Writes** stage dirty data in memory under the lock, as today.
- **A full dirty budget.** Today the writer commits inline (audit R11). Instead, the writer releases the
  lock, signals the commit worker and parks on commit progress, then retries. It never waits on a FLUSH
  while holding the lock. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **The commit worker** freezes the generation under the lock and writes and flushes without it. It takes
  the lock again only to publish. E2.5 takes an inventory of every device wait currently reached under the
  lock and lists what each becomes in that PR's body, checked by the guard in §9. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->
- **Contenders.** They always park, because every caller is sleepable (§3). The spin at
  `mount_lock.rs:136-138` goes, and so does the `EAGAIN` path. `preempt.rs`'s cooperative points stay
  until E2.10b makes the BreenFS syscalls preemptible, and then go. <!-- claim-lint:ok: normative requirement of docs/plans/e2-sleepable-contexts.md under ruling DEV-37(2), not a measured result. -->

### 7.4 The boundary with E5

E2.5 is the first step of E5 ("release the mount lock before I/O") and carries the I7 guard. E5 then adds
per-inode locks, per-node tree locks, N+1 copy-on-write over N with no replay, and the dentry cache, and
keeps I7 throughout. The PR belongs to E2's sequence. If E5's lane starts first, the coordinator decides
which lane carries it (§12, Q3). It must not land twice.

ext2's lock (#728) moves onto the park contract in E2.2. It keeps holding its lock across device reads,
because the ruling names only BreenFS. That matters for measurement: the QEMU rig's disk rows run on ext2
(§10).

## 8. PR sequence

Each E2 PR follows the same rules:

- its check is its own first commit and is red on the code before it;
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
| E2.10a-e | Preemptible syscalls by class (§4.4), with the `CpuPin` audit | E2.6; x86 also E2.7; 10d needs E2.9 | `syscall/handler.rs` (Tier 1), once, for the class table |
| E2.11 | Retire the brake, the `Braked` class and the baselines | E2.10a-e | `syscall/handler.rs` (Tier 1) |

E2.3, E2.4 and E2.5 are independent of each other after E2.2, and E2.6 can run beside them. One lane
should own each, not several agents on one PR (DEV-30(3)).

### E2.1: clean-room event queue (B1)

- **Scope.** Replace `WaitQueueHead` and its free functions, and the two scheduler hooks B1 names, with a
  new implementation written from B1 alone. Update callers in the same change. Semantics are unchanged: B1
  requirements 1-11, including today's preemption contract (requirement 9) and the x86 halting waiter
  (requirement 11). Later PRs change the semantics.
- **Who writes it.** A lane that reads B1, the specifications B1 names, the public signatures and the
  callers. It does not read the old bodies or the documents §0 excludes.
- **Tests, check-first.** These are B1's list:
  - a host model that enumerates the interleavings of the waiter's register, check, sleep and finish
    steps against the waker's set-condition and wake steps, asserting requirement 1;
  - the same enumeration for timeout against wake, signal against wake, wake-one with two waiters, and a
    waker in interrupt context;
  - out of memory at reservation;
  - the IRQ try-lock under a held lock;
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
  while holding the lock, now gets a receipt and an error.
- **Tests:**
  - a host table test of `context()` across its input space;
  - a host model of the baseline: each `WaitEnd` returns with the caller's entry depth (B1 requirement 9);
  - a structure test that no file outside `kernel/src/task/park*` compares `preempt_count()` with 1 or 0
    to decide whether to sleep, or adds a level for a wait. On today's tree it finds the sites in §1.2.
- **Risk retired.** "Whether code may sleep is an unstated property of its caller" (R4). The
  level-arithmetic hazard (the #672 underflow guard's class).
- **Rows.** No row is predicted to move. **Boot.** Not needed: the predicate is pure, and the host tests settle it.

### E2.3: parking and parked

- **Scope.** §3.5: the `Parking` state, requeueing on an involuntary switch, and committing only in
  `park::wait`. The non-preemptible branches of `arch_halt_while_blocked` become violations. The
  "yield before parking" workarounds go, because a committed park switches away.
- **Tests:**
  - a host scheduler-state model: publish, then an involuntary switch at depth 0, leaves the thread
    runnable; publish, wake, commit ends `Woken` with no sleep; publish, commit, wake ends `Woken` once;
  - a structure test that no task-context wait halts as current outside idle, the early-boot path and the
    pinned-idle case.
- **Risk retired.** The stranded lock holder of #1163 and #1155. x86 `can_schedule`'s BlockedOnIO gap
  (#666, #508).
- **Rows.** `process.fork-exec` is the candidate, because the parent's wait4 sleeps and wakes. It is 11.58x
  Linux today, and most of that is fork and exec work, so the prediction is a small move. The series
  decides.
- **Boot.** One ARM64 QEMU boot: the interrupt-exit switch path is the hardware exception return.

### E2.4: block I/O caller rule

- **Scope.** §6.
- **Tests:**
  - a host model of port admission with the DEV-24 interleaving: a sleepable caller arrives while a commit
    worker owns the port, and parks and gets the port after `end_port_io`. It is red on the old code, which
    returns `CannotPark`;
  - an atomic caller gets `NotSleepable`, a receipt and no data;
  - the early-boot token is refused after revocation, and polled I/O with the token while the port has an
    owner is refused;
  - the updated expectations in `tools/breenfs-host-tests` and `tools/vfs-host-tests`;
  - a structure test that `CannotPark`, the admission `yield` loop and the level arithmetic are gone.
- **Risk retired.** DEV-24 and its family (#3209). Context-dependent I/O modes.
- **Rows.** No QEMU row: the rig's disks are virtio-blk and its disk rows run on ext2. On the R240 the
  effect is boot and dev-loop reliability, read from the next kernel-only flash.
- **Boot.** One ARM64 QEMU boot, because the ARM64 init load moves to a kthread and that changes boot
  order. x86's part reaches hardware at the coordinator's flash.

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
  - **the write path:** a full dirty budget makes the writer park on commit progress without holding the
    lock.
- **Risk retired.** The BreenFS half of DEV-24 (R10). One reader's device wait stalls the whole mount,
  including commit publication.
- **Rows.** No row on the QEMU rig as it stands (§10). On the R240, `disk.stat`, `disk.create` and
  `disk.unlink` while the commit worker is active, and `compile.cargo-noop`. DEV-23's ban still keeps
  `disk.walk` and `disk.sequential-*` off the R240 until #3226 closes.
- **Boot.** Not needed. The host harness runs the production BreenFS code and settles lock order and the
  interleavings.

### E2.6: lock types and the validator

- **6a.** `sync::{SpinLock, IrqSpinLock, SleepLock, SleepRwLock}`, with `IrqSafeMutex` and `MountLock`
  becoming instances. The §4.2 validator. A structure guard bans `spin::Mutex` and `spin::RwLock` outside
  `kernel/src/sync/`, with an allowlist holding today's 126 files.
- **6b onward.** Migration by subsystem (drivers, fs, net and socket, ipc, memory, process, gpu). Each PR
  shrinks the allowlist and is safe alone, because a preempt-raising lock excludes what a raw one does and also keeps its holder on the CPU.
  The x86 ISR lock inventory (§4.3) lands with the driver migration.
- **Tests:**
  - host tests of the validator: an inverted pair is flagged; a class taken in IRQ and with interrupts on
    is flagged; a `SpinLock` across `park::wait` is flagged;
  - host tests that each guard raises and restores depth and mask in order;
  - the structure guard, red on today's tree.
- **Risk retired.** A preempted holder with its contender spinning on the same CPU (#1394's class). Locks
  shared with ISRs without masking, which E2.7 needs gone first.
- **Rows.** No row is predicted to move. **Boot.** Not needed. The migration is mechanical, and the validator carries the
  runtime evidence into the final run.

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
  - a host model of the fork copy: a parent fault during the copy sleeps, then resolves against the
    finished copy;
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

- **Scope.** §5.2.
- **Tests,** as host fixed interleavings against the fault model, each red on the old code, which returns
  `Resolved` without sleeping:
  - a fault sleeps behind a munmap holding the lock exclusive; after the wake the VMA is gone, the result
    is SIGSEGV, and no stale entry is installed;
  - a fault sleeps; mprotect changes the protection; the retried access gets the new protection;
  - a fault sleeps during a fork copy, and no entry is written into the copying address space before the
    copy completes;
  - SIGKILL during a fault's sleep: the thread exits, and no entry is installed;
  - two faults on one page: one frame, and the second fault sees it;
  - an atomic-context user access that would need to wait takes the EFAULT fixup and does not park.
- **Risk retired.** Faults that cannot wait, which T1 names as the reason for R1, R6 and R12. The busy
  refault loop (T8). It also unblocks E3's lazy mmap.
- **Rows.** `memory.anonymous-touch`, `memory.driver-touch` and `memory.malloc-4t` under contention.
  `memory.driver-mmap` waits for E3.
- **Boot.** One ARM64 QEMU boot of the cases that fault: restart depends on the hardware's exception
  return.

### E2.10a-e: preemptible syscalls by class

- **Scope.** §4.4-§4.5, one PR per class. The class table and the baseline-setting entry change land in
  10a, as the one Tier-1 edit of this group.
- **Tests:**
  - a structure test that the table lists exactly the converted set;
  - a structure test that the converted classes' modules have no raw spin locks or unpinned per-CPU reads
    left on their allowlists;
  - a host model of entry and exit for both classes: baseline 1 with the brake, baseline 0 without, and
    custody unchanged;
  - a kernel test in the testing feature, driven by a fixed point: it sets `need_resched` at a named spot
    inside a converted syscall and checks that the thread is preempted and resumes with its result intact.
- **Risk retired.** Long syscalls that hold a CPU against runnable work. Latency to get running again then
  depends on the scheduler, not on syscall length.
- **Rows.**
  - `process.getpid` (4.19x) in 10a: each syscall drops a level take-and-release and the softirq check
    behind it. The prediction is small, because entry and exit carry other work.
  - `process.fork-exec` in 10e.
  - `network.tcp-*` and `memory.malloc-4t` from contended threads in 10c and 10d.
  - The compile rows are where the effect should show (cargo -j4), but they are deferred on the QEMU rig
    (`NO_BREENIX_RUSTC`), so they are read on the R240 after the batched flash.
- **Boot.** One ARM64 QEMU boot per class, of the cases that class reaches: preemption in the middle of a
  syscall is a timing effect. x86 gets its evidence at the flash.

### E2.11: retire the brake

- **Scope.** Delete the `Braked` class, the entry increment, `park_baseline` and
  `baseline_irqs_masked`. This is a Tier-1 commit of its own.
- **Tests.** The structure test now requires the class table to be empty and gone. The baseline model
  reduces to depth 0 at entry.
- **Risk retired.** The last caller-dependent rule (R4 closed).
- **Rows.** As 10a. **Boot.** One ARM64 QEMU boot, plus the coordinator's flash.

## 9. Test catalogue

| Check | Where | First red in |
|---|---|---|
| Event-queue interleaving model, out-of-memory, IRQ try-lock, wait ends | `#[cfg(test)]` module beside the new queue, as the existing queue tests are | E2.1 |
| `park::context` table and baseline model | `kernel/src/task/park` `#[cfg(test)]` | E2.2 |
| No private sleep predicates | `tests/park_contract_structure.rs` | E2.2 |
| Parking and parked scheduler model | host model in `kernel/src/task` `#[cfg(test)]` | E2.3 |
| No halting as current outside idle and early boot | `tests/park_contract_structure.rs` | E2.3 |
| Port admission with the DEV-24 interleaving; early-boot token | `kernel/src/drivers/ahci` `#[cfg(test)]` model; `tools/breenfs-host-tests`; `tools/vfs-host-tests` | E2.4 |
| I7 mount-lock guard; reader, commit and writer interleavings | `tools/breenfs-host-tests` | E2.5 |
| Lock validator; raw-spin-lock ban; x86 ISR lock inventory | `kernel/src/sync` `#[cfg(test)]`; `tests/sync_lock_types_structure.rs` | E2.6, E2.7 |
| Syscall entry order | `tests/syscall_entry_order_structure.rs` | E2.7 |
| Address-space lifetime and fork copy; fault paths off PROCESS_MANAGER | host models; `tests/fault_lock_order_structure.rs`; `tools/pagetable-host-tests` | E2.8 |
| Restartable-fault interleavings | host fault model | E2.9 |
| Class table, `CpuPin` allowlist, fixed-point preemption kernel test | `tests/syscall_class_structure.rs`; testing-feature kernel test | E2.10 |

The structure tests B1 names (`tests/pipe_fifo_blocking_structure.rs`, `tests/teardown_structure.rs`,
`tests/durable_irq_wakes_test.py`, `tests/wait_epoch.rs`, `tests/timer_wake_dispatch_structure.rs`), along
with `tests/preempt_bracket_structure.rs`, `tests/breenfs_mount_lock_test.py` and
`tests/fork_lock_order_structure.rs`, are kept, or are updated only where they pin a name that changes.
E2 weakens no check in them.

## 10. bbench rows (DEV-31)

The baseline is the ARM64 QEMU series `qemu-20261127-731f1d00` (Breenix and Linux, same VM shape, 5 samples
per row). Ratio above 1 means slower than Linux (DEV-28(1)).

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
- **Disk rows.** These do not show E2.4 or E2.5 on this rig: its disk rows run on ext2 on virtio-blk
  (`userland/bbench/RIG.md:88-91`), so they measure neither AHCI nor BreenFS. Until the rig has a BreenFS
  volume, those two PRs are measured on the R240 after a batched kernel-only flash (DEV-31(2)), within
  DEV-23's limits (§12, Q2).
- **Compile rows.** These are deferred on the rig (`NO_BREENIX_RUSTC`). They are where E2.10's preemption
  and E2.8's fault scaling should matter most (cargo -j4), and they are read on the R240.

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
- **R17 (#3238, merged).** The durable interrupt wake transport E2.1 calls.

## 12. Decisions for the coordinator

- **Q1. Boot evidence for the Tier-1 changes on x86** (E2.7, E2.10a, E2.11). CLAUDE.md requires boot
  evidence for a Tier-1 edit. x86 QEMU is not used (DEV-31(2)), and this lane has no R240 access. The proposal is that
  each of these PRs merges on its host tests and review, and its boot evidence is the coordinator's next
  kernel-only flash (staging boot and SSH). The alternative is to hold the merge until a flash of the PR
  head. The coordinator decides which.
- **Q2. A BreenFS volume in the QEMU rig.** Adding a second virtio disk formatted BreenFS for the disk rows
  would let E2.5, E5 and E11 be judged on the primary rig (DEV-31(1)). The rig is outside E2. The proposal
  is a rig PR before E2.5 lands.
- **Q3. Which lane carries E2.5.** If E5's lane opens first, the coordinator assigns E2.5 to one of the
  two lanes.
- **Q4. ext2's lock across I/O.** It stays out of scope (§7.4) unless the coordinator adds it. It bears on
  the QEMU disk rows while the rig runs ext2.

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
