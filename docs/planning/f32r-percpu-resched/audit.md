# F32r Per-CPU Reschedule Audit

Date: 2026-04-20

## Scope

This audit inventories every call site in the current tree that directly sets
the global `NEED_RESCHED` flag, directly sets an x86_64/aarch64 per-CPU
`need_resched` flag, or calls `scheduler::set_need_resched()` and therefore
sets both the global flag and the current CPU's per-CPU flag.

The Linux comparison point is not a global boolean. Linux marks a specific
runqueue's current task through `resched_curr(rq)` and, for remote CPUs, sends
a targeted reschedule IPI (`/tmp/linux-v6.8/kernel/sched/core.c:1041-1063`).
The explicit helper `resched_cpu(int cpu)` locks `cpu_rq(cpu)` and delegates
to `resched_curr()` (`/tmp/linux-v6.8/kernel/sched/core.c:1065-1073`).
On arm64 the flag is per-thread/per-task state: `TIF_NEED_RESCHED` and
`_TIF_NEED_RESCHED` live in `thread_info.flags`
(`/tmp/linux-v6.8/arch/arm64/include/asm/thread_info.h:24-46`,
`:60-86`). Linux wakeups determine a target CPU in `try_to_wake_up()` and
queue the task to that CPU (`/tmp/linux-v6.8/kernel/sched/core.c:4222-4370`).

## Breenix Primitives Today

| Site | Purpose | Target CPU known at call? | How | Linux equivalent |
| --- | --- | --- | --- | --- |
| `kernel/src/task/scheduler.rs:133-134` | Declares global `NEED_RESCHED`. | No. | Single global bit has no CPU ownership. | No equivalent global bit; Linux uses `TIF_NEED_RESCHED` on the target task/current task (`thread_info.h:60-86`). |
| `kernel/src/task/scheduler.rs:2563-2570` | Public `set_need_resched()` sets global + current CPU per-CPU bit. | Current CPU only. | Uses current CPU's per-CPU storage, but global bit lets any CPU observe it. | Same-CPU branch of `resched_curr()` sets current's need-resched (`core.c:1051-1056`); remote case is targeted IPI (`core.c:1059-1060`). |
| `kernel/src/task/scheduler.rs:2572-2596` | `check_and_clear_need_resched()` reads/clears per-CPU and swaps global. | N/A read path. | Any CPU can consume and clear the global bit, which loses target ownership. | Linux clears the previous task's flag during schedule (`core.c:6691-6693`). |
| `kernel/src/task/scheduler.rs:2599-2610` | `is_need_resched()` reads per-CPU OR global. | N/A read path. | Idle gate sees global as local work. | Linux `need_resched()` is per current task/thread, not a machine-global flag. |
| `kernel/src/task/scheduler.rs:2018-2023` | `set_need_resched_inner()` sets global + aarch64 current CPU per-CPU bit while scheduler lock is held. | Current CPU only. | Called from aarch64 context-switch redirect paths. | Same-CPU `resched_curr()` behavior (`core.c:1051-1056`). |
| `kernel/src/arch_impl/aarch64/exception.rs:1243-1248` | SGI reschedule handler sets target CPU's per-CPU bit. | Yes: receiving CPU. | The GIC delivered `SGI_RESCHEDULE` to this CPU. | Remote branch of `resched_curr()` sends `smp_send_reschedule(cpu)` (`core.c:1059-1060`). |
| `kernel/src/per_cpu.rs:429-435` | x86_64 per-CPU setter. | Current CPU only. | Writes x86 per-CPU storage. | Same-CPU need-resched flag set. |
| `kernel/src/per_cpu_aarch64.rs:221-228` | aarch64 per-CPU setter. | Current CPU only. | Writes aarch64 per-CPU storage. | Same-CPU need-resched flag set. |

## Scheduler Wake And Spawn Sites

