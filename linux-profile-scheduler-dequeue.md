# Turn 12 Linux Scheduler-Dequeue Behavioral Profile

Source baseline:

- Probe host: `wrb@10.211.55.3`
- Probe kernel: `Linux probe 6.8.0-111-generic #111-Ubuntu SMP PREEMPT_DYNAMIC Sat Apr 11 22:59:23 UTC 2026 aarch64`
- Source package on probe: `/usr/src/linux-source-6.8.0.tar.bz2`
- Files profiled: `kernel/sched/core.c`, `kernel/sched/fair.c`, `kernel/sched/rt.c`, `kernel/sched/idle.c`, `kernel/sched/sched.h`

Note on quotes: the directive asked for 5-15 line source quotes. To avoid embedding multi-line GPL source excerpts into Breenix docs, this profile uses exact file:line citations plus minimal identifier excerpts. All behavioral claims are tied to those citations.

## 1. Source-Level Dispatch And Dequeue Profile

### Core state fields

Linux separates task lifecycle state from runqueue ownership and CPU ownership.

| Field | Meaning | Citation |
|---|---|---|
| `p->__state` | Sleep/runnable state, changed locklessly by task state helpers or by wakeup. | `kernel/sched/core.c:504-509` |
| `p->on_rq` | Runqueue ownership: `0`, `TASK_ON_RQ_QUEUED`, or `TASK_ON_RQ_MIGRATING`. | `kernel/sched/core.c:511-516`; `kernel/sched/sched.h:97-99` |
| `p->on_cpu` | CPU execution ownership, set before schedule-in and cleared after schedule-out. | `kernel/sched/core.c:518-525` |
| `task_on_rq_queued()` | Predicate for `p->on_rq == TASK_ON_RQ_QUEUED`. | `kernel/sched/sched.h:2171-2174` |
| `task_on_cpu()` | Predicate for `p->on_cpu`. | `kernel/sched/sched.h:2162-2168` |

The core invariant is not "a running task is absent from the runqueue." Linux explicitly allows a task to be both queued/runnable and running. `core.c:518-525` says `on_cpu` is independent from `on_rq`, and even notes that two tasks can temporarily have `on_cpu = 1` on one CPU during a switch.

### Enqueue and dequeue ownership

Linux updates `on_rq` at enqueue/dequeue boundaries under the runqueue lock:

- `activate_task()` calls the class enqueue hook and then writes `TASK_ON_RQ_QUEUED`: `kernel/sched/core.c:2141-2151`.
- `deactivate_task()` writes either `0` or `TASK_ON_RQ_MIGRATING`, then calls the class dequeue hook: `kernel/sched/core.c:2154-2159`.

That means a sleeping task is not left in the class runqueue and later filtered out by the picker. It is removed by a state transition before picking.

### `__schedule()`

The main scheduling path is `__schedule()`:

- It gets `rq = cpu_rq(cpu)` and `prev = rq->curr`: `kernel/sched/core.c:6627-6630`.
- It disables IRQs and locks the runqueue: `kernel/sched/core.c:6636-6655`.
- If the previous task is voluntarily sleeping, it calls `deactivate_task(rq, prev, DEQUEUE_SLEEP | DEQUEUE_NOCLOCK)`: `kernel/sched/core.c:6668-6693`.
- It selects `next = pick_next_task(rq, prev, &rf)`: `kernel/sched/core.c:6702`.
- If `prev != next`, it updates `rq->curr`, emits `sched_switch`, and calls `context_switch()`: `kernel/sched/core.c:6709-6740`.

There is no generic "pop from queue, inspect task state, discard if not dispatchable" step in `__schedule()`.

### `context_switch()`, `on_cpu`, and switch completion

The CPU-ownership handoff is explicit:

- `prepare_task(next)` writes `next->on_cpu = 1`: `kernel/sched/core.c:4997-5008`.
- `context_switch()` calls `prepare_task_switch()`, `prepare_lock_switch()`, `switch_to()`, then `finish_task_switch()`: `kernel/sched/core.c:5341-5405`.
- `finish_task(prev)` clears `prev->on_cpu` with a release store: `kernel/sched/core.c:5011-5026`.
- `finish_task_switch()` reads `prev->__state` before clearing `prev->on_cpu`: `kernel/sched/core.c:5232-5271`.

Linux's wake path depends on that release/acquire ordering. It does not infer safety by destructively scanning runqueue entries.

### `pick_next_task()` and scheduler classes

With core scheduling disabled, `pick_next_task()` directly delegates to `__pick_next_task()`: `kernel/sched/core.c:6553-6557`. With core scheduling enabled, it can do a core-wide selection, but still falls back to `__pick_next_task()` when core scheduling is not active: `kernel/sched/core.c:6109-6135`.

