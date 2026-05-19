# Turn 1 - Linux Wake/Enqueue Forensic

## A. Linux wake-enqueue atomicity invariant

Environment: `linux-probe` is Ubuntu `6.8.0-107-generic` on aarch64 Parallels (`turn1-artifacts/linux-probe/uname.txt`).

Runtime ftrace evidence from `turn1-artifacts/linux-probe/ftrace-wake-pipe.txt` shows the two Linux wake shapes that matter:

```text
CPU0: wake_up_state() -> try_to_wake_up() -> ttwu_queue_wakelist()
CPU1: ttwu_do_activate() -> enqueue_task_fair()
```

Example artifact lines: ftrace lines 31-37 show `try_to_wake_up()` on CPU0 calling `ttwu_queue_wakelist()`, followed by `ttwu_do_activate()` and `enqueue_task_fair()` on CPU1. Repeated instances appear throughout the trace. Counts in that trace:

- `try_to_wake_up()`: 185
- `ttwu_queue_wakelist()`: 170
- `ttwu_do_activate()`: 170
- `enqueue_task_fair()`: 372
- `wake_up_new_task()`: 101

The source invariant in Linux 6.8 is:

1. `try_to_wake_up()` serializes concurrent wakeups with `p->pi_lock`.
2. If the task is already queued (`p->on_rq`), Linux takes the task rq lock in `ttwu_runnable()` and publishes `TASK_RUNNING` without a new enqueue.
3. If the task was dequeued, Linux first publishes the transitional state `TASK_WAKING`, not `TASK_RUNNING`.
4. Actual activation happens under the destination `rq->lock` in `ttwu_do_activate()`.
5. `activate_task()` calls the scheduler-class enqueue and sets `p->on_rq`.
6. Only after activation/enqueue does `ttwu_do_wakeup()` publish `TASK_RUNNING`.

Relevant source anchors in `turn1-artifacts/linux-source/core.c`:

```c
/* core.c:3768-3772 */
static inline void ttwu_do_wakeup(struct task_struct *p)
{
	WRITE_ONCE(p->__state, TASK_RUNNING);
	trace_sched_wakeup(p);
}
```

```c
/* core.c:3775-3797 */
ttwu_do_activate(struct rq *rq, struct task_struct *p, int wake_flags,
		 struct rq_flags *rf)
{
	lockdep_assert_rq_held(rq);
	activate_task(rq, p, en_flags);
	wakeup_preempt(rq, p, wake_flags);
	ttwu_do_wakeup(p);
}
```

```c
/* core.c:2139-2148 */
void activate_task(struct rq *rq, struct task_struct *p, int flags)
{
	enqueue_task(rq, p, flags);
	WRITE_ONCE(p->on_rq, TASK_ON_RQ_QUEUED);
}
```

```c
/* core.c:4312-4369, condensed */
WRITE_ONCE(p->__state, TASK_WAKING);
if (smp_load_acquire(&p->on_cpu) &&
    ttwu_queue_wakelist(p, task_cpu(p), wake_flags))
	break;
smp_cond_load_acquire(&p->on_cpu, !VAL);
cpu = select_task_rq(p, p->wake_cpu, wake_flags | WF_TTWU);
ttwu_queue(p, cpu, wake_flags);
```

The key invariant is stricter than "state mutation and enqueue are both in `try_to_wake_up()`": Linux does not publish the externally runnable state until the task is either already on a runqueue or has been enqueued under the target runqueue lock. `TASK_WAKING` is the bridge state that permits remote wake-list queueing without creating "running/runnable but unreachable from a runqueue."

## B. Linux cross-CPU wake mechanism

Linux's remote path is not "set runnable and hope the other CPU notices." The waker chooses/observes the target CPU while holding `p->pi_lock`, leaves the task in `TASK_WAKING`, and either:

- Directly locks the target runqueue and calls `ttwu_do_activate()`, or
- Queues `p->wake_entry.llist` through `__smp_call_single_queue()` so the target CPU drains it in `sched_ttwu_pending()`.

Source anchors:

