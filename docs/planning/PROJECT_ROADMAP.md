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

- ✅ **PR #418 follow-ups: clone stack ownership, exit-status reporting + GPU DMA lifetime fix** ([PR #494](https://github.com/ryanbreen/breenix/pull/494), Aug 2026)
  - Closes two of #418's tracked follow-ups: [#481](https://github.com/ryanbreen/breenix/issues/481) `Thread::clone` now transfers `kernel_stack_allocation` to the scheduler-owned thread copy on aarch64 (mirroring `fork`), so the child's kernel stack is protected by the two-epoch retirement grace; [#433](https://github.com/ryanbreen/breenix/issues/433) repeat teardown passes on the same process now report the *first-recorded* exit status to `btrt`/waitpid instead of a later pass's parameter, while keeping SIGCHLD/parent-wakeup/`btrt` notification unconditional on every pass.
  - **GPU DMA lifetime fix**, found by this round's own Parallels launcher-test gate, not by review: a spurious `SIGCHLD` (default-ignored, but `check_signals_for_eintr` ignores disposition) aborted an interruptible VirtIO-GPU ctrlq completion wait mid-flight, freeing device-owned DMA buffers while the device still held live virtqueue descriptors; the device's later DMA write poisoned a `linked_list_allocator` `Hole.next` pointer, surfacing as an unrelated EL1 `DATA_ABORT` in `HoleList::deallocate`. Fixed by switching the ctrlq wait to `wait_timeout_uninterruptible`, matching the AHCI precedent. A dual Opus+Codex-Sol RCA converged independently on this root cause. 10/10 clean Parallels launcher-test runs post-fix.
  - Three new issues filed from residuals surfaced during RCA/review: [#491](https://github.com/ryanbreen/breenix/issues/491) (SIGKILL teardown bypasses the hardened exit path — pre-existing eager-free UAF class), [#492](https://github.com/ryanbreen/breenix/issues/492) (fault-exit deferred drain unbounded under IRQ mask), [#493](https://github.com/ryanbreen/breenix/issues/493) (`check_signals_for_eintr` ignores signal disposition — root cause of the GPU fix above; futex/epoll/`wait.rs`/AHCI/`socket.rs`/`time.rs` share the same imprecise check).
  - #464 (designated-init flag + panic-on-init-exit) stays open: reverted after five review findings across three attempts; needs design-first work together with #471. #448 (idle-path CoW-walk drain bound) stays open: three bounded-drain designs each drew a blocking review finding; must be designed together with #492.

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
- 📋 **Designated-init runtime flag + panic-on-init-exit** (design together with #471) — [#464](https://github.com/ryanbreen/breenix/issues/464)
- 📋 **Idle-path CoW-walk latency under IRQ mask** in the `schedule_from_kernel` drain (design together with #492) — [#448](https://github.com/ryanbreen/breenix/issues/448)
- 📋 **SIGKILL teardown bypasses the hardened exit path** (pre-existing eager-free UAF class in `send_signal_to_process`) — [#491](https://github.com/ryanbreen/breenix/issues/491)
- 📋 **Fault-exit deferred drain unbounded under IRQ mask** (`DEFERRED_FAULT_EXIT_BUFFERS` can replay up to 128 passes in one `schedule_from_kernel` call) — [#492](https://github.com/ryanbreen/breenix/issues/492)
- 📋 **`check_signals_for_eintr` ignores signal disposition** (default-ignored signals spuriously interrupt futex/epoll/`wait.rs`/AHCI/`socket.rs`/`time.rs` waits) — [#493](https://github.com/ryanbreen/breenix/issues/493)

Run `gh issue list --repo ryanbreen/breenix` for the full current backlog.