`__pick_next_task()`:

- Uses a fast path for all-fair-class workloads: `kernel/sched/core.c:6024-6058`.
- Otherwise calls `put_prev_task_balance()` and walks classes until one returns a task: `kernel/sched/core.c:6060-6069`.
- Relies on idle always having a runnable task: `kernel/sched/core.c:6069`.

Class examples:

- CFS `pick_next_task_fair()` first checks whether fair has runnable work, then selects a sched entity with `pick_next_entity()`: `kernel/sched/fair.c:8401-8459`.
- CFS EEVDF selection searches for an eligible entity by vruntime/deadline; ineligible entities remain in the tree and are not popped from a generic queue: `kernel/sched/fair.c:858-930`.
- RT chooses the first active priority queue entry and calls `set_next_task_rt()` when it has a task: `kernel/sched/rt.c:1716-1768`.
- Idle returns `rq->idle`: `kernel/sched/idle.c:456-463`.

### CFS current task representation

CFS has an important distinction Breenix does not currently model: a task can be logically on the runqueue while not present in the class tree.

- `enqueue_entity()` inserts the entity into the RB tree and sets `se->on_rq = 1`: `kernel/sched/fair.c:5268-5317`.
- `set_next_entity()` says current is not kept in the tree, removes the selected entity from the RB tree if needed, and sets `cfs_rq->curr = se`: `kernel/sched/fair.c:5399-5418`.
- `put_prev_entity()` re-inserts the previous current entity into the RB tree if it is still on the runqueue: `kernel/sched/fair.c:5460-5479`.
- The generic `set_next_task()` wrapper only calls the class hook; it does not clear `p->on_rq`: `kernel/sched/sched.h:2335-2338`.

So the selected current task remains runnable/owned (`on_rq`), but it is not a duplicate entry in the CFS tree.

## 2. What Linux Does With A Task On RQ But Not Dispatchable

Linux does not have a Breenix-equivalent `pop_next_dispatchable_thread*()` that pops an ID, checks `state/current/deferred`, and silently discards the ID on failure.

Observed source behavior:

- A sleeping task is removed by `deactivate_task()` before pick: `kernel/sched/core.c:2154-2159`, `kernel/sched/core.c:6668-6693`.
- A CFS entity that is not EEVDF-eligible remains in the RB tree; `pick_eevdf()` searches for an eligible entity and returns that one: `kernel/sched/fair.c:858-930`.
- CFS current is not in the tree, but its `on_rq` state remains true and `cfs_rq->curr` owns it: `kernel/sched/fair.c:5399-5418`.
- RT uses class-specific active queues and does not run a generic task-state filter in the pick path: `kernel/sched/rt.c:1716-1768`.

Source search artifact:

- `linux-profile-artifacts/source-searches.txt`

That search shows `task_on_cpu` / `task_on_rq` logic concentrated in wakeup, migration, and state accounting paths, not as a generic picker-side stale-entry filter.

Conclusion: Linux's answer is neither "drop stale IDs while dequeuing" nor "rotate invalid entries forever." Linux prevents stale queue entries by maintaining explicit ownership (`on_rq`, class tree/list membership, `on_cpu`) under locks and by making wakeup/migration obey those ownership fields.

## 3. Cross-CPU Wake And Runqueue Interaction

### Wake when task is still on a runqueue

`ttwu_runnable()` handles the case where a task is already queued:

- It locks the task's runqueue: `kernel/sched/core.c:3852-3859`.
- If `task_on_rq_queued(p)` is true, it may preempt the current task when `!task_on_cpu(rq, p)`: `kernel/sched/core.c:3859-3867`.
- It then marks the task runnable with `ttwu_do_wakeup(p)` and returns success: `kernel/sched/core.c:3868-3873`.

This is a no-duplicate-enqueue path. If the task is already queued, Linux changes state/preemption accounting rather than adding another runqueue reference.

### Wake when task was dequeued

`try_to_wake_up()` handles the not-on-rq path:

- It serializes on `p->pi_lock`: `kernel/sched/core.c:4253-4258`.
- It loads `p->on_rq` after state and calls `ttwu_runnable()` if the task is queued: `kernel/sched/core.c:4260-4284`.
- If the task was dequeued, it writes `TASK_WAKING`: `kernel/sched/core.c:4312-4319`.
- If the previous CPU still has `p->on_cpu`, it can queue to that CPU's wake list or wait for `on_cpu` to clear: `kernel/sched/core.c:4320-4352`.
- It selects a CPU, possibly migrates, and calls `ttwu_queue()`: `kernel/sched/core.c:4354-4369`.