| Site | Purpose | Target CPU known at call? | How | Linux equivalent |
| --- | --- | --- | --- | --- |
| `kernel/src/task/scheduler.rs:565-587` | `add_thread_inner()` queues a new thread. | Yes internally. | `least_loaded_cpu()` is stored in local `target` (`:581-587`) but `add_thread()`/`add_thread_front()` discard it. | `try_to_wake_up()` selects a CPU (`core.c:4354-4364`) before `ttwu_queue()` (`core.c:4369`). |
| `kernel/src/task/scheduler.rs:2212-2230` | `spawn()` adds a new thread, sets global/current per-CPU, and broadcasts SGI to idle CPUs. | Yes, but not exposed. | `add_thread_inner()` computed target; public `add_thread()` currently returns `()`. | Wake path should enqueue to selected CPU and resched that runqueue (`core.c:4369`, `core.c:1041-1063`). |
| `kernel/src/task/scheduler.rs:2237-2255` | `spawn_front()` adds fork child to front, sets global/current per-CPU, and broadcasts SGI to idle CPUs. | Yes, but not exposed. | `add_thread_inner()` computed target; public `add_thread_front()` currently returns `()`. | Same as child wake/enqueue to selected CPU; no global flag (`core.c:4354-4370`). |
| `kernel/src/task/scheduler.rs:1244-1303` | `unblock()` makes blocked thread ready and queues it. | Yes when queued; yes when current. | Queued target is `find_target_cpu_for_wakeup(thread_id)` (`:1290-1291`); if still current, target is `cpu_state[cpu].current_thread`. Current callers often add a later global set. | `try_to_wake_up()` handles current task specially (`core.c:4227-4244`) or chooses `task_cpu/select_task_rq` (`core.c:4339-4369`). |
| `kernel/src/task/scheduler.rs:1437-1509` | `unblock_for_signal()` wakes a signal waiter; sets global via `set_need_resched()`. | Yes after lookup. | Queued target is `find_target_cpu_for_wakeup()` (`:1486-1487`); if current, target is owning `cpu_state` (`:1471-1474`). | `wake_up_state()`/`try_to_wake_up()` (`core.c:4505-4507`, `core.c:4222-4370`). |
| `kernel/src/task/scheduler.rs:1561-1604` | `unblock_for_child_exit()` wakes waitpid waiter; sets global via `set_need_resched()`. | Yes after lookup. | Queued target is `find_target_cpu_for_wakeup()` (`:1587-1588`); if current, target is owning `cpu_state`. | `try_to_wake_up()` target CPU selection (`core.c:4354-4369`). |
| `kernel/src/task/scheduler.rs:1705-1710` | `unblock_for_io()` wakes I/O waiter and broadcasts SGI to idle CPUs when enqueued. | Yes. | `wake_io_thread_locked()` returns `enqueued_target`; this call ignores the specific CPU and broadcasts through `send_resched_ipi()`. | Targeted `resched_curr(rq)`/`smp_send_reschedule(cpu)` (`core.c:1041-1063`). |
| `kernel/src/task/scheduler.rs:1713-1725` | `wake_waitqueue_thread()` wakes waitqueue waiter and sends targeted SGI. | Yes. | `IoWakeResult::resched_target()` returns queued target or current owner (`:170-179`, `:1721-1722`). | Closest current Breenix parity to `try_to_wake_up()` + target runqueue (`core.c:4222-4370`). |
| `kernel/src/task/scheduler.rs:1728-1770` | `wake_io_thread_locked()` core I/O wake; sets global via `set_need_resched()`. | Yes. | Captures current CPU owner (`:1752-1753`) or enqueued target (`:1765-1767`). | `try_to_wake_up()` observes `on_cpu`/`task_cpu` and queues target (`core.c:4339-4369`). |
| `kernel/src/task/scheduler.rs:1840-1897` | `wake_expired_timers()` queues expired timer/I/O sleepers. | Yes when queued. | Uses `find_target_cpu_for_wakeup(tid)` (`:1894-1896`), but does not itself resched the target. | Timer wake should be normal target wake (`core.c:4222-4370`). |
| `kernel/src/task/scheduler.rs:1944-1958` | `find_target_cpu_for_wakeup()` target lookup helper. | Yes. | Prefer current owner CPU, else least-loaded runqueue. | Simplified analogue of `task_cpu(p)`/`select_task_rq()` (`core.c:4339-4364`). |
| `kernel/src/task/scheduler.rs:2040-2126` | `rescue_stuck_ready_threads()` queues stuck Ready threads and broadcasts SGI. | Yes. | Target from `find_target_cpu_for_wakeup(tid)` (`:2123-2125`), but broadcast IPI is used. | Targeted `resched_cpu(cpu)` (`core.c:1065-1073`). |
| `kernel/src/task/scheduler.rs:2535-2544` | `yield_current()` asks current CPU to reschedule. | Yes. | Current CPU only; not a wake. | Same-CPU branch of `resched_curr()` (`core.c:1051-1056`). |
| `kernel/src/task/scheduler.rs:2636-2645` | `isr_unblock_for_io()` pushes tid into current CPU ISR wake buffer and sets global/current per-CPU. | Yes: current CPU buffer. | `current_cpu_id_raw()` selects the buffer drained on that CPU. | Deferred remote wake endpoint resembles Linux `ttwu_queue_wakelist()` but current design intentionally drains locally (`core.c:3930-4049`). |
| `kernel/src/task/scheduler.rs:846-852` | Schedule fallback switches current CPU to idle and sets per-CPU bit for follow-up scheduling. | Yes. | Current CPU is executing the schedule path. | Same-CPU `resched_curr()` branch (`core.c:1051-1056`). |
| `kernel/src/task/scheduler.rs:1127-1134` | aarch64 deferred schedule fallback switches current CPU to idle and sets per-CPU bit. | Yes. | Current CPU is executing the schedule path. | Same-CPU `resched_curr()` branch (`core.c:1051-1056`). |

