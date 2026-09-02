# #745 spec: x86 `fork()` refused in the production profile

> **Archived verbatim.** This file is the investigation artifact the #745
> implementation round followed, committed unedited. `scripts/claim-lint.py`
> reports findings in it (43 at the round-2 bytes); they are NOT discharged
> here, because annotating or rewriting an archived document to satisfy a
> linter would change the record of what was written at the time. Claims this
> PR itself makes live in `README.md`, which is claim-lint clean. See
> `docs/planning/green-program/claim-linting.md`.

Repo `/Users/wrb/fun/code/breenix`, investigated STRICTLY READ-ONLY at
`main` @ `71ffda8184d00a29e7535cae53db628766bc400c` (HEAD did not move during
the investigation). x86 facts cross-checked live on `beast`
(`incus exec breenix-x86`, checkout `c3c41657...`, confirmed
`git merge-base --is-ancestor` true against this sha, one merge commit
behind) — every line-number citation in this spec that was grepped on beast
matched the local tree exactly, including the issue's own three original
citations (`manager.rs:2246`, `:2390`, `:3044`), which have **not drifted at
all** since #745 was filed.

Issue: `gh issue view 745`. Filed by #721's own TTY-arm-14 re-admission
attempt (commit `40d3ead8`, reverted by `7e2484ce` once fork — not exec —
was identified as the actual blocker).