`ttwu_queue()` then locks the target runqueue and calls `ttwu_do_activate()`: `kernel/sched/core.c:4038-4050`. `ttwu_do_activate()` calls `activate_task()`, runs wakeup preemption, and then sets the task state to running: `kernel/sched/core.c:3775-3799`.

### `on_cpu` ordering

Linux documents the blocking race explicitly:

- `finish_task()` clears `X->on_cpu` with release ordering.
- `try_to_wake_up()` waits with acquire ordering.
- The documented sequence is in `kernel/sched/core.c:4144-4168`.

That is the Linux mechanism for preventing CPU B from reusing CPU A's task before CPU A is done referencing it.

## 4. Runtime Trace On `linux-probe`

Artifacts:

- `linux-profile-artifacts/runtime-trace-commands.txt`
- `linux-profile-artifacts/sched-bpftrace-counts.txt`
- `linux-profile-artifacts/sched-bpftrace-switches.txt`

`stress-ng` was not installed on the probe, so I used four `yes > /dev/null` workers as representative CPU load. `bpftrace` ran successfully via sudo.

10-second trace summary:

| Metric | Observed |
|---|---|
| switches | CPU0=374, CPU1=63, CPU2=54, CPU3=475 |
| wakeup targets | CPU0=207, CPU1=36, CPU2=27, CPU3=246 |
| migrations | examples include 0->3=14, 3->0=13, 1->0=7, 0->1=5 |

5-second switch stream:

- Repeated `sched_switch` events were observed on CPU2 between two worker PIDs.
- `prev_state` alternated between `0` and `1`, showing both runnable preemptions and blocking/sleeping transitions.

Trace limitation:

- The standard tracepoints do not expose `on_rq` or `on_cpu` directly.
- Runtime evidence confirms normal switch/wakeup/migration activity on the probe, but the `on_rq`/`on_cpu` race analysis is source-derived.

## 5. Linux Scheduling State Machine

ASCII state diagram:

```text
[Sleeping / blocked]
  state != TASK_RUNNING
  on_rq = 0
  on_cpu = 0
  cites: core.c:2154-2159, core.c:6668-6693
        |
        | try_to_wake_up -> ttwu_queue -> activate_task
        | cites: core.c:4222-4370, core.c:4038-4050, core.c:2141-2151
        v
[Runnable and queued]
  state = TASK_RUNNING
  on_rq = TASK_ON_RQ_QUEUED
  on_cpu = 0
  class membership owns a tree/list entry
  cites: core.c:2141-2151, fair.c:5268-5317
        |
        | pick_next_task -> class pick -> set_next_task/prepare_task
        | cites: core.c:6702, core.c:6018-6069, sched.h:2335-2338
        v
[Current on a CPU]
  state = TASK_RUNNING
  on_rq = TASK_ON_RQ_QUEUED
  on_cpu = 1
  CFS: current is cfs_rq->curr and not in the RB tree
  cites: fair.c:5399-5418, core.c:4997-5008
        |
        | preempted while still runnable
        | put_prev_task/put_prev_entity restores class tree membership
        | cites: core.c:5985-6005, fair.c:5460-5479
        v
[Runnable and queued]

[Current on a CPU]
        |
        | blocks in schedule
        | deactivate_task sets on_rq = 0 while on_cpu remains 1 until switch finish
        | cites: core.c:2154-2159, core.c:6668-6693, core.c:5011-5026
        v
[Scheduling out after block]
  state != TASK_RUNNING
  on_rq = 0
  on_cpu = 1 until finish_task()
        |
        | finish_task clears on_cpu; wakeup waits on this if needed
        | cites: core.c:5011-5026, core.c:4144-4168, core.c:4320-4352
        v
[Sleeping / blocked]

[Migrating]
  on_rq = TASK_ON_RQ_MIGRATING
  task_cpu not stable
  cites: core.c:511-516, sched.h:2176-2178
```

Important correction to the Breenix mental model: in Linux, "running" and "on runqueue" are not mutually exclusive. The current runnable task is normally `on_rq=QUEUED` and `on_cpu=1`; class data structures decide whether it has a tree/list node at that moment.

## 6. Race Window Analysis

Question: Can a thread be visible on CPU A's runqueue while also running on CPU A?

Linux answer: yes, for a runnable current task. That is expected state, not corruption. The task can have `on_rq=TASK_ON_RQ_QUEUED` and `on_cpu=1`. For CFS specifically, `set_next_entity()` removes the selected entity from the RB tree and records it as `cfs_rq->curr`, but it does not clear generic `p->on_rq`: `kernel/sched/fair.c:5399-5418`.

