# #745 adversarial pre-check — verdict: GO-WITH-CONDITIONS

Repo `/Users/wrb/fun/code/breenix`, READ-ONLY, verified at `main` @
`71ffda8184d00a29e7535cae53db628766bc400c` (same sha the spec names; tree
clean, HEAD did not move). Every citation below was read in this tree, not
taken from the spec.

**Bottom line.** The direction is right and §1's root cause is correct. But
the spec ships a *false safety rationale* borrowed from aarch64 (C1), calls
the un-gated CoW machinery "already-proven-in-production" when it has never
executed in a zero-feature build (C3), un-gates a path that kills the parent
process on an ENOMEM-class error (C2), and — despite a section titled
"Precheck landmine … pre-registered so implementation doesn't discover it
after the fact" — misses **three** further host-test landmines (C4, C5, C6)
and understates the newly-reachable production surface by an order of
magnitude (C13). Sixteen binding conditions.

---

## VERIFIED — what the spec got right

- `handler.rs:222` `Some(SyscallNumber::Fork) => super::handlers::sys_fork_with_frame(frame)`. Tier-1, no change needed. ✔
- `handlers.rs:1838` / `:1866` / the `crate::arch_without_interrupts(||` at `:1868` closing at `:1985`. ✔
- `manager.rs:2272` `fork_process_with_parent_context`; refusal `log::error!("fork_process: Cannot fork - testing feature not enabled")` + `return Err(...)` at `:2386`-`:2390`. ✔
- `manager.rs:2685` `#[cfg(all(target_arch = "x86_64", feature = "testing"))] fn complete_fork`. ✔
- x86 fork calls only `scheduler::reclaim_terminated_threads()` (`handlers.rs:1921`); aarch64 calls both (`arch_impl/aarch64/syscall_entry.rs:924`-`925`). §3.2 GAP confirmed. ✔
- The self-referential comment is real: `handlers.rs:2848`-`2850` ("mirrors `sys_fork_with_parent_context`'s ordering") and `tests/teardown_structure.rs:3158` both already *assume* fork drains both. ✔
- The five `teardown_structure.rs` census arrays are exactly at `:2746`, `:2792`, `:2846`, `:3142`, `:3152` with the entry strings quoted. ✔
- `exec_smoke.rs` carries zero `target_arch` cfgs — the arch-neutral precedent for `fork_smoke.rs` holds. ✔
- Gate template lines `:491`-`497`, `:664`-`670`, `:915`-`921`, `LIVENESS_WINDOW_SECONDS=60` at `:556`. ✔
- §3 items 4/6 (CLONE_VM sibling guard, `pending_old_page_tables`) genuinely do not transfer to fork. ✔
- §3 item 8 (fd-table clone via shared `fork.rs:474 copy_process_state`) is genuine parity. ✔
- `init.rs:543`-`544`'s "setsid/ioctl/PTY/termios/fork are all production-safe on x86 already" is indeed false. ✔

---

## BINDING CONDITIONS

### C1 — BLOCKER. §3.1's safety rationale is aarch64-only and false on x86.

