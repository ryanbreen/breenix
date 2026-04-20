# F32r Per-CPU Reschedule Design

Date: 2026-04-20

## Goal

Retire the global `NEED_RESCHED` scheduling signal and replace it with a
targeted `resched_cpu(target)` primitive. A wake site must mark the CPU that
owns or receives the runnable work. Idle and interrupt-return paths must only
consume their own CPU's reschedule state.

This matches Linux's scheduler shape:

- `resched_curr(rq)` is the sole primitive that marks a runqueue's current task
  for reschedule (`/tmp/linux-v6.8/kernel/sched/core.c:1035-1063`).
- `resched_cpu(int cpu)` is the explicit CPU-targeted helper
  (`/tmp/linux-v6.8/kernel/sched/core.c:1065-1073`).
- arm64 need-resched state is per task/thread-info, not global
  (`/tmp/linux-v6.8/arch/arm64/include/asm/thread_info.h:24-46`,
  `:60-86`).
- `try_to_wake_up()` determines a target CPU before queuing the wake
  (`/tmp/linux-v6.8/kernel/sched/core.c:4222-4370`).

## Target Architecture

### 1. Add `resched_cpu(target: u8)`

Add a scheduler-owned primitive in `kernel/src/task/scheduler.rs`:

```rust
#[cfg(target_arch = "aarch64")]
pub fn resched_cpu(target: u8) {
    // validate target is online and < MAX_CPUS
    // set target CPU's per-CPU need_resched flag
    // if target != current CPU, send SGI_RESCHEDULE to target
}
```

Required behavior:

- The target CPU's per-CPU `need_resched` bit is set before the IPI is sent.
- If `target == current_cpu`, only the local per-CPU bit is set.
- If `target != current_cpu`, the target bit is set and `SGI_RESCHEDULE` is
  sent to that CPU.
- No global boolean is set, read, or cleared by this primitive.

Linux parity:

- Linux's same-CPU branch sets the current task's need-resched bit and returns
  without an IPI (`core.c:1051-1056`).
- Linux's remote branch sets the target task's need-resched bit and sends a
  reschedule IPI (`core.c:1059-1060`).
- Linux's exported CPU helper takes an explicit `cpu` argument and calls
  `resched_curr()` for that CPU's runqueue (`core.c:1065-1073`).

Implementation detail:

Current aarch64 per-CPU storage stores `need_resched` as a plain `u8` inside
`ALL_CPU_DATA` (`kernel/src/per_cpu_aarch64.rs:16-32`, `:93-104`), and the
public setter only writes the current CPU through `TPIDR_EL1`
(`kernel/src/per_cpu_aarch64.rs:221-228`). F32r should add a small aarch64
helper for remote target writes, for example
`per_cpu_aarch64::set_need_resched_for_cpu(cpu, true)`, with release ordering
or an atomic byte. That helper must be the only remote writer. It should be
documented as scheduler-owned so random subsystems do not mutate remote
per-CPU state directly.

The receiving SGI handler at `kernel/src/arch_impl/aarch64/exception.rs:1243-1248`
can keep setting the local bit during the migration as an idempotent backstop,
but the correctness contract should be that the waker marks the target before
IPI, just like Linux's `set_nr_and_not_polling()` happens before
`smp_send_reschedule(cpu)` (`core.c:900-910`, `:1059-1060`).

### 2. Convert Wake Sites To Return Or Carry Target CPU

Every wake path must call `resched_cpu(target)` with the CPU selected by the
enqueue/state transition, while still holding the scheduler lock when the
target is derived.

Rules:

- If a thread is enqueued onto `per_cpu_queues[target]`, call
  `resched_cpu(target)`.
- If the thread is still current on a CPU and must observe its state change on
  interrupt/syscall return, call `resched_cpu(owner_cpu)`.
- If a wake drains multiple waiter TIDs, loop per waiter and target each one.
- If a wake is process-level, resolve the affected thread IDs first and then
  use the same per-thread target path.
- If a thread is not yet assigned, spawning must choose a CPU before publishing
  the runnable state.

Linux parity:

- Linux handles the "waking current" special case without a runqueue lock when
  the target is current (`core.c:4227-4244`).
- Linux waits for `on_cpu` and uses `task_cpu(p)` or `select_task_rq()` before
  queueing (`core.c:4339-4364`).
- Linux calls `ttwu_queue(p, cpu, wake_flags)` only after a CPU is known
  (`core.c:4369`).
- Linux's remote wake-list path is per target CPU, not a broadcast
  (`core.c:3930-4049`).

Concrete Breenix API changes:

- Change `Scheduler::add_thread()` and `Scheduler::add_thread_front()` to
  return the selected CPU from `add_thread_inner()`; the target is already
  computed at `kernel/src/task/scheduler.rs:581-587`.
- Change `Scheduler::unblock()`, `unblock_for_signal()`,
  `unblock_for_child_exit()`, and `unblock_for_io()` to return a wake result
  that includes either `enqueued_target` or `current_cpu`. `wake_io_thread_locked()`
  already has this shape through `IoWakeResult` (`scheduler.rs:170-179`,
  `:1728-1770`).
- Replace `send_resched_ipi()` broadcasts with `resched_cpu(target)` at sites
  that already have a target local variable.
- Keep `send_resched_ipi_to_cpu()` only as a lower-level architecture helper
  or fold it into `resched_cpu()`.

### 3. Idle Gate Reads Per-CPU Only

After every wake/spawn path targets a CPU, change the aarch64 idle gate to read
only local per-CPU state:

- `idle_gate_state()` in `kernel/src/arch_impl/aarch64/context_switch.rs:3233-3242`
  should not call `scheduler::is_need_resched()` while that helper still ORs in
  the global bit.
- The final shape should be a local read equivalent to
  `per_cpu_aarch64::need_resched()` plus the existing ISR wake-buffer depth
  check.

Linux parity:

- Linux's idle loop checks `need_resched()` for the current task/CPU, not a
  machine-global pending bit.
- The F32i Linux audit records the generic idle loop's `!need_resched()` sleep
  gate and ordering requirements in `docs/planning/f32i-cpu0-wfi-wake/linux-audit.md`.

This step must happen only after wake sites have target coverage. F32q proved
that switching the idle gate first loses cross-CPU wake signals because the
global flag was still carrying wake information.

### 4. Retire Global `NEED_RESCHED`

Migration stages:

1. Add targeted API and keep all current behavior.
2. Convert call sites to target per-CPU resched while temporarily leaving the
   global `NEED_RESCHED.store(true)` in place as a migration cross-check.
3. Switch the idle gate to local per-CPU reads after targeted coverage exists.
4. Remove the redundant global stores from converted sites.
5. Delete the global atomic and update helpers so `set_need_resched()` is
   either removed or becomes a current-CPU-only wrapper.

The temporary global must not be used as a fallback after Phase 3c. If retained
for diagnostics, it should become a counter or assertion-only instrument that
is never consulted by idle or interrupt-return scheduling decisions.

Linux parity:

- There is no Linux global `need_resched`; the flag is part of the current
  task/thread-info (`thread_info.h:24-46`, `:60-86`).
- Linux clears need-resched as part of scheduling the previous task
  (`core.c:6691-6693`), which corresponds to clearing the local target's flag
  when that CPU reaches the scheduler.

## Conversion Order

F32o's diagnosis is not present in this checkout at the prompt path
`docs/planning/f32o-perf/diagnosis.md`, so the ordering below uses the prompt's
reported symptom (idle gate bypassing WFI 13.6M times/120s) and the audit's
call-site traffic expectations.

1. `resched_cpu(target)` API, no consumers.
2. Highest-risk/highest-traffic scheduler internals:
   - `wake_io_thread_locked()` / `unblock_for_io()`
     (`kernel/src/task/scheduler.rs:1705-1770`)
   - `spawn()` / `spawn_front()` (`scheduler.rs:2212-2255`)
   - timer wake enqueue in `wake_expired_timers()` (`scheduler.rs:1840-1897`)
3. Device and UI wake sites:
   - GPU completion (`kernel/src/drivers/virtio/gpu_pci.rs:1488-1494`)
   - TTY/PTY/stdin waiters (`kernel/src/ipc/stdin.rs`, `kernel/src/tty/*`)
4. Network waiters:
   - UDP/TCP waiter loops (`kernel/src/socket/udp.rs:132-139`,
     `kernel/src/net/tcp.rs:1558-1588`)
5. Current-CPU-only paths:
   - yield, exit, signal termination, fault termination, context-switch retry,
     and timer quantum expiry.
6. Process-level signal paths:
   - `kernel/src/syscall/signal.rs:158-206` must resolve concrete thread IDs
     instead of relying on a process-level global resched.
7. Idle gate local-read switch.
8. Global store removal and final global deletion.

## Validation Plan

Every implementation sub-phase must pass:

- x86_64 clean build:
  `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- aarch64 clean build:
  `cargo build --release --features testing,external_test_bins --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `wait_stress` for 60 seconds with zero stalls.
- Short Parallels boot after each wake-site batch.

Final validation:

- `wait_stress` 60s: 0 stalls.
- 5 consecutive 120s Parallels boots: bsshd, bounce, frames, strict render,
  no AHCI timeout.
