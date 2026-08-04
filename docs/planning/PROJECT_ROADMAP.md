# Breenix OS Project Roadmap

This is the master project roadmap for Breenix OS: current development status,
completed phases, in-progress work, and planned work. Update after each PR
merge and when starting new work.

Detailed, per-issue tracking lives in GitHub Issues (`gh issue list` /
`gh issue create` in `ryanbreen/breenix`), not here — this file stays a
short pointer to what's current. Live turn-by-turn ARM64/Parallels work
log: `docs/planning/ralph-roadmap.html`.

## Current Development Status

Focus is ARM64/Parallels: teardown/process-lifecycle correctness, SMP
scheduling, and the userland/POSIX compliance stack (dashboard:
https://v0-breenix-dashboard.vercel.app/).

## Recently Completed

- ✅ **Teardown closure: fault-driven exit path quiescence** ([PR #418](https://github.com/ryanbreen/breenix/pull/418), Aug 2026)
  - Follow-through on [PR #417](https://github.com/ryanbreen/breenix/pull/417)'s normal-exit teardown quiescence, extended to the **fault-driven exit path** (SIGSEGV/SIGBUS taken from an aarch64 EL0 synchronous exception), which #417 didn't cover.
  - `quiesce_ttbr0_for_exit()` installed at all 4 EL0 fault sites (data abort, instruction abort, both fault-deferral drain sites), mirroring #417's normal-exit machinery.
  - Idempotent teardown at both entry points (`ProcessManager::exit_process`, `handle_thread_exit`): a narrow `already_terminated` guard skips the second `cleanup_cow_frames()` walk without skipping reparent/SIGCHLD/ready-queue bookkeeping, preserving the single-CoW-decref invariant across sigkill-then-exit, sigkill-then-fault, fault-only, and plain-exit orderings.
  - Single-pid victim quarantine: `Scheduler::terminate_process_threads(pid)` marks and drains every scheduler-owned thread for the faulting process before resource handoff, at all 4 fault sites.
  - Fault exits now always defer the address-space free through the two-epoch grace (no liveness gate); reclaim order in `Scheduler` is grace-first/liveness-last; a new drain site in `schedule_from_kernel` (alongside the existing `sys_fork` drain) lets fault exits reclaim without waiting on a later fork.
  - Plus three standalone correctness fixes: `decode_last_dispatched()` used in anomaly postmortems (was reading a packed tid-and-slot word); kernel-stack liveness sampling reordered to check grace-elapsed first; PID-1 init-child retention on exit made explicit.
  - **Deliberately out of scope** (attempted, found to introduce new blocking defects, stripped back out before merge — see PR body): a `CLONE_VM` thread-group-wide teardown sweep, and a PID-1 "kill init" panic guard. Both tracked as follow-ups below.
  - The full grave/reclaimer structural rewrite (a proper deferred-reclaim subsystem, replacing these two-epoch-grace point-fixes) stays **parked** on branch `fix/teardown-grave`; design record and spec live at `docs/planning/teardown-lifecycle/` on that branch.

## Planned Follow-ups

- 📋 **CLONE_VM group seal + exec-time `thread_group_id`/`inherited_cr3` detach** — [#471](https://github.com/ryanbreen/breenix/issues/471)
- 📋 **`Thread::clone` kernel-stack ownership transfer** (mirror `fork`'s `kernel_stack_allocation` transfer) — [#481](https://github.com/ryanbreen/breenix/issues/481)
- 📋 **Designated-init runtime flag + panic-on-init-exit** — [#464](https://github.com/ryanbreen/breenix/issues/464)
- 📋 **Duplicate SIGCHLD / stale exit code on the already-terminated exit path** — [#433](https://github.com/ryanbreen/breenix/issues/433)
- 📋 **Idle-path CoW-walk latency under IRQ mask** in the `schedule_from_kernel` drain — [#448](https://github.com/ryanbreen/breenix/issues/448)

Run `gh issue list --repo ryanbreen/breenix` for the full current backlog.