The spec's fix paragraph says: *"The PM lock's own interrupt-disable is
already the documented, sufficient synchronization — same rationale
`creation.rs:31–36`, `sys_spawn` (#713), and aarch64 fork itself all already
state."*

`kernel/src/process/mod.rs:208`-`231`: `manager()` masks interrupts **only**
under `#[cfg(target_arch = "aarch64")]` (`msr daifset, #0xf`, `:211`-`:214`;
restore in `Drop` at `:121`-`:128`, also aarch64-gated). The
`#[cfg(not(target_arch = "aarch64"))]` arm at `:225`-`:231` takes a bare
`spin::Mutex` with **no interrupt masking whatsoever**. The struct's own
doc-comment (`:65`-`:68`) says "On ARM64, acquiring PROCESS_MANAGER must
disable interrupts…" — the spec read that comment as arch-neutral.

Removing the wrapper on x86 is therefore a *strictly larger* change than the
one aarch64 made: aarch64 kept every PM window IRQ-off; x86 would make them
fully preemptible for the first time.

**Condition.** Do not ship the aarch64 rationale. Re-derive the safety
argument from what actually holds on x86 and record it in the PR:
(a) #713's landed precedent — `handlers.rs:2880`-`2886`, `sys_spawn`'s
Window 2 already holds the PM lock unmasked in production;
(b) an explicit audit that **every** x86 interrupt-context PM access is
non-blocking, which is true today and must be re-verified at implementation
time: `context_switch.rs:277`, `:601`, `:728`, `:1199`, `:1543` and
`interrupts.rs:726` all use `try_manager()`;
(c) the consequence of `try_manager()` failing while fork holds PM —
`context_switch.rs:277`-`287` refuses the dispatch and re-arms
`need_resched`, i.e. **nobody is scheduled at all while fork holds the PM
lock**. That is the fact the rest of the analysis has to be built on.

### C2 — BLOCKER. The un-gated CoW error path orphans the parent's page table and gets the parent killed.

`manager.rs:2352`-`2367`:

```rust
let mut parent_page_table = parent.page_table.take().ok_or(...)?;   // :2355-:2359
let pages_shared = super::fork::setup_cow_pages_with_vmas(...)?;    // :2360-:2367  <-- `?`
parent.page_table = Some(parent_page_table);                        // :2369-:2370  (restore)
```

Any `Err` from `setup_cow_pages_with_vmas` short-circuits **before** the
restore, leaving the live parent row with `page_table == None`, permanently.
`setup_cow_pages_with_vmas` has five reachable error returns
(`fork.rs:184`, `:211`, `:222`, `:236`, `:245`) — all frame/mapping
exhaustion shapes, i.e. exactly the ENOMEM case fork must survive.

The consequence is not a leak, it is a kill. `Process::cr3_value()`
(`process/process.rs:835`-`841`) returns `None` when `page_table` is `None`
and `inherited_cr3` is `None` (which it is — §3 item 4's own finding that a
forked parent/child never carries `inherited_cr3`). Both x86 dispatch
consumers respond by **terminating the thread**:
`context_switch.rs:742`-`767` and `:1268`-`:1288`
(`USERSPACE_DISPATCH_NO_CR3_REFUSED`, `[PMGUARD] no-cr3 dispatch refused`,
`thread.set_terminated()`, `switch_to_idle()`).

Today this arm is `#[cfg(feature = "testing")]`-only. De-gating it ships a
production path where a transient allocator failure inside fork kills the
*calling* process. The identical latent defect is in aarch64's
`fork_process_aarch64` (`manager.rs:2487`-`2500`) and in the two other x86
fork variants (`:2216`-`:2224`, `:3014`-`:3022`).

**Condition.** Restore the parent's page table on every exit path (scope
guard, or explicit put-back before the `?`), in this PR, on the x86 path at
minimum, with a delete-mutation proof that the fix is load-bearing. State
explicitly whether the aarch64 twin is fixed here or filed. Per
`ANY FAILURE YOU FIND IS YOUR PROBLEM`, "pre-existing behind a feature gate"
is not a disposition when the same PR is what makes it reachable.

### C3 — BLOCKER. §3 item 5's "already-proven-in-production machinery" is false. This is the #713-precheck class recurring verbatim.

The spec: *"the `ProcessPageTable`/CoW machinery this function already calls
(once de-gated) is the identical, already-proven-in-production machinery
`spawn_process`/`create_user_process` exercise today."*

`ProcessPageTable::new()` is shared with spawn. **The CoW half is not.**
Tree-wide:

- `make_cow_flags` has exactly two call sites: `fork.rs:218` and a unit test
  (`test_framework/registry.rs:413`).
- `setup_cow_pages_with_vmas` has exactly four: `manager.rs:2219`, `:2363`,
  `:2500`, `:3017`. The three x86 ones are all inside
  `#[cfg(feature = "testing")]` blocks; the aarch64 one (`:2500`) is the
  only production caller in the tree.
- `spawn_process` / `create_process_with_argv` / `create_user_process` call
  neither. They never create a CoW mapping.