- Host CPU drops meaningfully from the 800% baseline, with target below 300%
  on idle boot running bsshd + bounce + BWM and no user input.
- Rendering remains intact.
- Bounce FPS is at least 160.

Before and after CPU measurements should be recorded in the final `exit.md`
with the exact command, duration, and observed values.

## Risks And Required Invariants

### Stale Wake Target

Risk: a thread can move between CPUs after a wake site computes a target.

Invariant: target derivation, state transition, and enqueue must happen under
the scheduler lock. If the thread is enqueued to `per_cpu_queues[target]`, then
`resched_cpu(target)` is correct even if a later load-balancing operation moves
it; the mover becomes responsible for rescheduling the new target.

Linux parity: `try_to_wake_up()` serializes state and CPU selection with
`p->pi_lock`, `on_cpu` ordering, and `set_task_cpu()` before `ttwu_queue()`
(`core.c:4253-4369`).

### Current-On-CPU Wake

Risk: a thread blocked in a syscall may still be the current thread on a CPU,
so enqueuing it elsewhere double-schedules the same kernel stack.

Invariant: if `cpu_state[cpu].current_thread == tid`, do not enqueue it; mark
that owning CPU with `resched_cpu(cpu)` so it observes the state change.

Linux parity: `try_to_wake_up()` treats waking current specially and reasons
about `on_cpu` before queueing (`core.c:4227-4244`, `:4339-4352`).

### Multi-Waiter Broadcast Replacement

Risk: stdin, PTY, TCP, UDP, and signal wait queues can wake multiple threads
that belong to different CPUs.

Invariant: replace broadcast with a targeted per-waiter loop. The loop may
send multiple IPIs, one per distinct target, or coalesce targets into a CPU
mask, but it must never set a global fallback bit.

Linux parity: waitqueue wake eventually calls `try_to_wake_up()` per task; the
target CPU is task-specific (`core.c:4222-4370`).

### Remote Per-CPU Storage Safety

Risk: a plain `u8` remote write can become a data race or lack ordering.

Invariant: remote target marking must use an atomic representation or a small
unsafe helper with explicit release/acquire semantics and no aliasing through
ordinary mutable references. The local read/clear path must pair with that
ordering.

Linux parity: Linux atomically ORs `_TIF_NEED_RESCHED` into the target
thread-info flags before deciding whether to send an IPI (`core.c:900-910`).

### CPU0 / Parallels SGI Behavior

Risk: prior work observed CPU0-specific SGI admission problems. A pure
reschedule SGI may not be enough if the target CPU never acknowledges it.

Invariant: this migration must preserve F32c-F32k behavior and keep the F32i
larger endpoint in view: per-target wake-list delivery may still be required
for CPU0 parity. F32r should not reintroduce broadcast fallback to hide a
missed targeted wake.

Linux parity: the larger endpoint is Linux's `ttwu_queue_wakelist()` path,
which queues remote wake work to the target CPU and wakes it through an IPI
(`core.c:3930-4049`).

### Hot Path Cleanliness

Risk: debugging this migration by adding serial breadcrumbs to interrupt,
syscall, or idle paths would change timing and repeat earlier failures.

Invariant: no logging or formatting in Tier 1 paths, IRQ paths, syscall entry,
or idle loops. Use existing trace framework, post-mortem logs, GDB, and QEMU
interrupt tracing.

## Phase Commit Map

| Phase | Commit intent | Behavior change? | Validation |
| --- | --- | --- | --- |
| 1 | `docs(kernel): F32r per-CPU resched audit` | No | x86_64 + aarch64 clean builds |
| 2 | `docs(kernel): F32r per-CPU resched design` | No | x86_64 + aarch64 clean builds |
| 3a | Add `resched_cpu(target)` and remote per-CPU setter, no consumers | No intended behavior change | x86_64 + aarch64 clean builds |
| 3b | Convert wake sites one batch at a time, keep global cross-check | Yes, target per-CPU signals added | wait_stress + short Parallels boot per batch |
| 3c | Idle gate reads local per-CPU only | Yes, removes global idle wake source | wait_stress + short Parallels boot |
| 3d | Remove redundant global stores from converted sites | Yes, global fallback no longer present | wait_stress + Parallels boot after each safe batch |
| 3e | Delete global atomic/helper fallback | Yes, final architecture | full final gate |

## Stop Conditions

Stop and write `exit.md` instead of continuing if:

- A wake site cannot derive a target CPU from thread/runqueue state.
- A targeted conversion loses a wake, stalls rendering, or regresses
  `wait_stress`.
- The only apparent fix is to keep or reintroduce global resched state.
- Validation requires Tier 1 edits or logging in IRQ/syscall/idle paths.
