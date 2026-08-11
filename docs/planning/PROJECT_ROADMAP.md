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

- ✅ **AArch64 IRQ-resume page-table corruption fixed** ([PR #531](https://github.com/ryanbreen/breenix/pull/531), Aug 2026, closes [#528](https://github.com/ryanbreen/breenix/issues/528))
  - Root cause: compiler-generated NEON `memset` (used for page-table zeroing) ran on hardware-FP kernel codegen. The aarch64 IRQ entry/exit path does not preserve FP/SIMD registers, so an IRQ landing mid-`memset` resumed with a clobbered `q0`, spraying live machine values over the rest of the table — the contiguous 32-byte-boundary tails with 16-byte repeats captured by the Gate-0 discriminator.
  - Fix: split-target architecture. Kernel builds general-regs-only/soft-float (Rust and C, via `-mgeneral-regs-only`), eliminating live kernel `q`/`v` state across IRQ/syscall boundaries by construction (0 FP/SIMD-register operand lines across the entire linked kernel ELF, down from 4,807 at `main`). Userspace is unchanged — hard-float, 142/142 ELFs byte-identical to `main` (SHA-256).
  - Note: userspace FP/SIMD state is *not* preserved across task switches — that EL0↔EL0 exposure is separate and pre-existing, tracked at [#529](https://github.com/ryanbreen/breenix/issues/529). This fix only removes kernel-side FP clobbering.
  - Proven via DECISIVE unmasked proof gate 0/60 (vs. a ~3/35 baseline on the unfixed vehicle), clean gate 0/100, starved gate (14-hog) 0/100, beast x86 kthread 3/3, and 3 consecutive long-window Parallels boots (380/352/366 heartbeats, zero fault markers).
  - Merge commit: `26e96253969426b723fb67aa6ce8b71140f25cfd`.
  - Follow-ups tracked at [#529](https://github.com/ryanbreen/breenix/issues/529) (EL0↔EL0 FPSIMD context-switch preservation) and [#530](https://github.com/ryanbreen/breenix/issues/530) (build-time guard against a kernel build accidentally selecting the userspace hard-float target; portable linked-kernel q/v/d/s ratchet).
  - `fix/470-process-root-reclaim` remains open/held — its oracle found this bug. `repro/528-unmasked` is kept as the proof harness.

- ✅ **Parallels-only init `DATA_ABORT` boot regression fixed** ([PR #525](https://github.com/ryanbreen/breenix/pull/525), Aug 2026)
  - Culprit: CPU0 breadcrumb asm in `aarch64_enter_exception_frame` left behind by the P1 teardown-unification merge ([308c281b](https://github.com/ryanbreen/breenix/commit/308c281b)). Deterministic on Parallels, invisible to QEMU, so it survived every prior gate.
  - Found by the conclusive #519 post-merge demonstration, not by a new bug report.
  - Proven via 0/100 + 0/100 QEMU gates, beast x86 3/3, and 3 consecutive green Parallels boots on `main` at `8ef8575f`.
  - Merge commit: `8ef8575fc98c1355053bee22f0af318a80c00ad5`.

- ✅ **`timer_delay` starved-boot false-FAIL fixed** ([PR #524](https://github.com/ryanbreen/breenix/pull/524), Aug 2026)
  - Fixed via counter-verified host-stall crediting, so a starved boot no longer misreports as a real `timer_delay` failure.
  - Discovered by the #519 post-merge demonstration.
  - Merge commit: `7ad21766d93abc1db6589f84b711ad0b1b331432`.

- ✅ **`exit_kick_protocol_gate` SMP-liveness flake fixed** ([PR #521](https://github.com/ryanbreen/breenix/pull/521), Aug 2026, closes [#519](https://github.com/ryanbreen/breenix/issues/519))
  - Fixed the rare (~1% zero-load) PSCI CPU_ON 3-of-4 CPU bring-up miss that could fail `exit_kick_protocol_gate` with progress-re-armed liveness waits, keyed to the awaited kthread's own execution counters rather than a fixed wall-clock window.
  - Added a periodic resched-SGI re-kick (was one-shot self-heal), a bounded PSCI CPU_ON retry, per-CPU bring-up-stage breadcrumbs, and a 5-8s SMP online-wait with progress lines instead of a single hard timeout.
  - Proven via 0/100 starved (14-hog) + 0/100 clean confirmation runs, plus beast x86 3/3.
  - Merge commit: `306d665c67a42ce792e605759f278eb938ca4cf3`.
  - Follow-ups explicitly rated non-blocking at merge review and tracked at [#522](https://github.com/ryanbreen/breenix/issues/522): multi-target waits use a summed-union window instead of strict per-target windows; the shared Phase-1 ceiling is anchored at kernel entry (a new, if remote, false-FAIL mode on slow-but-healthy boots); six new failure classes no longer match the canonical `exit_kick_gate:.*unresponsive` grep, so gate scripts need widening; a latent off-by-one in the new test-helper raw-string scanner.

- ✅ **Teardown-unification tranche 1 (P0+P1+P2) COMPLETE — SIGKILL use-after-free spine closed** ([PR #515](https://github.com/ryanbreen/breenix/pull/515), Aug 2026)
  - P2 (SPINE-1) stops SIGKILL/fault-kill from eager-freeing victim frames, routing the kill path through `exit_process_and_retire` receipt custody + the `EXIT_KICK` expedite SGI, closing the cross-CPU use-after-free tracked at [#491](https://github.com/ryanbreen/breenix/issues/491). This completes **tranche 1** of the `docs/planning/teardown-unification/` phased plan (P0 observability/ratchet + P1 retirement-fence/drain restructure + P2 spine), all three now merged to `main`.
  - Gates: `exit_kick_protocol_gate` deterministic 100/100 (hardened with bounded reservation handshakes + self-heal IPI after a fatal reusability defect found in review — see `docs/planning/teardown-unification/PLAN.md` CHANGELOG); `fork_exit_defer_reclaim_pairing_test` deterministic 100/100 (unblocked by #513, a test observation-race fix); 0 fault/UAF/panic markers across confirmation boots; beast x86 3/3. x86 fault-path findings from review adjudicated non-blocking, tracked at #511.
  - Pre-existing, unrelated flake filed as a follow-up: ARM64 `timer_quantum_reset_aarch64` / `arm64_socket_reset_quantum` fail ~4% at zero load — [#516](https://github.com/ryanbreen/breenix/issues/516).
  - Merge commit: `6003c7a6758a51c4f2092f8a1e3a502432273795`.

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
- 📋 **Fault-exit deferred drain unbounded under IRQ mask** (`DEFERRED_FAULT_EXIT_BUFFERS` can replay up to 128 passes in one `schedule_from_kernel` call) — [#492](https://github.com/ryanbreen/breenix/issues/492)
- 📋 **`check_signals_for_eintr` ignores signal disposition** (default-ignored signals spuriously interrupt futex/epoll/`wait.rs`/AHCI/`socket.rs`/`time.rs` waits) — [#493](https://github.com/ryanbreen/breenix/issues/493)

Run `gh issue list --repo ryanbreen/breenix` for the full current backlog.