Therefore **no CoW page has ever existed in an x86 zero-feature build**, and
the entire x86 CoW *fault* path has never executed in production:
`handle_cow_fault` (`interrupts.rs:702`), `handle_cow_with_manager` (`:745`),
`handle_cow_direct` (`:830`), the `find_process_by_cr3_mut` lookup (`:759`),
and the `frame_is_shared`/`frame_decref` refcount protocol
(`frame_metadata.rs:35`-`49`). It is compiled in (no feature gate) — which is
precisely the "compile-time presence is not runtime proof" trap the spec
itself names in §3 item 10 and then fails to apply here.

The `[COW FAULT #N]` lines in the in-repo evidence
(`docs/planning/green-program/x86-prod/serials/r3-baseline-…-serial_user.txt`)
are all from `boot_tests`/`testing` builds, confirming this.

**Conditions.**
1. Restate §3 item 5's verdict as **GAP (runtime-unproven)**, not "NOT A GAP".
2. `fork_smoke` must force a CoW write fault on **both** sides — the child
   *and* the parent must each write to a page that was writable before the
   fork — and the gate must assert the faults actually occurred (either the
   existing `[COW FAULT #…]` literal or, better, C11's counter). A child that
   only yields and exits proves nothing about `frame_is_shared` returning the
   right answer, and getting that wrong silently corrupts parent/child memory
   rather than faulting.
3. Decide, in this PR, whether `serial_println!` from inside the page-fault
   handler for the first 20 faults (`interrupts.rs:715`-`722`, `:735`) is
   acceptable in the production gate log now that it becomes reachable.

### C4 — BLOCKER. §3.1 and §3.2 are one indivisible edit; the reclaim call must not land inside a masked window.

`tests/teardown_structure.rs:12036` (`validate_reclaim_preempt_bracket`)
pins that `reclaim_deferred_process_resources` is a *preempt* bracket whose
body contains no interrupt masking, with the recorded reason (`:12005`-`:12009`)
that masking "would put page-table retirement, the process-manager lock and
row destructors in an IRQ-off window". Adding the call while the
`arch_without_interrupts` wrap at `handlers.rs:1868` still encloses it
reintroduces exactly what #653/#655 paid down.

Separately, `reclaim_deferred_process_resources` itself takes the PM lock
(`task/process_task.rs:673`, blocking `crate::process::manager()`). On x86
that lock is a bare spinlock (C1) — calling it with a guard live is an
immediate self-deadlock, not a lock-order warning.

**Condition.** §3.1's unwrap and §3.2's added call ship in the same commit,
in that order, with the call placed where no manager guard is live. The
structural test (§4d) must assert the *guard-liveness* property, not merely
"before `ProcessPageTable::new(`".

### C5 — BLOCKER. Unregistered landmine #1: `exec_lock_order_structure.rs`'s empty publication census.

`tests/exec_lock_order_structure.rs:1440`:
`const CREATION_PUBLICATION_UNDER_PM_GUARD: &[(&str, &str, usize)] = &[];`
— an **empty** expected-offender census, i.e. zero tolerance tree-wide.

`validate_creation_publications_release_process_manager` (`:1517`-`:1594`,
driven over `rust_sources_below("kernel/src")` at `:2274`) flags any function
containing both `crate::process::manager()` and `scheduler::spawn(`/
`spawn_front(`/`spawn_as_current(` where a publication occurs inside a
manager-binding scope that is not explicitly dropped first.
`sys_fork_with_parent_context` matches the candidate predicate **today** and
survives only because `drop(manager_guard)` at `handlers.rs:1951` is
lexically detectable by `guard_is_explicitly_dropped`.

**Conditions.** (a) The restructure must keep an explicit, textually
detectable `drop(...)` of every manager binding before every `spawn_front`.
(b) There is also a live runtime twin the spec overlooked:
`[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]`, emitted by
`account_creation_publication_pm_held` / `note_scheduler_publication`
(validated at `:1596`-`:1650`, predicate
`crate::process::process_manager_held_on_current_cpu()`,
`process/mod.rs:187`-`191`). §4's claim that "fork has no ExecSchedCommit-
shaped receipt to assert on … so the oracle here is structural/host-side
instead of a kernel print" is wrong — this *is* fork's publication-ordering
kernel print. Pin it at 0 in the gate.

### C6 — BLOCKER. Unregistered landmine #2: `init.rs::main()`'s call sequence is census-pinned.

`tests/green_program_envelope_structure.rs:238`
(`x86_tty_oracle_runs_with_no_persistent_background_process`) asserts
**exactly 0** persistent (spawn-without-waitpid) launchers appear before the
x86-gated `run_tty_oracle()` in `init.rs::main()`, walking the cfg-gated call
sequence with `main_call_sequence()` (`:90`) and `classify_launcher()`
(`:178`). The aarch64 twin at `:223` pins exactly 1.

§4b's slot ("after `run_exec_smoke()`, before `start_bsshd()`") is safe —
`run_fork_smoke()` lands after `run_tty_oracle()` in the x86 order
(`init.rs:123`-`130`). Any other placement (e.g. the equally plausible
"right after `run_spawn_smoke()`") reddens this test.

**Conditions.** (a) Place `run_fork_smoke()` strictly after
`run_tty_oracle()`; (b) it must contain a literal `waitpid(` in its own body
so `classify_launcher` returns `Reaped`, not `Persistent`; (c) update
`docs/planning/green-program/WORKLOAD-ENVELOPES.md:119`-`125`, which pins
both `init.rs` "lines 123-130" and "the concurrent userspace process set is
**exactly 2**".

### C7 — BLOCKER. §2's manager.rs scope is short; "~10 lines, two cfg-gate edits" undercounts.

`fork_process_with_parent_context` has **three** cfg blocks, not two:
- CoW block `#[cfg(feature = "testing")]` `:2348`-`:2384`
- refusal `#[cfg(not(feature = "testing"))]` `:2386`-`:2390`
- a **second** `#[cfg(feature = "testing")]` block `:2392`-`:2404` that sets
  `child_process.page_table = Some(child_page_table)` and makes the
  `self.complete_fork(...)` tail call.

The spec's range `:2348–:2394` stops inside the third block. Without
de-gating it the function has no tail expression.

It also carries four dishonest suppressions that become live-code lies once
the gate is gone and must be deleted per CLAUDE.md "honest fixes only":
`#[cfg_attr(not(feature = "testing"), allow(unused_variables))]` at `:2274`
and `:2281`, `#[cfg_attr(not(feature = "testing"), allow(unused_variables, unused_mut))]`
at `:2276`-`:2277`; plus `#[allow(dead_code)]` on
`setup_cow_pages_with_vmas` (`fork.rs:163`), which stops being dead code.

### C8 — BLOCKER. §3 item 3 declares the publication neighborhood clean by comparing fork only to fork. It is behind its own sibling syscall.

§3 item 3: *"NOT A GAP — verified identical, and correctly so… this is the
one area where x86 fork already matches aarch64's hardening."* Both fork
implementations match each other. Both are behind `sys_spawn`.

`sys_spawn` (`handlers.rs:2919`-`2941`) tears the child row down when
`main_thread` is missing at publish time — retains `parent.children`, then
`remove_from_ready_queue`, then `remove_process` — with the recorded reason
(`:2914`-`:2918`) that a dangling `children` entry makes the parent's later
`waitpid(-1)` block forever instead of returning `ECHILD`. That is #713
precheck C2, already landed on the production x86 creation path.

`sys_fork_with_parent_context`'s equivalent arms (`handlers.rs:1966`-`1972`)
log and return `ENOMEM` with the child row already inserted
(`manager.rs:2882`) and `parent.children` already pushed (`:2878`). aarch64
is the same (`manager.rs:2665`-`2668`, `syscall_entry.rs:997`).

**Condition.** Either apply the #713-C2 undo to fork's failure arms, or
record an explicit ruling (with the reason) that it is held — but do not
leave §3 item 3's "NOT A GAP" standing, because it is the sentence that
would cause implementation to skip this.

### C9 — MAJOR (parity-table omission #1). aarch64 fork's no-logging-under-PM invariant is absent from the table entirely.

aarch64 states it as a deadlock invariant in four places:
`syscall_entry.rs:899`-`905` (the postmortem the spec quotes — but it quotes
only the "no wrapper" half and drops the "no logging" half),
`manager.rs:2472`-`2474`, `manager.rs:2555`-`2556`, `fork.rs:172`-`174`.

x86's `fork_process_with_parent_context` + `complete_fork` execute roughly
twenty-five `log::info!`/`log::debug!`/`log::warn!`/`log::error!` calls
inside the PM-locked region — `manager.rs:2326`, `:2372`, `:2694`, `:2719`,
`:2737`, `:2778`, `:2782`, `:2805`, `:2810`, `:2814`, `:2822`, `:2828`,
`:2833`, `:2871`, `:2875`, `:2884`, `:2886`, `:2891`.

This matters *more* on x86 than aarch64, not less: the PM lock is a bare
spinlock (C1), and `check_need_resched_and_switch` refuses to schedule
anyone while it is held (`context_switch.rs:277`-`287`). A thread preempted
mid-`log::` can therefore never be scheduled to release the logger lock, so
fork spinning on that lock is a hard hang — on a gate that boots `-smp 1`
(`green_program_envelope_structure.rs:349`-`360`).

**Condition.** Either purge logging from the PM-locked region as aarch64
did, or write down the analysis for why x86 is exempt. Do not leave it
unaddressed while §6 budgets `manager.rs` at "~10 lines changed (two cfg-gate
edits, no new logic)".