Precedents read in full and applied throughout: `p713/spec.md` +
`p713/precheck.md` (SPAWN, PR #730), `p721/spec.md` + `p721/precheck.md` +
`p721/review.md` (exec, PR #747, merged same day as this investigation).
**The central #721 lesson — a title-level "just swap X" fix compiles, passes
a smoke boot, then corrupts state on the first preemption, reproducing a
historical crash class in a new arch — reproduces exactly for fork, and is
this spec's most important finding (§3, item 1).**

---

## 1. ROOT CAUSE — exactly why production fork returns an error

**The dispatch arm is already correct and needs no edit.**
`kernel/src/syscall/handler.rs:222`: `Some(SyscallNumber::Fork) =>
super::handlers::sys_fork_with_frame(frame)` — Tier-1, untouched, routes
correctly today. Same as #721's finding for `Exec`: no Tier-1 diff is
needed for this issue.

**The refusal is three layers down, in one specific function.**

1. `sys_fork_with_frame` (`handlers.rs:1838`, `#[cfg(target_arch =
   "x86_64")]`, unconditional on features) builds a `CpuContext` from the
   syscall frame and calls...
2. `sys_fork_with_parent_context` (`handlers.rs:1866`, `#[cfg(target_arch =
   "x86_64")]`, unconditional on features) — **wraps its entire body,
   `:1868`–`:1985`, in `crate::arch_without_interrupts(|| { ... })`** (see
   §3.1 for why this is itself a major finding, independent of the
   testing-gate). It finds the parent process under a brief PM-lock window,
   drops the lock, calls `reclaim_terminated_threads()`, allocates a new
   `ProcessPageTable`, then calls...
3. `manager.fork_process_with_parent_context` (`manager.rs:2272`) — this
   function is itself unconditional, but its body is split:
   ```rust
   #[cfg(feature = "testing")]
   { /* CoW setup: setup_cow_pages_with_vmas + heap/mmap/vma inheritance */ }
   #[cfg(not(feature = "testing"))]
   {
       log::error!("fork_process: Cannot fork - testing feature not enabled");
       return Err("Cannot implement fork without testing feature");
   }
   ```
   The `not(testing)` arm is at `manager.rs:2388`–`2391` — this is the exact
   refusal the issue quotes, confirmed byte-identical on beast today. The
   gated CoW block sits at `:2348`–`:2387`.

**Three near-identical refusal sites exist in `manager.rs`, but only one is
on the live production syscall path.** The issue itself names all three
(`:2246` in `fork_process_with_page_table`, `:2390` in
`fork_process_with_parent_context`, `:3044` in `fork_process_with_context`).
Grepping every call site tree-wide:

| Function | Called from |
|---|---|
| `fork_process_with_parent_context` | `handlers.rs:1938` — **the live `sys_fork_with_frame` → `sys_fork_with_parent_context` chain, unconditional** |
| `fork_process_with_page_table` | `tracing/providers/teardown.rs:1865,2753,5797` only — all `#[cfg(feature = "boot_tests")]` |
| `fork_process_with_context` (via `fork_process()`) | `test_exec.rs:125,394`, `userspace_test.rs:835` (all `testing`-gated), plus `teardown.rs:5815` (`boot_tests`-gated) |

Per #721's own minimal-wiring precedent (it left `sys_exec_with_frame`/
`load_elf_from_ext2` untouched because neither had a live production
caller), **only `fork_process_with_parent_context` and its callee
`complete_fork` (`manager.rs:2685`, `#[cfg(all(target_arch = "x86_64",
feature = "testing"))]`) need de-gating.** The other two functions are only
ever reached from code paths where `testing`/`boot_tests` is already
enabled — touching them is optional hygiene, not required for #745's
production fix, and doing so unasked would widen the diff past what's
load-bearing.

---

## 2. MINIMAL CORRECT WIRING — file by file, tier-classified

| File | Tier | Change |
|---|---|---|
| `kernel/src/syscall/handler.rs` | **Tier-1** | **No change.** `Fork` arm at `:222` already routes correctly. |
| `kernel/src/syscall/handlers.rs` | Unrestricted | Restructure `sys_fork_with_parent_context` (`:1866`–`:1986`): remove the `arch_without_interrupts` wrap; add the missing `reclaim_deferred_process_resources()` call (§3.2). See §3.1 for the exact target shape (aarch64's `sys_fork_aarch64`). No existing *other* function touched. |
| `kernel/src/process/manager.rs` | Unrestricted | De-gate exactly two functions: strip the inner `#[cfg(feature = "testing")]`/`#[cfg(not(feature = "testing"))]` split in `fork_process_with_parent_context` (`:2348`–`:2394`) so the CoW block always runs; change `complete_fork`'s outer gate (`:2685`) from `#[cfg(all(target_arch = "x86_64", feature = "testing"))]` to `#[cfg(target_arch = "x86_64")]`. `fork_process`, `fork_process_with_page_table`, `fork_process_with_context` **stay untouched** (§1). |
| `userspace/programs/src/init.rs` | Unrestricted, userspace | Correct the false claim near `run_tty_oracle()`'s x86 doc comment ("setsid/ioctl/PTY/termios/fork are all production-safe on x86 already" — see §3.10, it is not and never has been runtime-proven). Add `run_fork_smoke()` call site (§4). |
| `userspace/programs/src/fork_smoke.rs` (new) | Unrestricted, userspace | New acceptance program, arch-neutral (no `target_arch` cfg needed — mirrors `exec_smoke`'s precedent). See §4. |
| `tests/teardown_structure.rs` | Unrestricted (test infra) | **Must update in the SAME commit as the manager.rs/handlers.rs cfg changes** — five separate census arrays pin the exact cfg-path strings being changed. This is #745's own pre-registered #721-B1-equivalent landmine; see §3 "Precheck landmine" below. Non-optional. |
| New `tests/fork_lock_order_structure.rs` (or an addition to an existing `*_structure.rs`) | Unrestricted (test infra) | Structural, mutation-provable assertion that the masked-interrupt window never re-encloses the PM lock / page-table allocation / scheduler publish (§4d). |
| `docker/qemu/run-x86-prod-profile-boot-test.sh` | Unrestricted (test infra) | New `FORK_SMOKE_*` markers, following the `EXEC_SMOKE_*` template exactly. |
| `docs/planning/745-x86-fork/` (new) | Unrestricted | Spec/round-plan/serials, per project convention. |

**Not touched, and should not be:** `kernel/src/process/fork.rs` (already
arch-neutral and correct — `setup_cow_pages_with_vmas`,
`copy_process_state` need no changes, §3.4/§3.8); `libs/libbreenix/*`
(`fork()`/`ForkResult` already correct and already the exact call `bsh.rs`
uses, §3.10); `kernel/src/arch_impl/aarch64/*` (aarch64 fork is the
reference implementation this fix matches, not touches);
`kernel/src/interrupts/*`, `context_switch.rs`, `kthread.rs`,
`workqueue.rs`, `gdt.rs`, `per_cpu.rs`, `entry.asm`, `time.rs`,
`timer.rs`/`timer_entry.asm` (Tier-1/Tier-2, no reason to touch — this fix
is entirely about syscall-context lock/interrupt discipline in
`handlers.rs`, not about restore/dispatch itself, exactly as #721 found for
exec).

---

## 3. HARDENING-PARITY TABLE

**This is the section that matters most.** Answering the task's direct
question — what hardening has aarch64 fork received that x86 fork has
not — item by item, verified against both implementations line-for-line.

### 1. Interrupt-masking discipline across the whole operation — CRITICAL, THE #721-B1-EQUIVALENT FINDING

**aarch64** (`sys_fork_aarch64`, `arch_impl/aarch64/syscall_entry.rs:874`–):
carries an explicit in-code postmortem comment (`:899`–`:905`):
> "NOTE: No without_interrupts wrapper! ... Wrapping the entire fork in
> without_interrupts caused deadlocks on single-CPU ARM64: fork acquires
> the logger lock, heap allocator lock, FRAME_METADATA lock, and pipe
> buffer locks while interrupts are disabled. If ANY other thread was
> preempted while holding one of those locks, the single CPU deadlocks
> permanently... This was the root cause of the intermittent 1-in-5 boot
> hang."
Only the PM lock's own interrupt-disable brackets the two narrow windows
that need it (`:908`–`:920` read parent info, `:938`–`:947` fork + publish);
everything else — `reclaim_deferred_process_resources()` +
`reclaim_terminated_threads()` (`:924`–`:925`), `ProcessPageTable::new()`
(`:929`–`:933`), `scheduler::spawn_front()` (`:970`) — runs with hardware
interrupts enabled.

**x86** (`sys_fork_with_parent_context`, `handlers.rs:1866`–`:1986`):
**still wraps the entire function body in `crate::arch_without_interrupts(||
{ ... })`** (opened `:1868`, closes at the function's own `:1985`/`:1986`).
Every one of the following runs with hardware interrupts disabled for the
whole duration: the PM-lock acquire/drop/re-acquire cycle, `find_process_by_thread`,
`reclaim_terminated_threads()`, `ProcessPageTable::new()` (frame + heap
allocation), the CoW page-table walk once de-gated
(`setup_cow_pages_with_vmas`), `publish_to_scheduler()`,
`scheduler::spawn_front()`, and roughly a dozen `log::info!`/`log::debug!`/
`log::error!` calls throughout.

**Verdict: GAP, and the dangerous kind.** This is not a missing feature —
it is the *live presence*, today, of the exact anti-pattern aarch64's own
commit history already proved causes an intermittent single-CPU deadlock.
A fix that only flips `fork_process_with_parent_context`'s
`#[cfg(not(feature = "testing"))]` refusal to the real body — the shape the
issue's own "suggested shape" section implies, and exactly the class of fix
#721's precheck flagged as "not a risk... contradicts the issue's own
suggested fix shape" for exec's loader swap — would compile, pass a single
uncontended smoke boot, and then hang or deadlock the first time another
thread is preempted mid-lock-hold during a fork call: reproducing, on x86,
the identical historical crash class aarch64 already paid down once. This
is #745's version of #721's central correction.

**Fix:** remove the `arch_without_interrupts` wrapper entirely; restructure
to aarch64's shape — narrow PM-lock window to read parent info (drop before
anything else), reclaim calls with interrupts enabled and no lock held,
`ProcessPageTable::new()` with interrupts enabled, a second PM-lock window
for the actual fork + `publish_to_scheduler()`, drop that lock, then
`scheduler::spawn_front()` with interrupts enabled. The PM lock's own
interrupt-disable is already the documented, sufficient synchronization —
same rationale `creation.rs:31`–`36`, `sys_spawn` (#713), and aarch64 fork
itself all already state.

### 2. Reclaim ordering before consuming a fresh kernel-stack-pool slot (P4/#601; #713's C8; #721's K12)

**aarch64:** calls **both** `process_task::reclaim_deferred_process_resources()`
**and** `scheduler::reclaim_terminated_threads()` (`syscall_entry.rs:924`–`925`)
before `ProcessPageTable::new()`.

**x86:** calls **only** `scheduler::reclaim_terminated_threads()`
(`handlers.rs:1918`ish) — the process-resource reclaim call is missing
entirely.

**Verdict: GAP.** Same shape #713's C8 and #721's K12 already fixed for
spawn/exec — every x86 syscall that consumes a fresh
kernel-stack-pool/process-table slot must drain *both* reclaim passes
first, not just one. Independently corroborated by the codebase's own
documentation: `tests/teardown_structure.rs`'s `DEFERRED_RECLAIM_DRAIN_SITES`
census (`:3152`) carries the comment "#713: sys_spawn drains ... mirroring
sys_fork_with_parent_context's own ordering" — **the project's own test
comments already assume fork does this; the code doesn't yet.** Fix: add
the missing call, same relative position as aarch64, no lock held,
interrupts enabled.

### 3. Scheduler-side publication / `ExecSchedCommit`-equivalent

**aarch64:** `publish_to_scheduler()` under the PM lock → extract child
thread info → explicit `drop(manager_guard)` **before**
`scheduler::spawn_front()` (`:940`–`:970`).

**x86:** identical ordering — `publish_to_scheduler()` under the PM lock →
`drop(manager_guard)` **before** `scheduler::spawn_front()`
(`handlers.rs:1938`–`1953`).

**Verdict: NOT A GAP — verified identical, and correctly so.** Unlike exec
(which must safely mutate an **already scheduler-resident** thread object,
hence the staged-receipt/drop-before-apply `ExecSchedCommit` discipline
#721 built to prevent the historical "keeps pre-exec context, faults on
first restore" bug), fork's child thread has **never been published
before** — there is no existing scheduler-side copy that could go stale.
`publish_to_scheduler()` + `spawn_front()`, called after the PM lock is
already dropped, is the correct and sufficient mechanism, and x86 already
gets the *ordering* right. **Answering the task's direct question: this is
the one area where x86 fork already matches aarch64's hardening.** It
should not be "improved" into an `ExecSchedCommit`-style receipt — that
would solve a problem fork doesn't have. The only real defect in this
neighborhood is item 1 above: the correct lock ordering is undermined by
remaining wrapped in a hardware-interrupt mask for the entire duration.

### 4. CLONE_VM sibling / page-table-retirement guard (#721 B2/K5, `find_live_clone_vm_sibling_holding_cr3`)

**Neither arch's fork calls it, and neither should.** `grep` confirms zero
call sites of `find_live_clone_vm_sibling_holding_cr3` inside
`fork_process_aarch64`/`complete_fork_aarch64` or any x86 fork function —
it is called only from the four exec functions. Fork never retires a live
CR3/TTBR0 root the way exec does: it always builds a brand-new, independent
`ProcessPageTable` for the child via CoW (the parent's own page table is
only transiently taken-and-returned inside the CoW-setup block, on both
arches, never actually swapped out from under a running thread). Confirmed
`copy_process_state` (`fork.rs`, shared, arch-neutral) never writes
`thread_group_id`/`inherited_cr3`, and `Process::new()`'s defaults leave
both `None` for the child — a forked child can never be mistaken for a
CLONE_VM sibling of anything.

**Verdict: checked, genuinely not applicable** — named explicitly (per this
project's "state why a risk doesn't transfer" convention, not silently
skipped) rather than left implicit.

### 5. Page-table / CR3 handling generally

Both arches build an entirely new, independent page table for the child
(`ProcessPageTable::new()` + CoW setup) and never touch the *currently
loaded* CR3/TTBR0 of the parent thread — the parent keeps running on its
own table, and the child's table isn't installed anywhere until the
scheduler actually dispatches it. No architecture-specific novelty is
needed on x86; the `ProcessPageTable`/CoW machinery this function already
calls (once de-gated) is the identical, already-proven-in-production
machinery `spawn_process`/`create_user_process` exercise today.

**Verdict: NOT A GAP.**

### 6. `pending_old_page_tables` discipline (#721's own central fix)

**Not applicable to fork on either arch** — fork never retires an old page
table (only exec does, detaching a *running* thread from its pre-exec
address space). Confirmed: no `pending_old_page_tables` reference anywhere
in `complete_fork`/`complete_fork_aarch64`.

**Verdict: not applicable, checked and cleared.**

### 7. Custody of the child's kernel stack (P4/#601, #583)

Both arches use the identical, arch-neutral
`crate::memory::kernel_stack::allocate_kernel_stack()` call, wire the
result into the same `Thread` fields the same way, and transfer sole
ownership into the scheduler's copy via the one shared
`Thread::publish_to_scheduler()` both callers use.

**Verdict: NOT A GAP — genuinely identical.** One disclosed, pre-existing,
cross-cutting risk applies equally to both arches once fork becomes a live
per-boot consumer, and is not specific to fork: the finite user-stack-VA
bump allocator (#720, ~240 process creations/boot before exhaustion) and
the `GuardedStack` no-op-`Drop` leak (#583) that #721's precheck (C14)
already flagged for spawn/exec. **Disclose in the PR body citing #720/#583;
do not attempt to fix in this arc** — standing, filed, cross-cutting, not
this issue's job.

### 8. fd-table cloning + refcount protocol (#707/#724)

Both arches call the identical, fully shared, arch-neutral
`copy_process_state()` (`fork.rs:474`) — `child_process.fd_table =
parent_process.fd_table.clone()` (doc comment: "clone increments
pipe/PTY refcounts"). Both `complete_fork` and `complete_fork_aarch64` call
it identically, same position, same arguments shape.

**Verdict: NOT A GAP — clean parity.** #707/#724's refcount-correct `Clone`
impl on `FdTable` applies uniformly to both arches already. This is the one
area of the whole fix requiring zero new code and zero new scrutiny.

### 9. TTY / ctty / foreground-pgrp interference

Neither arch's fork path calls `set_foreground_pgrp` anywhere — a forked
child correctly does not steal the terminal, matching exec's
already-verified behavior (#721 precheck §3.4/C10).

**Verdict: NOT A GAP.**

### 10. Runtime-proof vs. compile-time-presence (recurrence of #721's precheck C-g)

`userspace/programs/src/init.rs`'s own in-tree doc comment (directly above
x86's `run_tty_oracle()`) states: *"setsid/ioctl/PTY/termios/fork are all
production-safe on x86 already."* **This is false today**, and — more
importantly — it has never been runtime-exercised even as a claim about
intent: `/etc/init.js` (`scripts/create_ext2_disk.sh:591`–`614`) issues only
the JS `spawn()` builtin, seven times, zero fork-triggering paths. So
`bsh.rs`'s own three `libbreenix::process::fork()` call sites — `:418`
(exec-builtin), `:729` (pipe builtin), `:1169` (general "run external
command") — have **never executed on x86 in production**, despite being
compiled in and load-bearing for any real interactive or scripted use of
`bsh` (which is itself already reachable in production today via
`run_boot_script()`'s `spawn(b"/bin/bsh\0", [..., "--init-shell", ...])`,
per #713). This is the identical "compile-time presence is not runtime
proof" trap #721's precheck flagged (C-g) for exec's pre-split argv
parsing.

**Verdict: GAP in documentation/verification, not in code shape.** Correct
the false comment in the same round (§2), and treat bsh's three `fork()`
call sites as newly-reachable, currently-unvalidated production surface
once #745 lands (§4g).

---

### Precheck landmine — pre-registered so implementation doesn't discover it after the fact (this arc's #721-B1)

`tests/teardown_structure.rs` pins the **exact cfg-path string** of the two
functions §2 de-gates, across **five separate census arrays** — verified by
direct grep, not assumed:

| Array (line) | Entry that must change |
|---|---|
| `PROCESS_ROW_MAP_MUTATIONS` (`:2792`) | `"impl ProcessManager::#[cfg(all(target_arch=x86_64,feature=testing))] fn complete_fork => insert"` |
| `KERNEL_STACK_MUTATIONS` (`:2846`) | `"impl ProcessManager::#[cfg(all(target_arch=x86_64,feature=testing))] fn complete_fork"` |
| `ALLOCATE_ORDINARY_PID_CALLS` (`:2746`) | `"impl ProcessManager::#[cfg(target_arch=x86_64)] fn fork_process_with_parent_context"` — outer cfg unchanged, but must be **re-verified**, not assumed stable, since the function body's internal cfg boundary is what's being removed and `call_offsets` scans the whole body under a `code_mask` |
| `PROCESS_PAGE_TABLE_CONSTRUCTORS` (`:3142`) | `"#[cfg(target_arch=x86_64)] fn sys_fork_with_parent_context"` — same re-verify caveat |
| `DEFERRED_RECLAIM_DRAIN_SITES` (`:3152`) | **currently has no entry for `sys_fork_with_parent_context`** — once §3.2's missing `reclaim_deferred_process_resources()` call is added, a new entry must be appended, **and** the array's own doc comment ("Both x86 production calls are normal-context sites") must become "All three," **and** the delete-mutation anti-vacuity proof re-run |

Any one of these left stale fails the corresponding `validate_*` function in
`tests/teardown_structure.rs` on the very next `cargo test` — this is
exactly the class of miss #721's own review caught as **blocking** (B1: a
different structure-test file, `context_restore_structure.rs`, reddened by
an un-registered new call site the spec's author never ran). **Update all
five in the same commit as the cfg changes, not in a follow-up.**
`context_restore_structure.rs`'s only fork-related mention (`:5774`,
`sys_fork_aarch64` inside a negative-precision fatal-handler-reachability
check) was checked and confirmed unrelated to x86 — no landmine there.

---

## 4. ACCEPTANCE

The x86 prod gate must observe a **real** fork + child-runs + exit + reap in
production, with anti-vacuity both directions, plus whatever receipt
oracles are actually applicable (per §3 item 3, fork has no
`ExecSchedCommit`-shaped receipt to assert on — that's correct, not a gap,
so the oracle here is structural/host-side instead of a kernel print).

**a) New userspace acceptance pair — `fork_smoke.rs`** (arch-neutral, no
`target_arch` cfg, mirroring `exec_smoke`'s already-proven pattern):
prints `[FORK_SMOKE:LAUNCH]`, calls `fork()`; the child prints
`[FORK_SMOKE:CHILD pid={}]`, performs **at least one voluntary yield**
before exiting with a distinguishing code — deliberately, to force the
freshly-published, never-preempted child thread through a real
context-switch/reschedule cycle, exactly the "descheduled and redispatched
at least once" scenario `exec_smoke_target`'s own header comment cites as
its whole point, and exactly the scenario §3 item 1's masked-interrupt
regression would need to survive. The parent `waitpid`s and prints
`[FORK_SMOKE:PARENT_REAPED child={} code={}]` only on a genuine `Ok` reap
(mirroring `run_tty_oracle`'s review-B3-hardened honest-waitpid pattern,
not a pre-zeroed-status fabrication). Negative markers:
`[FORK_SMOKE:FORK_FAILED {}]`, `[FORK_SMOKE:CHILD_UNEXPECTED_RETURN]` (a
real historical fork-bug shape: `fork()` returning twice into the same
branch), `[FORK_SMOKE:REAP_FAILED {}]`.

**b) `init.rs`:** new `run_fork_smoke()`, `#[cfg(target_arch = "x86_64")]`,
positioned after `run_exec_smoke()` and before `start_bsshd()` (the same
slot convention `run_spawn_smoke`/`run_tty_oracle`/`run_exec_smoke` already
occupy), launched via `spawn()` (already proven in production, #713) and
waited on directly — not entangled with `run_boot_script()`'s own reap
loop, per #713/#721's own established rationale for this exact code region.

**c) Gate script (`run-x86-prod-profile-boot-test.sh`):** new
`FORK_SMOKE_*_LITERAL`/`_PREFIX` constants and assertions, following the
`EXEC_SMOKE_*` template verbatim (`:491`–`506`, `:664`–`674`, `:915`–`925`
as the exact pattern to copy). Positive markers `-eq 1`, negative markers
`-eq 0`. Re-measure `LIVENESS_WINDOW_SECONDS` (currently `60`, `:556`) —
#721 needed to extend this same window for `exec_smoke`'s added work
(commit `31ea763b`); do not assume it is unaffected this time either.

**d) New structural, mutation-provable host test** (`tests/fork_lock_order_structure.rs`
or an addition to an existing suite, mirroring `exec_lock_order_structure.rs`'s
convention) asserting, as a text-scanner over `sys_fork_with_parent_context`'s
body:
- zero occurrences of `arch_without_interrupts(`/`without_interrupts(`
  enclosing the PM-lock windows, `ProcessPageTable::new(`, or
  `scheduler::spawn_front(` — proving §3.1's fix **by construction**, not by
  hoping a race shows up in a 25-boot sample. This is #745's version of
  `validate_sys_exec_releases_process_manager`.
- exactly one call each to `reclaim_deferred_process_resources` and
  `reclaim_terminated_threads`, both preceding `ProcessPageTable::new(`.
- a delete-mutation proof for every assertion (reintroduce the wrapper,
  drop a reclaim call, reorder something) confirmed to redden — per this
  project's standing anti-vacuity rule, and explicitly learning from
  #721's review finding M1 ("K13 met" was reported without ever actually
  reddening most of the new gate assertions under mutation). Do not repeat
  that in this arc's own prove.md.

**e) Anti-vacuity negative control:** run the identical extended gate
against the pre-fix tree (fork refused, current
`[ERROR] ... Cannot implement fork without testing feature` present) and
confirm every new positive marker is absent. Confirms the gate is not
vacuously green on old code.

**f) Round-count:** standard 25-boot/profile budget (this project's
standing gate-sizing rule — no AC arithmetic here justifies more), zero
UNATTRIBUTED failures, both anti-vacuity directions captured as in-repo
evidence.

**g) `bsh.rs`'s three now-newly-reachable `fork()` call sites (§3 item
10):** at minimum, disclose them in the PR body as an unvalidated residual
(mirroring #721's own m7 disclosure pattern for FD_CLOEXEC-across-exec
coverage). If scope allows, extend `/etc/init.js` — or a dedicated
bsh-driven smoke leg — to exercise bsh's real "run an external command"
fork+exec path at least once in the gate, proving the corrected doc comment
(§2) is now actually true rather than merely less false.

---

## 5. tty_oracle arm 14 (`cloexec_exec`) — can it be re-admitted in this arc?

**Yes — as an explicit, separately-gated round within this same arc (#745),
not deferred to a new issue.** Four concrete reasons:

1. The exact 5-edit diff already exists in git history — commit `40d3ead8`
   (`test(tty,x86): re-admit the cloexec_exec arm now that x86 exec() works
   (#721)`), reverted only by `7e2484ce` because **fork**, not exec, turned
   out to be the blocker. Nothing about that diff needs rediscovery, only
   re-application once fork's own gate is green. (Correcting the review's
   own minor finding C-i / precheck's tally: it touches **five**
   `#[cfg(target_arch = "aarch64")]` sites in `tty_oracle.rs` — `:29`,
   `:32`, `:721`(now `:722` per current line, verify at implementation
   time), `ARM_COUNT`, and the call site — not three.)
2. #721's own review already closed the exec-side half of what arm 14
   needs: the CLONE_VM sibling guard now covers **both** x86 exec functions
   (review B2 fix), and `close_cloexec()` is verified present and correct
   on both.
3. Sequencing it as its own round, gated on this arc's own base-fork gate
   being green first, preserves #721's K14 diagnostic-separability
   discipline: a red arm-14 result stays attributable to "the re-admission
   edit" vs. "fork itself," never conflated.
4. It is genuinely cheap: five mechanical edits in `tty_oracle.rs`, plus
   `run-x86-tty-oracle-gate.sh`'s `EXPECTED_ARMS` + delete the
   `CLOEXEC_EXEC_VERDICT_LITERAL` zero-verdict block, plus delete
   `tests/tty_oracle_structure.rs`'s now-stale
   `the_x86_gate_refuses_a_cloexec_exec_verdict`.

**But it must land after §4's Round 2/3 gate is green, not alongside it,
and needs its own soak beyond a single boot** — for one genuinely new
reason neither #721 nor the historical revert surfaced. Arm 14
(`tty_oracle.rs:763`–`778`) does `fork()` **then immediately `exec()`s a
fresh copy of itself** in the child, before that freshly-forked,
never-preempted, `has_started=true` thread has ever survived a normal
context-switch round trip. **This exact fork-then-immediate-exec sequence
has never been proven by any existing or proposed gate:** `exec_smoke` is
launched via `spawn()` (a brand-new process construction, not a fork —
§4's own `fork_smoke` forces a voluntary yield *before exiting* but does
not chain into an exec afterward). Arm 14 is therefore the **first** test
anywhere in the tree of "child of a fork immediately calls exec()" on x86,
exercising the interaction between fork's plain `publish_to_scheduler()`
and exec's `ExecSchedCommit` re-publish of that same, still-very-fresh
thread. **Recommend a dedicated multi-boot soak (not the standard
single-pass gate) for this round specifically because of that novel
interaction**, before trusting it as green — this is exactly the kind of
"first-run interaction" #713's own precheck (C11) and #721's own round-3
sequencing rationale both warned about for shapes no existing gate had
exercised.

---

## 6. SIZING + ROUND PLAN

No line/file ceiling (`fix-scope-budgets-never.md`). Realistic shape:
`handlers.rs` restructure ~40-70 changed lines (mostly moving existing code
out from under the closure, not new logic); `manager.rs` ~10 lines changed
(two cfg-gate edits, no new logic — the CoW block itself is untouched,
only its gate); new `fork_smoke.rs` ~60-90 lines (mirrors `exec_smoke.rs`'s
size); `init.rs` ~20 lines (`run_fork_smoke()` + comment fix); gate script
~30-50 lines (new markers, template-copied from `EXEC_SMOKE_*`);
`teardown_structure.rs` ~10 lines across 5 arrays + 1 comment fix; new
structural test file ~80-150 lines (mirrors `exec_lock_order_structure.rs`'s
per-assertion style).

**Round 0 (this spec) — done.**

**Round 1 — recommended precheck slot.** Given the interrupt-masking
restructuring is real surgery on syscall-context lock/interrupt ordering
with genuine deadlock consequences if gotten wrong (§3.1), and both #713
and #721 each had blocking findings their own specs missed (#721's review
alone found 2 blockers: a false "safe to twin" claim reddening a host
census, and a missing guard call on a sibling function with zero live
coverage), a precheck pass is worth the round even though this spec
pre-registers the one landmine (§3 "Precheck landmine") most likely to
recur. Marginal value is smaller here than for #713/#721 specifically
*because* that landmine is already found and itemized — but the
lock/interrupt restructuring itself (§3.1) is new surgery neither prior
precheck covered, and deserves its own adversarial read before
implementation.

**Round 2 — implement.**
- `handlers.rs`: remove the `arch_without_interrupts` wrap in
  `sys_fork_with_parent_context`; restructure into aarch64's narrow-window
  shape; add the missing `reclaim_deferred_process_resources()` call
  (§3.1, §3.2).
- `manager.rs`: de-gate `fork_process_with_parent_context` + `complete_fork`
  only (§1, §2).
- `init.rs`: correct the false production-safety comment (§3.10); add
  `run_fork_smoke()` call site (§4b).
- New `fork_smoke.rs` (§4a).
- `tests/teardown_structure.rs`: update all five census arrays + the one
  stale comment (§3 "Precheck landmine") — **in this same commit**.
- New `tests/fork_lock_order_structure.rs` with delete-mutation proofs
  (§4d).
- Build clean (0 warnings, CLAUDE.md's zero-tolerance rule).
  GDB-verify the restructured lock/interrupt sequence before trusting the
  first boot — this touches syscall-context lock ordering with real
  deadlock stakes, worth a breakpoint pass even though `handlers.rs` isn't
  itself Tier-1/Tier-2, per CLAUDE.md's GDB-first debugging philosophy.

**Round 3 — extend + prove the gate (§4).** Gate script markers, 25-boot/
profile, anti-vacuity both directions, mutation-first on every new
assertion (explicitly not repeating #721 review M1's "reported met, never
actually reddened" mistake). File any real defect the gate surfaces as a
new issue rather than weakening an assertion to pass it, per standing
project practice.

**Round 4 (separate PR, gated on Round 3's gate being green) —** tty_oracle
arm-14 re-admission (§5), with its own multi-boot soak given the
fork-then-immediate-exec novelty. Mirrors #721's own K14 discipline
exactly: a separate PR, not folded into the round that lands the base fix.

**Merge:** trunk-based, PR + merge commit (never squash — preserve every
SHA), return to `main`, delete the branch, per this repo's standing
workflow.

**Verification commands (beast, per CLAUDE.md):**
```
ssh beast 'sudo -n incus exec breenix-x86 -- bash -lc "cd /root/breenix && cargo build --release --bin qemu-uefi 2>&1 | grep -E \"^(warning|error)\""'
ssh beast 'sudo -n incus exec breenix-x86 -- bash -lc "cd /root/breenix && ./docker/qemu/run-x86-prod-profile-boot-test.sh"'
ssh beast 'sudo -n incus exec breenix-x86 -- bash -lc "cd /root/breenix && cargo test --test teardown_structure --test context_restore_structure --test fork_lock_order_structure 2>&1 | tail -100"'
```
(zero-feature build for the gate — matches the production profile #713/
#721/#673/#718's gates all measure; host-side `cargo test` runs need no
`--features` flag either, they compile the test binary's own scanner code,
not the kernel.)