- `core.c:3936-3942`: `__ttwu_queue_wakelist()` writes `rq->ttwu_pending` and queues the task's wake-entry llist to the target CPU.
- `core.c:3864-3898`: `sched_ttwu_pending()` takes the local `rq_lock_irqsave()`, updates the rq clock, then calls `ttwu_do_activate()` for each queued wake.
- `core.c:4038-4049`: `ttwu_queue()` first tries the wake-list path; if that path is not used, it takes `rq_lock()` and activates directly.
- `sched.h:2178-2183`: `WF_TTWU` marks wake balancing; `WF_MIGRATED` marks task migration during wake.

The runtime captures support that sequence:

- Function graph shows waker CPU calls `ttwu_queue_wakelist()`, then target CPU later runs `ttwu_do_activate()` and `enqueue_task_fair()`.
- `bpftrace-wake-ipi.txt` shows generic call-single activity during the workload: `@smp_call_single_queue: 132`, `@smp_call_single_interrupt: 131`, `@ipi_send_cpu: 103`, `@ipi_entry: 140`.
- `arch_smp_send_reschedule` is present on this kernel, but it is not the full remote wake-list IPI mechanism. The actual wake-list mechanism goes through `__smp_call_single_queue()`.

## C. bpftrace numbers under workload

Probe availability is recorded in `turn1-artifacts/linux-probe/available-wake-probes.txt`. This aarch64 kernel exposes `arch_smp_send_reschedule`, not `smp_send_reschedule`, and exposes scheduler-class enqueue probes (`enqueue_task_fair`, `enqueue_task_rt`, `enqueue_task_dl`, `enqueue_task_stop`) rather than a direct `enqueue_task` kprobe.

First broad counter run (`bpftrace-wake-counts.txt`, 5-second interval, four pipe-wakeup workers):

```text
@ttwu: 1918
@queue_remote: 1320
@activate: 1287
@enqueue: 2890
@new: 808
@ipi: 33
```

That raw `@enqueue` count intentionally does not match `@ttwu`: it includes `wake_up_new_task()` and unrelated scheduler-class enqueues. It is useful as a sanity check that enqueue activity is occurring, not as a one-to-one denominator for successful wakeups.

Second success-return run (`bpftrace-wake-success.txt`) separated `try_to_wake_up()` returns:

```text
@ttwu_entry: 10925
@ttwu_success: 8678
@ttwu_noop: 1932
@queue_remote: 8977
@activate: 8948
@enqueue_all: 12574
@new_task: 1795
@ipi: 227
```

This run was manually interrupted after the scripted interrupt failed to propagate through the `sudo bpftrace` wrapper, so it is not a clean interval denominator. It is still useful because `@activate` tracks the wake activity closely and `@enqueue_all` is again larger due non-TTWU enqueue sources.

Refined return-value run (`bpftrace-ttwu-queue-ret.txt`) disambiguated the `ttwu_queue_wakelist` probe:

```text
@ttwu: 1896
@ttwu_do_activate: 1211
@ttwu_queue_wakelist_entry: 1251
@ttwu_queue_wakelist_true: 47
@ttwu_queue_wakelist_false: 1123
@smp_call_single_queue: 113
@resched_ipi: 28
```

Conclusion from the numbers: raw `ttwu_queue_wakelist` entry count is not "remote queued wake count"; its return value matters. On this workload most wake-list checks returned false and Linux activated directly under an rq lock. The true remote wake-list path did execute, and the function-graph trace proves its ordering when it does.

Stack/latency run (`bpftrace-ttwu-stack-latency.txt`) captured common `try_to_wake_up()` callers and latency:

- Top caller in this workload: `child_wait_callback -> __wake_up_common -> __wake_up_parent -> do_notify_parent`, count 953.
- Other visible callers: softirq wakeups, workqueue `kick_pool`, timer work, block completion paths.
- Completed latency samples sum to 2274; most are under 32 us, with the largest displayed bucket `[32K, 64K)` ns at 15 samples.

## D. Breenix source preview

No Breenix code was edited this turn.

Current rescue mechanisms in `kernel/src/task/scheduler.rs`:

- `READY_THREAD_RESCUE_COUNT`: lines 130-141.
- Inline queue-empty rescue in `Scheduler::schedule_deferred_requeue()`: lines 1081-1153. This is the dispatch-time rescue that chooses a Ready thread when all runqueues are empty.
- Timer safety-net rescue `Scheduler::rescue_stuck_ready_threads()`: lines 2098-2203.
- Nonblocking wrapper `rescue_stuck_ready_threads_try()`: lines 2491-2505.