### C10 — MAJOR (parity-table omission #2). TLS registration under the PM lock, x86-only.

x86 `complete_fork` calls `crate::tls::register_thread_tls(child_thread_id, child_tls_block)`
(`manager.rs:2718`). `complete_fork_aarch64` does not — it has no TLS
registration at all.

`tls.rs:225`-`246` nests, inside the PM window: a second global lock
(`TLS_MANAGER.lock()`), an unbounded `tls_blocks.push()` heap growth loop
indexed by thread id (`:232`-`:234`), and a `log::debug!` (`:239`). It has no
unregister counterpart on process exit, so every fork grows that vector
monotonically. The lock-order edge PM→TLS_MANAGER already exists on the
spawn path (`manager.rs:1325`, `:1412`), so this is not novel — but it is a
real per-fork cost the parity table does not mention.

### C11 — MAJOR (parity-table omission #3). Tracing/counter receipts exist on aarch64 and are missing on x86, and `count_fork()` is dead everywhere.

aarch64 fork emits four events: `trace_fork_entry` (`manager.rs:2423`),
`trace_stack_map` (`:2558`), `trace_fork_exit` (`:2675`), `trace_spawn_front`
(`syscall_entry.rs:974`). x86 fork emits none.

`tracing/providers/counters.rs:272 pub fn count_fork()` has **zero call
sites tree-wide**, so `get_process_counters()`'s `fork_total` (`:308`-`:315`)
and `/proc`'s `cow_faults` neighbour (`fs/procfs/mod.rs:838`) are dead on
both arches.