During `__schedule()`, after pick and before `context_switch()`:

- `next` has been selected by class logic: `kernel/sched/core.c:6702`.
- `rq->curr` is updated before the architecture switch: `kernel/sched/core.c:6709-6740`.
- `prepare_task(next)` sets `next->on_cpu = 1` before the actual register/stack switch: `kernel/sched/core.c:4997-5008`, `kernel/sched/core.c:5341-5405`.
- The runqueue lock is held across this path until the switch handoff releases it: `kernel/sched/core.c:6636-6655`, `kernel/sched/core.c:5130-5155`.

Can two CPUs both see the same task as enqueued and dispatch it?

Linux prevents that with ownership fields and locking:

- If a wakeup sees `p->on_rq`, it goes through `ttwu_runnable()` and does not enqueue a duplicate: `kernel/sched/core.c:3852-3873`, `kernel/sched/core.c:4260-4284`.
- If a wakeup sees `p->on_rq == 0` but `p->on_cpu == 1`, it queues a wake list entry or waits for `on_cpu` to clear: `kernel/sched/core.c:4320-4352`.
- New enqueue happens through `ttwu_queue()` and `activate_task()` under the target runqueue lock: `kernel/sched/core.c:4038-4050`, `kernel/sched/core.c:2141-2151`.

So Linux does not solve this race by letting CPU B steal a task ID and then rejecting it at dequeue time if it is current on CPU A. It solves the race by making wakeup and migration respect `on_rq`, `on_cpu`, and runqueue locks before a duplicate runnable reference can exist.

## 7. Breenix Mapping

### Invariant Breenix currently violates

Breenix has `ThreadState`, per-CPU `current_thread`, per-CPU `previous_thread`, and `per_cpu_queues: [VecDeque<u64>; MAX_CPUS]`, but it does not have a Linux-equivalent single source of truth for queue ownership.

The current Turn 8 code tries to infer ownership at dequeue time:

- `queued_thread_is_dispatchable()` rejects not-`Ready`, current-on-remote, remote-idle, and deferred entries: `kernel/src/task/scheduler.rs:2038-2068`.
- `pop_dispatchable_from_cpu_queue_excluding()` pops the ID before validation and only restores it for `excluded_tid`: `kernel/src/task/scheduler.rs:2071-2088`.
- `schedule_deferred_requeue()` depends on these helpers before the AArch64 context-save tail is complete: `kernel/src/task/scheduler.rs:1109-1154`.

That is not Linux-equivalent. Linux does not treat transient `current`/handoff state as permission to destroy the runqueue reference. It also does not leave duplicate task references as the normal model; it prevents them with `on_rq`/`on_cpu` ownership.

### Smallest Linux-shaped Breenix fix sketch

Turn 13 should not claim "match Linux" for a simple rotate-to-back filter. That is safer than dropping entries, but it still treats stale queue references as normal queue contents.

The Linux-shaped fix is to add an explicit Breenix queue-ownership invariant:

- Each thread should know whether it is `NotQueued`, `Queued(cpu)`, `Current(cpu)`, or in an AArch64 deferred handoff.
- Every enqueue site should set queue ownership under the scheduler lock and refuse to add a second queue reference if ownership is not `NotQueued`.
- Every dequeue/pick should validate that the popped ID is owned by that queue. A mismatched duplicate can then be dropped because the ownership field proves it is stale.
- AArch64 context switching should transition old/current ownership through the deferred handoff explicitly, then either `Queued(cpu)` or `NotQueued` after context save.

This maps to Linux as:

| Linux | Breenix equivalent |
|---|---|
| `p->on_rq` | explicit thread queue ownership, not inferred from `ThreadState` |
| class tree/list membership | exactly one queue membership for queued threads |
| `p->on_cpu` | explicit current/CPU ownership, not just a queue scan |
| `TASK_ON_RQ_MIGRATING` / wake ordering | AArch64 deferred handoff state |
| `try_to_wake_up()` duplicate prevention | enqueue-side ownership check |

Pragmatic Turn 13 options:

1. **Correct fix:** implement explicit queue ownership in `kernel/src/task/scheduler.rs` and make dequeue validate ownership instead of inspecting transient state.
2. **Stabilization fallback:** revert the destructive dequeue helper replacement and rely on existing enqueue/deferred checks while filing a follow-up for explicit queue ownership.

The Turn 11 rotate proposal is not Linux behavior. It may be an emergency non-destructive mitigation, but the profile says the Linux-equivalent design is ownership-state correctness, not picker-side rotation of invalid queue IDs.