Current virtio-gpu wake delivery:

- `kernel/src/drivers/virtio/gpu_pci.rs:1792-1809`: MSI-X handler reads the used index and calls `GPU_COMPLETION.complete(...)` when it advances.
- `kernel/src/task/completion.rs:478-496`: `complete()` stores the done token, executes `sev` on aarch64, loads the waiter, and calls `scheduler::isr_unblock_for_io(tid)`.
- `kernel/src/task/scheduler.rs:2700-2719`: `isr_unblock_for_io()` pushes the TID into a per-CPU lock-free ISR wakeup buffer and sets `need_resched`; it does not mutate thread state or enqueue.
- `kernel/src/task/scheduler.rs:1002-1013`: `schedule_deferred_requeue()` drains all ISR wakeup buffers under the scheduler lock and calls `unblock_for_io(tid)`.

Breenix Ready-state transitions preview:

- `ThreadState::Ready` is documented in `kernel/src/task/thread.rs:29-33` as "ready to run and in scheduler queue."
- `Thread::set_ready()` at `kernel/src/task/thread.rs:881-885` only mutates state.
- New thread constructors initialize threads as `Ready`; `Scheduler::add_thread_inner()` then pushes them to a per-CPU queue under the scheduler lock (`scheduler.rs:645-656`).
- `Scheduler::schedule_deferred_requeue()` sets the outgoing running thread to Ready at `scheduler.rs:1034`, but intentionally defers queue insertion until after context save.
- `Scheduler::unblock()` sets Ready at `scheduler.rs:1329`, then may skip queue insertion if the thread is current on any CPU or in deferred requeue (`scheduler.rs:1337-1365`).
- `Scheduler::wake_io_thread_locked()` sets Ready at `scheduler.rs:1804-1806`, then may skip queue insertion if the thread is current or in deferred requeue (`scheduler.rs:1819-1842`).
- Timer wake sets Ready before queueing at `scheduler.rs:1933-1940` and `scheduler.rs:1955-1970`.
- `requeue_thread_after_save()` is the later repair/completion path for deferred context-save windows (`scheduler.rs:1264-1301`).

Preview divergence from Linux: Breenix often publishes `ThreadState::Ready` before it knows the thread has either been enqueued or is still protected by a current/deferred handoff. Linux publishes `TASK_WAKING` in that interval and reserves `TASK_RUNNING` for tasks already queued or activated under the destination rq lock.

## E. Hypotheses for Turn 2

Ranked hypotheses:

1. Missing `Waking`/atomic activation state in Breenix wake paths. `wake_io_thread_locked()`, `unblock()`, and timer wake publish `Ready` before the enqueue decision is complete. If the "current/deferred" handoff path fails to requeue, the thread is observable as Ready but not on any per-CPU queue. This directly matches the rescue predicate and is the strongest Linux divergence.

2. ISR wake buffering is not equivalent to Linux's per-target wake-list. `isr_unblock_for_io()` records the wake on the interrupting CPU's buffer and relies on a future scheduler pass to drain all buffers. Linux queues to the selected target CPU while the task is `TASK_WAKING`, and the target CPU activates under its own rq lock. Turn 2 should map whether Breenix can drain an ISR wake on one CPU, mark the waiter Ready, then miss the target queue/IPI handoff.

3. Deferred requeue publication order may be too weak. The context-switch path deliberately keeps the old thread current until context save, then updates `current_thread`, sets `previous_thread`, and stores the deferred requeue slot. The source comments already identify windows around `previous_thread`/deferred publication. Turn 2 should instrument those windows with memory-only counters, especially for tid=13.

4. Timer and generic unblock paths share the same Ready-before-enqueue shape. Even though virtio-gpu uses `complete()` -> `isr_unblock_for_io()`, Turn 2 should include all Ready transitions in the map so the eventual fix is architectural, not just a virtio-gpu special case.

Recommended Turn 2 focus: build a source-level wake-state/enqueue map for Breenix and add minimal memory-only attribution counters around `wake_io_thread_locked()`, ISR buffer drain, deferred requeue, and queue insertion. The counters should distinguish "Ready published then queued", "Ready published but skipped due current/deferred", and "later requeue completed", without adding logging to hot paths.