## External Wake Callers Using Global `set_need_resched()`

These sites currently wake one or more known threads and then call the global
helper. The target CPU is derivable per waiter by having the scheduler wake
primitive return `IoWakeResult`/target CPU, or by moving the resched call inside
the scheduler method that already computes the target.

| Site | Purpose | Target CPU known at call? | How | Linux equivalent |
| --- | --- | --- | --- | --- |
| `kernel/src/ipc/stdin.rs:146-153` | Wake stdin readers after interrupt-context input. | Yes per reader after scheduler lookup. | Iterates concrete thread IDs; `sched.unblock(thread_id)` can return target. | `wake_up_process()`/`try_to_wake_up()` (`core.c:4499-4501`, `core.c:4222-4370`). |
| `kernel/src/ipc/stdin.rs:171-179` | ARM64 try-lock stdin reader wake. | Yes per reader after scheduler lookup. | Same concrete thread ID loop. | Same as above. |
| `kernel/src/ipc/stdin.rs:237-246` | Wake stdin readers on buffered input path. | Yes per reader after scheduler lookup. | Same concrete thread ID loop. | Same as above. |
| `kernel/src/ipc/stdin.rs:264-272` | ARM64 lock-taking stdin reader wake. | Yes per reader after scheduler lookup. | Same concrete thread ID loop. | Same as above. |
| `kernel/src/tty/driver.rs:675-683` | Wake TTY readers. | Yes per reader after scheduler lookup. | Iterates concrete waiter TIDs. | Waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/tty/pty/pair.rs:155-168` | Wake PTY master waiters. | Yes per waiter after scheduler lookup. | Iterates concrete waiter TIDs. | Waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/tty/pty/pair.rs:186-198` | Wake PTY slave waiters. | Yes per waiter after scheduler lookup. | Iterates concrete waiter TIDs. | Waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/tty/pty/pair.rs:382-389` | PTY hangup wakes signal/child-exit waiters. | Yes after scheduler lookup. | Has a concrete `thread_id`; scheduler methods can return targets. | Signal wake follows `wake_up_state()`/`try_to_wake_up()` (`core.c:4505-4507`). |
| `kernel/src/socket/udp.rs:132-139` | Wake UDP recv waiters. | Yes per waiter after scheduler lookup. | Iterates concrete waiter TIDs. | Socket waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/net/tcp.rs:1558-1570` | Wake TCP accept waiters. | Yes per waiter after scheduler lookup. | Iterates concrete waiter TIDs. | Socket waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/net/tcp.rs:1575-1588` | Wake TCP connection waiters. | Yes per waiter after scheduler lookup. | Iterates concrete waiter TIDs. | Socket waitqueue wake -> `try_to_wake_up()` (`core.c:4222-4370`). |
| `kernel/src/drivers/virtio/gpu_pci.rs:1488-1494` | Wake compositor waiting for GPU command completion. | Yes after scheduler lookup. | `GPU_WAITING_THREAD` holds concrete TID; `sched.unblock()` can return target. | Device completion wake -> `try_to_wake_up()` target CPU (`core.c:4222-4370`). |
| `kernel/src/socket/udp.rs:139`, `kernel/src/net/tcp.rs:1570`, `kernel/src/net/tcp.rs:1588` | Network waiter global resched calls. | Yes per waiter after scheduler lookup. | Duplicate direct global helper rows called out because these are likely high traffic. | Same as socket waitqueue wake above. |
| `kernel/src/task/kthread.rs:183-192` | `kthread_unpark()` wakes parked kernel thread. | Yes after scheduler lookup. | Handle stores concrete TID. | `wake_up_process()` (`core.c:4499-4501`). |
| `kernel/src/task/kthread.rs:216-242` | `kthread_exit()` terminates current kthread and asks current CPU to reschedule. | Yes. | Current CPU must stop running terminated current thread. | Same-CPU `resched_curr()` (`core.c:1051-1056`). |

## Syscall, Signal, Fault, And Timer Sites

These sites are mostly "current CPU must stop running this thread" rather than
cross-CPU wakeups. They should become `resched_cpu(current_cpu)` or a current
CPU helper, not a global broadcast.

| Site | Purpose | Target CPU known at call? | How | Linux equivalent |
| --- | --- | --- | --- | --- |
| `kernel/src/interrupts/timer.rs:62-66` | x86_64 quantum expiry. | Yes. | Timer interrupt is running on the target CPU. | Timer tick calls scheduler on local runqueue; local need-resched flag. |
| `kernel/src/arch_impl/aarch64/timer_interrupt.rs:716-720` | aarch64 quantum expiry. | Yes. | Timer interrupt's `cpu_id` indexes the current CPU. | Local tick marks current runqueue/task for resched. |
| `kernel/src/arch_impl/aarch64/timer_interrupt.rs:723-730` | aarch64 idle CPU fast path sets need-resched every tick. | Yes but this is the CPU burn source. | Uses `cpu_id`; should eventually be unnecessary once wakeups target idle CPUs. | Linux idle exits on local `need_resched()`; no global polling bit. |
| `kernel/src/syscall/handlers.rs:213-215` | `sys_exit` terminates current thread. | Yes. | Current syscall CPU. | Current task exits and schedules locally. |
| `kernel/src/syscall/handler.rs:683-689` | x86_64 signal termination after syscall delivery. | Yes. | Current syscall CPU. | Current task needs local resched. |
| `kernel/src/arch_impl/aarch64/syscall_entry.rs:276-282` | aarch64 signal termination after syscall delivery. | Yes. | Current syscall CPU. | Current task needs local resched. |
| `kernel/src/syscall/signal.rs:158-168` | `SIGKILL` process termination. | Partly. | Process state known, but target thread/CPU is not resolved at this call; should wake/mark affected threads explicitly. | Linux signal wake paths resolve target tasks and call `try_to_wake_up()` as needed. |
| `kernel/src/syscall/signal.rs:178-185` | `SIGCONT` readies blocked process. | Partly. | Process is known; target thread/CPU must be derived from process threads. | Targeted signal wake through `wake_up_state()`/`try_to_wake_up()` (`core.c:4505-4507`). |
| `kernel/src/syscall/signal.rs:203-206` | Ordinary signal readies blocked process. | Partly. | Process is known; target thread/CPU must be derived from process threads. | Targeted signal wake through `wake_up_state()`/`try_to_wake_up()`. |
| `kernel/src/arch_impl/aarch64/exception.rs:461-468` | Data abort terminates current user thread and redirects to idle. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/arch_impl/aarch64/exception.rs:482-494` | Kernel-mode data abort cleanup redirect. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/arch_impl/aarch64/exception.rs:810-813` | Instruction abort terminates current user thread and redirects to idle. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/arch_impl/aarch64/exception.rs:824-836` | Kernel-mode instruction abort cleanup redirect. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/arch_impl/aarch64/context_switch.rs:3460-3463` | Signal delivery terminated current process on aarch64 IRQ/syscall return path. | Yes. | Current CPU. | Current task termination schedules locally. |
| `kernel/src/arch_impl/aarch64/context_switch.rs:3468-3471` | Delivered signal caused current process termination. | Yes. | Current CPU. | Current task termination schedules locally. |
| `kernel/src/interrupts.rs:1419-1430` | x86_64 page-fault/current process termination. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/interrupts.rs:1728-1736` | x86_64 general-protection/current process termination. | Yes. | Faulting CPU is current CPU. | Current task termination schedules locally. |
| `kernel/src/interrupts/context_switch.rs:151-159` | x86_64 context switch abort because process manager lock is busy. | Yes. | Current CPU should retry later. | Same-CPU `resched_curr()` retry behavior. |
| `kernel/src/interrupts/context_switch.rs:286-288` | x86_64 context save failure retry. | Yes. | Current CPU. | Same-CPU `resched_curr()` retry behavior. |
| `kernel/src/interrupts/context_switch.rs:828-835` | x86_64 context restore failure retry. | Yes. | Current CPU. | Same-CPU `resched_curr()` retry behavior. |
| `kernel/src/interrupts/context_switch.rs:1016-1025` | x86_64 bad userspace context terminates thread. | Yes. | Current CPU. | Current task termination schedules locally. |
| `kernel/src/interrupts/context_switch.rs:1107-1123` | x86_64 signal delivery terminates current process. | Yes. | Current CPU. | Current task termination schedules locally. |
| `kernel/src/interrupts/context_switch.rs:1338-1349` | x86_64 signal delivery terminates current process, second path. | Yes. | Current CPU. | Current task termination schedules locally. |
| `kernel/src/preempt_count_test.rs:176-192` | Test-only direct x86 per-CPU need-resched set. | Yes. | Current CPU under test. | Unit-style local flag testing; not production wake path. |