This directly contradicts §4's premise that fork has no kernel-side receipt
to assert on. Wiring `count_fork()` (and `count_cow_fault()` in the x86 CoW
handler) is a one-line-each, lock-free receipt that would let the gate prove
C3's CoW requirement numerically instead of by literal-grep.

### C12 — MAJOR (parity-table omission #4). Fork-style `clone()` routing diverges.

`libs/libbreenix/src/process.rs:39`-`53`: `fork()` issues `syscall0(FORK)` on
x86 but `syscall5(CLONE, 17 /*SIGCHLD*/, 0,0,0,0)` on aarch64. aarch64's
dispatcher routes a no-`CLONE_VM` clone to `sys_fork_aarch64`
(`syscall_entry.rs:150`-`157`). x86's `sys_clone` refuses it:
`clone.rs:65`-`67`, `log::warn!("clone: called without CLONE_VM, use fork instead")`.

So the aarch64 "reference implementation" is reached through a different
syscall number than the x86 one being fixed, and after #745 x86 `fork()`
works while `clone(SIGCHLD)` still does not. Worth one line in the table; it
also means any future musl/busybox userland on x86 stays broken.

### C13 — MAJOR (corrects §3.10). The newly-reachable production fork surface is far larger than "bsh's three call sites".

§3.10 says `/etc/init.js` "issues only the JS `spawn()` builtin, seven times,
zero fork-triggering paths". That is a category error: the spawned
*programs* fork.