## Broadcast Or Targeted SGI Sources

These sites do not directly set the local flag, but they cause
`exception.rs:1247` to set a remote CPU's per-CPU bit. They matter because the
new `resched_cpu(target)` primitive should replace both the flag set and the
IPI send.

| Site | Purpose | Target CPU known at call? | How | Linux equivalent |
| --- | --- | --- | --- | --- |
| `kernel/src/task/scheduler.rs:1308-1344` | `send_resched_ipi()` broadcasts SGI to all idle CPUs except current. | No single target. | Scans `cpu_state` for idle current threads. This was useful as a wake-all fallback but is not Linux-like. | Linux targets one runqueue via `resched_curr(rq)`; `wake_up_if_idle(cpu)` still takes a specific CPU (`core.c:3946-3955`). |
| `kernel/src/task/scheduler.rs:1346-1362` | `send_resched_ipi_to_cpu(target_cpu)` sends SGI to one CPU. | Yes. | Explicit argument. It currently returns without setting local per-CPU state for same-CPU targets. | Remote branch of `resched_curr()` (`core.c:1059-1060`). |
| `kernel/src/task/scheduler.rs:1222-1227` | `requeue_thread_after_save()` queues current CPU's deferred thread and broadcasts SGI. | Yes. | Queues to `Self::current_cpu_id()` (`:1224-1225`) but wakes all idle CPUs. | Targeted `resched_cpu(cpu)` (`core.c:1065-1073`). |
| `kernel/src/task/scheduler.rs:1290-1302` | `unblock()` queues to target CPU and broadcasts SGI. | Yes. | Target local variable from `find_target_cpu_for_wakeup()`. | Targeted wake CPU in `try_to_wake_up()` (`core.c:4354-4369`). |
| `kernel/src/task/scheduler.rs:1486-1497` | `unblock_for_signal()` queues to target CPU and broadcasts SGI. | Yes. | Target local variable. | Targeted wake CPU in `try_to_wake_up()`. |
| `kernel/src/task/scheduler.rs:1587-1599` | `unblock_for_child_exit()` queues to target CPU and broadcasts SGI. | Yes. | Target local variable. | Targeted wake CPU in `try_to_wake_up()`. |
| `kernel/src/task/scheduler.rs:1705-1709` | `unblock_for_io()` ignores returned target and broadcasts. | Yes. | `wake.enqueued_target` is available. | Targeted `resched_curr(rq)`. |
| `kernel/src/task/scheduler.rs:1721-1722` | `wake_waitqueue_thread()` sends targeted SGI. | Yes. | Uses `IoWakeResult::resched_target()`. | Targeted `resched_curr(rq)`. |
| `kernel/src/task/scheduler.rs:2123-2125` | Rescue path queues stuck thread to target and broadcasts. | Yes. | Target local variable. | Targeted `resched_cpu(cpu)`. |
| `kernel/src/task/scheduler.rs:2226-2229` | `spawn()` broadcasts after adding new thread. | Yes internally but dropped. | `add_thread_inner()` target should be returned. | Targeted wake CPU in `try_to_wake_up()`. |
| `kernel/src/task/scheduler.rs:2249-2254` | `spawn_front()` broadcasts after adding fork child. | Yes internally but dropped. | `add_thread_inner()` target should be returned. | Targeted wake CPU in `try_to_wake_up()`. |
| `kernel/src/arch_impl/aarch64/context_switch.rs:2095-2105` | CPU0 user dispatch guard requeues a thread and sends SGI to CPU0. | Yes. | Explicit self target CPU0 to drain deferred requeue. | Explicit `resched_cpu(cpu)` helper (`core.c:1065-1073`). |

## Findings

1. The high-traffic idle-gate bug is structurally explained by
   `is_need_resched()` mixing local per-CPU state with global `NEED_RESCHED`.
   Any wake on any CPU can make every idle CPU believe it has local scheduling
   work.
2. Most wake sites already have enough information to target a CPU. The target
   is either a local variable (`find_target_cpu_for_wakeup()` result), a known
   current owner from `cpu_state`, the current CPU in timer/fault/yield paths,
   or an explicit SGI target.
3. Spawn and spawn_front compute the target but discard it. Converting
   `add_thread()`/`add_thread_front()` to return the selected CPU is the minimal
   API change needed before targeting those wakeups.
4. The remaining "partly known" process-signal sites operate at process level,
   not thread/runqueue level. They should be converted by resolving affected
   thread IDs and applying the same per-thread wake target loop, not by keeping
   a global fallback.
5. Broadcast SGI helpers are separate from the global atomic, but they are the
   same architectural smell: a wake site should mark and notify the target CPU
   selected by the enqueue operation, matching Linux `resched_curr(rq)`.