- `start_bsshd()` runs unconditionally on x86 (`init.rs:129`, `:582`), and
  `bsshd` forks at three sites (`bsshd.rs:70` per-connection, `:144` shell,
  `:247` exec-request). Not hit at boot, but live production surface.
- `run_boot_script()` on x86 spawns `/bin/bsh --init-shell`
  (`init.rs:453`-`462`), whose `/etc/init.js`
  (`scripts/create_ext2_disk.sh:596`-`612`) spawns `/bin/bterm` and
  `/bin/bcheck`.
- **`bcheck` is a fork+exec test runner.** `run_test` (`bcheck.rs:379`-`386`)
  forks and execs one binary per test; `main` falls back to
  `run_headless()` when `Window::new` fails (`:426`-`:431`), and
  `run_headless` (`:526`-`:539`) runs the *entire* list. `bterm` forks at
  `:352`.
- The x86 image is built from `userspace/programs/*.elf`
  (`create_ext2_disk.sh:51`-`55`), which contains `bcheck.elf`, `bterm.elf`,
  `bwm.elf`, `bounce.elf`, `blog.elf`.

Today all of those fork calls take their error branch. After #745 they
succeed, so **every x86 prod boot gains a fork+exec test-suite storm inside
the gate's liveness window**. The gate deliberately does not pin the init.js
chain (`run-x86-prod-profile-boot-test.sh:892`-`893`) and
`LIVENESS_WINDOW_SECONDS=60` (`:556`) was sized (`:546`-`:555`) against the
pre-fork workload.

**Conditions.** (a) Disclose bsshd/bcheck/bterm, not just bsh's three sites.
(b) Treat §4c's "re-measure the liveness window" as mandatory, not
conditional, and measure it against the *post-fix* workload. (c) This is
exactly the #728 shape `WORKLOAD-ENVELOPES.md:4`-`8` was written about — a
cell declared green against an image that had never run the workload. Say so
in the PR and re-derive the affected envelope rows.

### C14 — MAJOR. §5 re-admits arm 14 on a premise the green-program doc still contradicts.

`docs/planning/green-program/WORKLOAD-ENVELOPES.md:137`-`138` still states arm
14 is excluded "because `exec()` is `ENOSYS` in the shipped x86 zero-feature
profile". That has been false since #721 landed (PR #747, merge `40b86653`);
production x86 exec is `handlers.rs:2562`-`2600`. `:198`-`:203` repeats it for
the blended cell.

Fix the doc in this arc rather than leaving a known-false green-program claim
standing while §5 builds a round on its negation.

### C15 — MINOR. §1's call-site table is incomplete (does not change §1's conclusion).

`fork_process` also has kernel call sites the table omits: `test_exec.rs:125`,
`test_exec.rs:394` (both via `with_process_manager`), `userspace_test.rs:835`,
and `manager.rs:2123` (`fork_process`'s own body delegating to
`fork_process_with_context`). All `testing`-gated, so §1's "only
`fork_process_with_parent_context` + `complete_fork` need de-gating" survives
— but state the census completely so a future reader can re-derive it.

### C16 — MINOR. Anti-vacuity: the pre-fix baseline leaks a PID per fork call.

`allocate_ordinary_pid()` runs at `manager.rs:2325`, *before* the
`#[cfg(not(feature = "testing"))]` refusal returns at `:2386`-`:2390`. So
today every refused fork still burns a PID. §4e's negative-control run
against the pre-fix tree will show PID skew relative to the post-fix run;
record it so it is not later mistaken for a regression the fix introduced.

---

## Notes for the round plan

- §6's Round-1 self-assessment ("marginal value is smaller here … *because*
  that landmine is already found") did not hold: three further host tests
  (C4/C5/C6) pin the exact shapes this fix changes, two of them with
  zero-tolerance censuses.
- §4d's structural test is the right instrument. Add to its assertion list:
  no manager guard live across either reclaim call (C4/C16), an explicit
  `drop(...)` before every `spawn_front` (C5), and page-table restoration on
  every CoW error exit (C2) — each with its own delete-mutation proof, per
  #721 review M1.
- Verification commands in §6 are correct for this tree; add
  `--test exec_lock_order_structure --test green_program_envelope_structure`
  to the `cargo test` line.
