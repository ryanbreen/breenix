# Critical-path logging drain, PR-1: the x86 dispatch path's lock-held prints

**Branch:** `sched/chk-pr1-x86-dispatch-prints`
**Base:** `origin/main` `a0ec6cf8` ("Merge pull request #887 from ryanbreen/docs-822-true-main-verify")
**Plan:** `docs/planning/green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md`, §7 PR-1 row
**Mode:** R157 small-PR ratchet mode. R191.

PR-1 of the drain plan deletes the 16 H1 sites the classification table names in
`kernel/src/interrupts/context_switch.rs` -- a blocking serial acquisition on the
interrupt-return dispatch path, at a point where `PROCESS_MANAGER` is held or
where interrupts are disabled -- and leaves each arm publishing the same fact
through a lock-free counter that a thread-context reporter emits.

---

## 1. Re-derivation at the branch point

The plan's snapshot is `783a6a53`; this branch is cut from `a0ec6cf8`.

| Fact | At `783a6a53` (plan) | At `a0ec6cf8` (re-derived) |
|---|---|---|
| `bash scripts/check-critical-path-violations.sh` | exit 1, 274 stdout lines, 9 `VIOLATION` headers | exit 1, **275** stdout lines, 9 headers |
| `interrupts/context_switch.rs` flagged lines | 30 | 30 |
| the 16 H1 line numbers | `:458 :649 :656 :662 :837 :980 :1030 :1079 :1092 :1111 :1194 :1232 :1337 :1429 :1499 :1512` | identical |
| census total | 135 sites / 9 files | 135 sites / 9 files |

The one-line difference in the shell script's stdout is upstream of this PR and
outside `context_switch.rs`; the 135-site census and the 16 line numbers are
unchanged, so PR-1's slice re-derives exactly.

**One thing did NOT re-derive: the 9/7 split.** The plan's PR-1 text says nine
of the sixteen already publish the fact as a `DispatchAbandonSite` counter and
seven do not. Reading each arm says **6 and 10**:

* The plan groups `:980`, `:1030` and `:1111` with `IdleSignalTerminatedBlocked`
  because that `trace_dispatch_abandon` sits a few lines below them. It counts
  the signal-TERMINATION arm. Those three are the signal-DELIVERY arms; they
  complete the dispatch and reach no abandon site at all, so deleting their
  prints without a counter would have dropped the fact. They move from "already
  counted" to "needs a counter".
* The correction runs the other way for `:1092`: the plan lists it among the
  seven needing a counter, and it is in the same `match` arm as `:1079`, which
  `IdleSignalTerminatedBlocked` already counts. A second counter there could not
  ever differ from the first, so it gets no counter of its own. What IS lost is the
  `n.parent_pid` value the print carried; a relaxed counter carries occurrence,
  not values. Recorded under §7.

Net: **6 deletions with no new counter, 10 deletions with one new counter each.**
16 total either way, and the `After: 119` the plan predicts is unchanged.

---

## 2. The 16 sites

Line numbers are at `a0ec6cf8`, before this PR. "IF" is the interrupt flag at the
call. 16 of the 16 are on the interrupt-return dispatch path reached from
`timer_entry.asm` or `syscall/entry.asm`, so IF=0 at 16 of 16.

| # | Line | Level | Enclosing fn | Lock context | What it printed | Disposition | Emitted now by | Marker parity |
|---|---|---|---|---|---|---|---|---|
| 1 | `:458` | error | `check_need_resched_and_switch` | `process_manager_guard` held (taken at `:348`, moved at `:507`) | `Context switch aborted: failed to save thread {} context. Would cause return to stale RIP!` | delete only | `trace_dispatch_abandon(DispatchAbandonSite::RollbackSaveFailed)` on the next line | 0 greps |
| 2 | `:649` | error | `save_current_thread_context_with_guard` | called at `:448` with `&mut process_manager_guard`; PM held | `Process {} has no main_thread for thread {}` | **new counter** | `DispatchLogFact::SaveNoMainThread` → `save_no_thread=` | 0 greps |
| 3 | `:656` | error | `save_current_thread_context_with_guard` | PM held | `Could not find process for thread {} in process manager` | **new counter** | `DispatchLogFact::SaveProcessNotFound` → `save_no_proc=` | 0 greps |
| 4 | `:662` | error | `save_current_thread_context_with_guard` | PM held, inner option unset | `Process manager is None` | **new counter** | `DispatchLogFact::SaveManagerNone` → `save_no_pm=` | 0 greps. claim-lint:ok: the printed-string cell quotes 1 of 1 line, `kernel/src/interrupts/context_switch.rs:662` at `a0ec6cf8`, verbatim. |
| 5 | `:837` | error | `switch_to_thread` | owns `process_manager_guard` (moved at `:920`/`:1214`) | `Failed to switch TLS for thread {}: {}` | delete only | `DispatchAbandonSite::RollbackTls` two lines below | 0 greps |
| 6 | `:980` | info | `switch_to_thread` | inside `if let Some(mut manager_guard)`; PM held | `Thread {} has pending signals - delivering via saved userspace context` | **new counter** | `DispatchLogFact::SignalPendingBlocked` → `sig_pending_blocked=` | 0 greps |
| 7 | `:1030` | info | `switch_to_thread` | PM held | `Restored userspace context for signal delivery: RIP=… RSP=… RAX=-EINTR` | **new counter** | `DispatchLogFact::SignalContextBlocked` → `sig_ctx_blocked=` | 0 greps |
| 8 | `:1079` | info | `switch_to_thread` | PM held | `Signal terminated process, thread {}` | delete only | `DispatchAbandonSite::IdleSignalTerminatedBlocked` at the end of the same arm | 0 greps |
| 9 | `:1092` | debug | `switch_to_thread` | PM held | `Signal termination in blocked_in_syscall path: parent {} will be notified when resumed` | delete only | same arm, same `IdleSignalTerminatedBlocked`; `parent_pid` not preserved (§7) | 0 greps |
| 10 | `:1111` | info | `switch_to_thread` | PM held | `Signal delivered to thread {}` | **new counter** | `DispatchLogFact::SignalDeliveredBlocked` → `sig_delivered_blocked=` | 0 greps |
| 11 | `:1194` | error | `switch_to_thread` | PM guard **not** held -- this is the arm reached when it could not be taken; a blocking `SERIAL2` acquisition in interrupt context with IF=0 | `Failed to acquire lock to restore kernel context for thread {}. Context switch aborted.` | delete only | `DispatchAbandonSite::RollbackKernelContextLock` six lines below | 0 greps |
| 12 | `:1232` | error | `setup_idle_return` | `SCHEDULER` already released by the `with_scheduler` closure; 11 of its 13 call sites are inside a live PM guard scope | `Failed to get idle thread's kernel stack!` | **new counter** | `DispatchLogFact::IdleStackMissing` → `idle_no_stack=` | 0 greps |
| 13 | `:1337` | error | `setup_kernel_thread_return` | called from `switch_to_thread` at `:871`/`:882`, which still owns its PM guard | `KTHREAD_SWITCH: Failed to get thread info for thread {}` | **new counter** | `DispatchLogFact::KernelThreadInfoMissing` → `kthread_no_info=` | 0 greps |
| 14 | `:1429` | error | `restore_userspace_thread_context` | inside the `manager_guard` binding; PM held | `Refusing userspace restore of kernel frame for thread {}: saved CS=…` | delete only | `DispatchAbandonSite::IdleRestoreError` at the end of the error match, **plus** the `raw_serial_str("<KFRAME>")` marker already beside it, which is what splits this variant out of the other two | 0 greps |
| 15 | `:1499` | error | `restore_userspace_thread_context` | PM held | `ERROR: Userspace thread {} has no kernel stack!` | **new counter** | `DispatchLogFact::UserKernelStackMissing` → `user_no_kstack=` | 0 greps |
| 16 | `:1512` | debug | `restore_userspace_thread_context` | PM held | `Signal delivery check: process {} (thread {}) has deliverable signals` | **new counter** | `DispatchLogFact::SignalDeliverableUser` → `sig_deliverable_user=` | 0 greps |

### Marker parity, measured

Each of the 16 format strings was grepped across `scripts/`, `tests/`, `docker/`
and `xtask/`:

```
$ for s in "Context switch aborted" "has no main_thread for thread" \
    "Could not find process for thread" "Process manager is None" \
    "Failed to switch TLS for thread" "has pending signals - delivering" \
    "Restored userspace context for signal delivery" "Signal terminated process, thread" \
    "Signal termination in blocked_in_syscall path" "Signal delivered to thread" \
    "Failed to acquire lock to restore kernel context" \
    "Failed to get idle thread's kernel stack" "KTHREAD_SWITCH: Failed to get thread info" \
    "Refusing userspace restore of kernel frame" "Userspace thread {} has no kernel stack" \
    "Signal delivery check: process"; do
    grep -rIl -F "$s" scripts tests docker xtask; done
(no output -- 16 of 16 strings: 0 files)
```

0 of the 16 is a gate marker, so 0 of them needed moving to the reporter. The
`<KFRAME>` raw-serial marker beside site 14 is NOT one of the 16 and is kept
exactly as it was.

---

## 3. What replaced them

`kernel/src/task/dispatch_strand_census.rs` (x86-only, `#[cfg(target_arch =
"x86_64")] pub(crate) mod` in `kernel/src/task/mod.rs`) gains:

* `enum DispatchLogFact` -- 10 variants, `#[repr(usize)]`, discriminants indexing
  a fixed array. A **sibling** of `tracing::providers::sched::DispatchAbandonSite`,
  not an extension of it: 4 of the 10 arms (`SignalPendingBlocked`,
  `SignalContextBlocked`, `SignalDeliveredBlocked`, `SignalDeliverableUser`) do
  not abandon the dispatch, and folding them into that enum would have added
  them to the `DISPATCH_SWITCH_IDLE_REDIRECT` aggregate whose contributing arms
  that counter's own documentation enumerates.
* `static DISPATCH_LOG_FACTS: [AtomicU64; 10]` and
  `#[inline(always)] fn note_fact(fact)` -- one `fetch_add(1, Ordering::Relaxed)`.
  claim-lint:ok: the attribute is quoted from 1 of 1 declaration of `note_fact`
  in `kernel/src/task/dispatch_strand_census.rs`.
  No lock, no allocation, no formatting, no I/O. Pinned to that exact shape by
  `tests/dispatch_fact_census_structure.rs::note_fact_is_one_relaxed_atomic_add`.
* Ten named fields appended to the census line, rendered by a `Display` impl as
  `:<name>=<value>`.

The emission boundary is unchanged: `report_heartbeat_if_due()` still returns
early unless `crate::arch_interrupts_enabled()`, and the four callers are still
the four `tests/dispatch_strand_census_structure.rs` pins.

### The census line

Before (8 fields):

```
[DISPATCH_STRAND_CENSUS:seq=1:tick=200:ms=1000:saved=11:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
```

After (8 + 10):

```
[DISPATCH_STRAND_CENSUS:seq=1:tick=200:ms=1000:saved=11:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0:save_no_thread=0:save_no_proc=0:save_no_pm=0:sig_pending_blocked=0:sig_ctx_blocked=0:sig_delivered_blocked=0:idle_no_stack=0:kthread_no_info=0:user_no_kstack=0:sig_deliverable_user=0]
```

The 8 fields `scripts/x86-strand-census.sh` reads keep their names, order and
position. That tool's shape check now ACCEPTS a tail of further `name=digits`
fields rather than REQUIRING them, because the committed round-4 captures of
#775 under `docs/planning/green-program/sockets/serials/775/` carry the
eight-field form and are replayed verbatim by `tests/x86_gate_verdict_test.rs`;
a check demanding eighteen fields would score those real captures malformed.
What holds the 10 new fields on live bytes is the gate's `DISPATCH_FACT_ORACLE`
pin, not the regex. Both directions are measured in §5.

---

## 4. Why this edit belongs in a Tier-2 file

`kernel/src/interrupts/context_switch.rs` is Tier 2 ("context switch path --
timing sensitive").

* **The defect lives here.** The prohibited acquisition IS these 16 call sites.
  There is no other file in which to repair them.
* **The change removes work from the path**, which is the direction Tier 2 asks
  for: 16 blocking `SERIAL2` acquisitions with argument formatting become 10
  relaxed `fetch_add`s and 6 nothing-at-alls.
* **No new lock, allocation, formatting or I/O.** The whole diff to this file is
  deletions plus 10 `note_fact(...)` calls and one `use`. Verified three ways:
  `scripts/check-x86-dispatch-no-alloc.sh` on the built kernel (§6),
  `tests/dispatch_path_lock_free_structure.rs` at source, and
  `tests/dispatch_fact_census_structure.rs::note_fact_is_one_relaxed_atomic_add`
  on the callee.
* **Diagnostic vs repair.** 0 lines were added in order to OBSERVE the path. The
  10 counters are the replacement publication for facts the file already
  published; that is the repair, not instrumentation of it.
* **It lands with boot evidence, not a build.** §6.

---

## 5. Oracles and mutations

### 5.1 Forced legs, per new counter

`run_x86_dispatch_fact_oracle` (`kernel/src/test_framework/registry.rs`,
`#[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]`), dispatched once
from the x86 `boot_tests` gate block in `kernel/src/main.rs`.
claim-lint:ok: the attribute is quoted from 1 of 1 declaration of that function.
The steps are:

1. force a census snapshot (the **before** line),
2. drive 10 legs -- `note_fact` once per variant, in discriminant order,
3. read the 10 counters,
4. force a census snapshot (the **after** line),
5. print `[DISPATCH_FACT_ORACLE:x86:facts=10:legs=10:moved_by_one=N:moved_wrong=M:irqs_enabled_before=B:VERDICT]`.

`moved_by_one=10` is "each counter moved by EXACTLY the number of legs driven
against it". `moved_wrong=0` is the independence claim -- ten counters aliasing
one cell would report `moved_by_one=1`. `irqs_enabled_before=1` says the read
went through the reporter's real emission boundary.

The observed before/after pair from the boot-tests gate capture is quoted in
§6.2.

### 5.2 What the oracle does NOT show, and what does

It does not show that any site in `context_switch.rs` calls `note_fact`. It
drives the counters directly, because 7 of the 10 arms are defensive arms this
tree cannot reach on a running kernel:

| Fact | Why no forced leg drives the real arm |
|---|---|
| `SaveNoMainThread` | requires a process row whose `main_thread` is unset while a userspace thread of that process is being preempted |
| `SaveProcessNotFound` | requires a userspace thread with no process row in the manager |
| `SaveManagerNone` | requires the manager option unset after `process::init`, which does not happen again once that call has returned |
| `IdleStackMissing` | requires the idle thread to have no `kernel_stack_top` |
| `KernelThreadInfoMissing` | requires the thread to vanish from the scheduler between the dispatch decision and the return setup |
| `UserKernelStackMissing` | requires a live userspace thread with no kernel stack |
| `SignalDeliverableUser` / the 3 `Signal*Blocked` | REACHABLE -- they are ordinary signal-delivery arms; see §6.2 for what the gate boot observed |

Reaching the first six would mean injecting a fault into a Tier-2 dispatch path,
which this PR's scope excludes. The site-to-counter binding is therefore pinned
at source, per site, by `tests/dispatch_fact_census_structure.rs`:
`SITE_PUBLICATIONS` carries one row per deleted site naming its enclosing
function, its publication and how many of that publication that function
carries. 0 line numbers appear in that suite.

### 5.3 Mutation: re-insert one deleted print

```
$ python3 -  # re-insert log::error!("KTHREAD_SWITCH: Failed to get thread info for thread {}", thread_id)
             # into setup_kernel_thread_return, beside the note_fact that replaced it
$ bash scripts/run-structure-tests.sh critical_path_logging_census_structure
```

**Exit 101.** `test result: FAILED. 4 passed; 6 failed`. The assertion:

```
thread 'critical_path_log_census_is_pinned' panicked at
tests/critical_path_logging_census_structure.rs:818:5:
assertion `left == right` failed
  left: Err(["+ kernel/src/interrupts/context_switch.rs :: fn setup_kernel_thread_return  (1 occurrences, expected none)"])
 right: Ok(())
```

and, from the new total pin,

```
thread 'the_anchor_table_sums_to_the_ledger_total' panicked at ...:840:5:
assertion `left == right` failed: the tree carries 120 denylisted call sites, the drain ledger says 119
```

Reverted; the suite is exit 0 again.

### 5.4 Mutation: un-widen the strand tool's shape check

```
$ sed -i '' 's/:ledger_overflow=\[0-9\]+(:\[a-z_\]+=\[0-9\]+)\*/:ledger_overflow=[0-9]+/' scripts/x86-strand-census.sh
$ /tmp/x86gv the_census_reads
test result: FAILED. 0 passed; 1 failed; 22 filtered out
```

The widening is load-bearing: without it the whole eighteen-field population
scores malformed. Reverted; 23 of 23 pass.

### 5.5 The census ratchets, moved consciously

`tests/critical_path_logging_census_structure.rs`:

| Anchor (`interrupts/context_switch.rs`) | Before | After |
|---|---|---|
| `fn check_need_resched_and_switch` | 2 | 1 |
| `fn restore_userspace_thread_context` | 8 | 5 |
| `fn save_current_thread_context_with_guard` | 4 | 1 |
| `fn save_kthread_context` | 1 | 1 |
| `fn setup_first_userspace_entry` | 3 | 3 |
| `fn setup_idle_return` | 2 | 1 |
| `fn setup_kernel_thread_return` | 1 | **row removed** (reached 0) |
| `fn switch_to_thread` | 9 | 2 |
| file total | 30 | 14 |
| **census total** | **135** | **119** |
| wider census (adds `serial_print!`, `log_serial_print!`, `log::log!`) | 136 | 120 |

`tests/dispatch_strand_census_structure.rs` record histogram: 30 → 14, `debug 2
/ error 11 / info 8 / trace 9` → `error 1 / info 4 / trace 9`. The 2 debug, 10
error and 4 info removed are exactly the 16.

A new `the_anchor_table_sums_to_the_ledger_total` pins 119 as its own number
against both the anchor table and the tree, so a PR that shuffles rows without
moving the total, or moves the total without saying so, fails on the number the
drain plan is written in.

### 5.6 `scripts/check-critical-path-violations.sh`

| | exit | stdout lines | `VIOLATION` headers |
|---|---|---|---|
| before (`a0ec6cf8`) | 1 | 275 | 9 |
| after | 1 | 259 | 9 |

−16, one per deleted site. 16 of the 16 are `log::*!`, each matching exactly one
denylist pattern, so the script's 3×/2× report inflation does not apply to them.
The script still exits 1 because 119 sites remain; PR-11 of the drain plan is
where it reaches 0.

---

## 6. Gates

### 6.1 x86, on beast (`breenix-x86`, clone `/root/breenix-chk1`, `BREENIX_GATE_TMP=/root/breenix-chk1-tmp`)

| Gate | Command | Result |
|---|---|---|
| build, `boot_tests` profile | `cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi` | exit 0, **0 lines matching `^(warning|error)`** |
| dispatch-path no-alloc guard | `./scripts/check-x86-dispatch-no-alloc.sh` | exit 0 — `PASS: 0 allocating call targets in 3 in-scope symbol(s), 14 edge(s) checked.` |
| critical-path checker | `bash scripts/check-critical-path-violations.sh` | exit 1, 259 lines (275 before) |
| boot tests | `./docker/qemu/run-x86-boot-tests.sh 1` | **exit 0** — `x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0`, `x86 frame-custody gate run 1: PASS` |
| build, `testing,external_test_bins` profile (what the parallel gate boots) | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | exit 0, **0 lines matching `^(warning\|error)`** |
| production profile | `./docker/qemu/run-x86-prod-profile-boot-test.sh` | **exit 0** — `PASS: x86 production profile reached steady state with the teardown census at rest` |
| parallel boots | `./docker/qemu/run-boot-parallel.sh 5` | **exit 0** — `Results: 5 passed, 0 failed out of 5`, each `x86 userspace gate: PASS - exited=23 expected>=17 nonzero=0 allowlist=0` |

**A false start on the parallel gate, and what it was.** The first attempt
reported `Test 1: TIMEOUT` / `Test 2: TIMEOUT`. It was a sequencing error in
this round's runner script, not a reading about the branch:
`run-boot-parallel.sh` does not build — it picks the NEWEST
`target/release/build/breenix-*/out/breenix-uefi.img` — and the runner had run
`run-x86-prod-profile-boot-test.sh` just before it, leaving a ZERO-FEATURE
image as the newest. That kernel carries no `testing` feature and therefore no
kthread-join test, so the marker the gate waits for cannot appear. Rebuilding
`--features testing,external_test_bins` (0 warnings), repacking
`test_binaries.img` and `ext2.img`, and re-running gave 5/5. The timed-out
captures are `breenix_boot_*` from the first attempt and carry 0 of the marker
in question; no boot of the branch's own testing kernel failed.

### 6.2 The forced legs, on live bytes

From `/root/breenix-chk1-tmp/breenix_x86_boot_tests_1/`. The two forced
snapshots are lines 700 and 701 of `serial_kernel.txt` — adjacent, 3 ms apart,
with the ten legs between them:

**Before** (`seq=2`, line 700):

```
[DISPATCH_STRAND_CENSUS:seq=2:tick=29:ms=1060:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0:save_no_thread=0:save_no_proc=0:save_no_pm=0:sig_pending_blocked=0:sig_ctx_blocked=0:sig_delivered_blocked=0:idle_no_stack=0:kthread_no_info=0:user_no_kstack=0:sig_deliverable_user=0]
```

**After** (`seq=3`, line 701):

```
[DISPATCH_STRAND_CENSUS:seq=3:tick=29:ms=1063:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0:save_no_thread=1:save_no_proc=1:save_no_pm=1:sig_pending_blocked=1:sig_ctx_blocked=1:sig_delivered_blocked=1:idle_no_stack=1:kthread_no_info=1:user_no_kstack=1:sig_deliverable_user=1]
```

10 of 10 fields moved by exactly 1, and no other field of the line moved. The
oracle's own verdict, from `serial_user.txt`:

```
[DISPATCH_FACT_ORACLE:x86:facts=10:legs=10:moved_by_one=10:moved_wrong=0:irqs_enabled_before=1:PASS]
```

### 6.3 A natural leg the boot produced on its own

The **last** census line of the same boot, `seq=384` at `ms=477485`, after 384
snapshots:

```
[DISPATCH_STRAND_CENSUS:seq=384:tick=52085:ms=477485:saved=11:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0:save_no_thread=1:save_no_proc=1:save_no_pm=1:sig_pending_blocked=1:sig_ctx_blocked=1:sig_delivered_blocked=1:idle_no_stack=1:kthread_no_info=1:user_no_kstack=1:sig_deliverable_user=4]
```

9 of the 10 sit at exactly the oracle's 1 — the boot reached those arms 0 times,
which is what §5.2 predicts for the defensive ones. `sig_deliverable_user` is
**4**: 1 from the oracle plus **3 real dispatch-path occurrences**. That is
site 16 (`:1512`, the deleted `log::debug!` in
`restore_userspace_thread_context`) firing three times on this boot and being
counted. It is one of two legs in this PR that is driven by the real site through the
real path, and it is the boot itself that drove it, not a probe.

The other is stronger, because the oracle is not compiled into it at all. The
**zero-feature production profile** carries no `boot_tests`, so
`run_x86_dispatch_fact_oracle` does not exist in that kernel and contributes 0.
The last census line of `./docker/qemu/run-x86-prod-profile-boot-test.sh`:

```
[DISPATCH_STRAND_CENSUS:seq=55:tick=11670:ms=62534:saved=12:stranded=6:tids=5,15,17,19,20,22:tid_overflow=0:ledger_overflow=0:save_no_thread=0:save_no_proc=0:save_no_pm=0:sig_pending_blocked=0:sig_ctx_blocked=0:sig_delivered_blocked=0:idle_no_stack=0:kthread_no_info=0:user_no_kstack=0:sig_deliverable_user=3]
```

`sig_deliverable_user=3` with 0 oracle contribution: three occurrences of site
16, in the shipped kernel, with no probe of any kind in the binary. The other
nine read 0, which is the same shape §6.2's forced legs and §5.2's
reachability table predict.

And the `testing,external_test_bins` profile, which also has no `boot_tests`
and so also has no oracle: 5 of 5 parallel boots
(`./docker/qemu/run-boot-parallel.sh 5`) end with

```
:save_no_thread=0:save_no_proc=0:save_no_pm=0:sig_pending_blocked=0:sig_ctx_blocked=0:sig_delivered_blocked=0:idle_no_stack=0:kthread_no_info=0:user_no_kstack=0:sig_deliverable_user=3]
```

`sig_deliverable_user=3` on 5 of 5, matching the production profile's 3
exactly, and 0 on the other nine. The site fires a reproducible three times per
boot across three profiles and six independent boots.

**A reading in that line that is NOT this PR's:** `stranded=6:tids=5,15,17,19,20,22`.
That is the pre-existing strand ledger -- `note_save`/`note_restore`/`note_exit`
-- and this PR changes 0 of the three call sites that write it. The production
profile is not scored on the strand census (`run-x86-prod-profile-boot-test.sh`
does not invoke `scripts/x86-gate-verdict.sh`), so it is not a gate result
either way. Whether the same reading appears on `origin/main` is measured in
§6.5 rather than asserted here.

### 6.4 Serial-side absence of the 16 deleted strings

Across both capture files of the same boot, the 16 deleted format strings
appear **0 times**:

```
$ grep -h -c -e 'Context switch aborted' -e 'has no main_thread for thread' \
    -e 'Could not find process for thread' -e 'Process manager is None' \
    -e 'Failed to switch TLS for thread' -e 'has pending signals - delivering' \
    -e 'Restored userspace context for signal delivery' \
    -e 'Signal terminated process, thread' \
    -e 'Signal termination in blocked_in_syscall path' \
    -e 'Signal delivered to thread' \
    -e 'Failed to acquire lock to restore kernel context' \
    -e "Failed to get idle thread's kernel stack" \
    -e 'KTHREAD_SWITCH: Failed to get thread info' \
    -e 'Refusing userspace restore of kernel frame' -e 'has no kernel stack!' \
    -e 'Signal delivery check: process' serial_*.txt
0
0
```

384 census snapshots in the same capture, so the reporter is not silent — the
prints are.

`docker/qemu/run-x86-boot-tests.sh` now carries that same grep as a gate
assertion at `-eq 0`, next to the exact-literal pin of the oracle verdict and
its emission count of 1.

The same grep over the 10 capture files of the five `run-boot-parallel.sh`
boots also returns `0` on each of the 10.

### 6.5 A main baseline for the one reading this PR does not own

§6.3 reports `stranded=6:tids=5,15,17,19,20,22` in this branch's
production-profile census line. This PR changes 0 of the three call sites that
write that ledger, but it does change dispatch-path TIMING materially -- 16
blocking UART writes removed from an interrupt-return path -- so "unchanged
code" is not on its own an argument that the reading is unchanged. It was
measured instead.

A second clone of this container's `/root/breenix` was checked out at
`a0ec6cf8` (this branch's base, `origin/main`) and run through the same
`./docker/qemu/run-x86-prod-profile-boot-test.sh`, under
`BREENIX_GATE_TMP=/root/breenix-chk1-base-tmp`:

```
a0ec6cf84 Merge pull request #887 from ryanbreen/docs-822-true-main-verify
BASE_PROD_EXIT=0
[DISPATCH_STRAND_CENSUS:seq=41:tick=9265:ms=50227:saved=12:stranded=7:tids=5,15,17,19,20,22,23:tid_overflow=0:ledger_overflow=0]
PASS: x86 production profile reached steady state with the teardown census at rest
```

| | `origin/main` `a0ec6cf8` | this branch |
|---|---|---|
| gate verdict | exit 0, `PASS: … teardown census at rest` | exit 0, same line |
| `saved` | 12 | 12 |
| `stranded` | **7** | **6** |
| `tids` | `5,15,17,19,20,22,23` | `5,15,17,19,20,22` |
| census line shape | 8 fields | 8 + 10 |

The reading is pre-existing: main carries it too, at one TID MORE, and this
branch's stranded set is a subset of main's. Neither run is scored on it
(`run-x86-prod-profile-boot-test.sh` does not invoke
`scripts/x86-gate-verdict.sh`), and 1 boot per side is not a distribution, so
this pair says the reading is not introduced here -- it does not say the
difference of one TID means anything.

### 6.6 aarch64, on this Mac

`kernel/src/interrupts/context_switch.rs` carries `#![cfg(target_arch =
"x86_64")]` at its head; `kernel/src/task/dispatch_strand_census.rs` is behind
`#[cfg(target_arch = "x86_64")] pub(crate) mod` in `kernel/src/task/mod.rs`;
`kernel/src/main.rs` is the x86 entry point; and the oracle in
`kernel/src/test_framework/registry.rs` is
`#[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]`.
claim-lint:ok: 4 of 4 changed kernel files, each gate quoted from its own head.

```
$ git diff origin/main -- kernel/src/arch_impl/ kernel/src/main_aarch64.rs \
                          kernel/src/per_cpu_aarch64.rs kernel/src/task/scheduler.rs | wc -l
0
```

The aarch64 diff is empty. The gates were run anyway:

| Gate | Result |
|---|---|
| `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | exit 0; the grep for unused-item and error lines returned 0 |
| `./scripts/check-kernel-no-neon.sh target/aarch64-breenix-kernel/release/kernel-aarch64` | exit 0 — `PASS: 0 FP/SIMD load/store instructions in kernel .text` |
| `./docker/qemu/run-aarch64-boot-test-strict.sh` (script default, 20 iterations) | exit 0 — `PASS: 20/20 boots succeeded` |

### 6.7 Host-side suites

| Suite | Result |
|---|---|
| `scripts/run-structure-tests.sh <stem>` over every `tests/*_structure.rs` | **48/48 green** (47 before this PR, plus the new `dispatch_fact_census_structure`) |
| `tests/x86_gate_verdict_test.rs` (`rustc --test`, replays the committed #775 captures) | **23/23 green** (21 before, plus the 2 new shape legs) |
| `python3 scripts/claim-lint.py` | exit 0 — `clean (10 file(s) checked, changed hunks vs a0ec6cf8473d)` |
| `python3 scripts/test_claim_lint.py` | exit 0 |

---

## 7. What this PR does NOT claim

* **It does not claim a forced leg for 6 of the 10 new counters.**
  `SaveNoMainThread`, `SaveProcessNotFound`, `SaveManagerNone`,
  `IdleStackMissing`, `KernelThreadInfoMissing` and `UserKernelStackMissing`
  are driven only by the oracle, which calls `note_fact` directly. Their arms
  are defensive arms this tree cannot reach on a running kernel (§5.2), and
  reaching them would mean injecting a fault into a Tier-2 dispatch path. The
  site-to-counter binding for those 6 is a SOURCE-level pin, not a runtime
  measurement.
* **It does not claim a natural leg for 3 of the 4 reachable arms.** Of
  `SignalPendingBlocked`, `SignalContextBlocked`, `SignalDeliveredBlocked` and
  `SignalDeliverableUser`, only the last was observed firing on the gate boot
  (§6.3). The other three sit on the blocked-in-syscall signal-delivery arm,
  which this gate's userspace roster did not reach in the observed boot. Their
  counters read exactly the oracle's 1.
* **`n.parent_pid` is not preserved.** Site 9 (`:1092`) printed the parent PID a
  signal-terminated process would notify. Its ARM is counted by
  `IdleSignalTerminatedBlocked`; the pid VALUE has 0 publications now. A
  relaxed counter carries occurrence, not values, and adding a value store on
  this path was not in scope.
* **It does not claim the file is clean.** 14 of the original 30 flagged calls
  remain in `interrupts/context_switch.rs`: 9 `log::trace!` (H3 — dropped by
  `CombinedLogger::log` before any lock, so 0 bytes today, but their format
  arguments are still evaluated on each dispatch and they are one logger
  change away from being a live acquisition), 4 `log::info!` and 1
  `log::error!` that the plan classifies H2/H3 and hands to its PR-5 and PR-11.
  The whole-tree census is 119, not 0, and
  `scripts/check-critical-path-violations.sh` still exits 1.
* **It does not claim a deadlock was fixed.** The plan's H1 class is a SHAPE —
  a blocking serial acquisition where `PROCESS_MANAGER` is held or interrupts
  are off — plus two measurable costs (the lock hold extended across UART
  output, and `_log_print`'s `.expect(...)` panicking while holding `SERIAL2`).
  No AB-BA cycle through `SERIAL2` exists in this tree, and 0 were observed.
  This PR removes 16 instances of the shape; it does not report a defect it
  reproduced.
* **It does not claim the strand tool now validates the ten new fields.**
  `scripts/x86-strand-census.sh` ACCEPTS a `name=digits` tail; it does not
  require one, and it does not read any of the ten. What requires them on live
  bytes is the boot gate's exact-literal `DISPATCH_FACT_ORACLE` pin. A trailing
  field with a non-decimal value is still malformed
  (`tests/x86_gate_verdict_test.rs::a_trailing_field_with_a_non_numeric_value_is_still_malformed`).
* **It does not re-record the committed #775 captures.** The eight strand
  fields did not move, so those captures replay unchanged; R182's re-record
  applies to a line shape a consumer can no longer parse, and this shape change
  is append-only.
* **The one-TID difference in §6.5 is not a result.** 1 production boot per
  side is not a distribution. What that pair establishes is 1 fact: the strand
  reading exists on `origin/main` too. It establishes 0 facts about the
  direction or size of any difference.
* **The parallel gate is 1 run of 5 boots, not a soak.** `Results: 5 passed, 0
  failed out of 5`, on the `testing,external_test_bins` profile, on a host that
  was also running another lane's gate. R157 bounds what a single PR has to
  demonstrate; a soak is not part of this slice.
  claim-lint:ok: the 5 of 5 verdicts are quoted in §6.1 from
  `/root/breenix-chk1-tmp/parallel5b.log`.
* **The oracle perturbs the census for the rest of the boot.** Each census line
  after it carries the oracle's own 1 in each of the ten fields. §6.3's reading
  of `sig_deliverable_user=4` as "3 natural" depends on that offset being
  exactly 1, which the `moved_by_one=10` verdict is what establishes.

---

## 8. Receipts

```
claim-lint: scripts/claim-lint.py                                  -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/pr1msg/c1.txt  -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/pr1msg/c2.txt  -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/pr1msg/c3.txt  -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/gates/CRITICAL-PATH-DEBT-PR1-2026-09-06.md -> exit 0
claim-lint: scripts/claim-lint.py --files /tmp/pr1msg/pr-body.md   -> exit 0
```

---

## 9. Landing re-smoke (merged head e99ab48ed293e9b58ab92b1366796f1eb4069a92)

`git merge --no-ff origin/main` from `e99ab48e` was a genuine no-op --
`origin/main` had not moved past `a0ec6cf8` (this branch's own base) since the
branch was cut, so `git merge-base --is-ancestor origin/main HEAD` was already
true and git reported `Already up to date.` with 0 commits created: 0 files in
conflict, 0 doc-union hunks, 0 gate-script hunks to reconcile, and R182's
fixture re-record does not apply because 0 of the 2 sides' scorer contracts
changed in a merge that added 0 commits. The pushed head is therefore still
`e99ab48e`.

### 9.1 Host-side suites, at the pushed head

| Suite | Result |
|---|---|
| `tests/*_structure.rs`, all 48 files via `scripts/run-structure-tests.sh` | **48/48 green, 734 total cases** |
| `python3 scripts/test_claim_lint.py` | exit 0 |
| `bash scripts/check-critical-path-violations.sh` | exit 1, **259** stdout lines, **9** `VIOLATION` headers -- identical to §5.6/§6.1's reading |
| `python3 scripts/claim-lint.py` | exit 0 -- `clean (11 file(s) checked, changed hunks vs a0ec6cf8473d)` |

### 9.2 x86, on beast (`breenix-x86`, clone `/root/breenix-chk1`, `BREENIX_GATE_TMP=/root/breenix-chk1-tmp`)

Confirmed at `e99ab48ed293e9b58ab92b1366796f1eb4069a92`, clean working tree.

| Gate | Result |
|---|---|
| build, `boot_tests,testing,external_test_bins` | exit 0, **0 lines matching `^(warning\|error)`** |
| `./scripts/check-x86-dispatch-no-alloc.sh` | PASS -- `0 allocating call targets in 3 in-scope symbol(s), 14 edge(s) checked` (identical to §6.1) |
| `./docker/qemu/run-x86-boot-tests.sh 1` | exit 0 -- `x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0`, `x86 frame-custody gate run 1: PASS` |
| oracle verdict (`serial_user.txt:53`) | `[DISPATCH_FACT_ORACLE:x86:facts=10:legs=10:moved_by_one=10:moved_wrong=0:irqs_enabled_before=1:PASS]` -- byte-identical to §6.2 |
| census line (last, `seq=363`) | `[DISPATCH_STRAND_CENSUS:seq=363:tick=51052:ms=444116:saved=11:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0:save_no_thread=1:save_no_proc=1:save_no_pm=1:sig_pending_blocked=1:sig_ctx_blocked=1:sig_delivered_blocked=1:idle_no_stack=1:kthread_no_info=1:user_no_kstack=1:sig_deliverable_user=4]` -- same shape as §6.3, 9 of the 9 defensive counters at the oracle's 1, `sig_deliverable_user` carries the oracle's 1 plus 3 natural site-16 firings |
| 16-deleted-string grep, both capture files | **0, 0** |
| `./docker/qemu/run-x86-prod-profile-boot-test.sh` | exit 0 -- `PASS: x86 production profile reached steady state with the teardown census at rest` |
| prod census line (last, `seq=53`) | `stranded=7:tids=5,15,17,19,20,22,23`, `sig_deliverable_user=3`, other 9 counters at 0 -- same shape as §6.3's production reading; the `stranded` count (7 here vs 6 in §6.1) is the same not-scored, 1-boot-per-side reading §6.5 already declines to treat as a result |

A first launch of the prod gate wrapped in `timeout 900` was killed by that
wrapper while still queued on the host lock (840s+ waiting behind two other
lanes' boots, 0s of it spent booting) -- a re-smoke sequencing artifact, not a
reading about the branch. Re-launched without the wrapper once the lock was
free; it ran to completion on the first attempt.

### 9.3 aarch64, on this Mac

Userspace ELFs and the aarch64 ext2 image did not exist in this fresh
worktree and were built first (`userspace/programs/build.sh --arch aarch64`,
`scripts/create_ext2_disk.sh --arch aarch64`) -- a worktree-freshness step,
not a branch reading; the kernel diff that matters is still the empty one
§6.6 already established.

| Gate | Result |
|---|---|
| `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | exit 0; 0 lines matching `unused\|error\[\|error:`. 1 unrelated nightly toolchain notice appears (`warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 ...`) -- a `-Z build-std` future-incompat notice about the `core` crate's own source, not kernel code; it is not gated on by the round doc's own check (§6.6) and is orthogonal to this PR's diff. |
| `./scripts/check-kernel-no-neon.sh target/aarch64-breenix-kernel/release/kernel-aarch64` | PASS -- `0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)` |
| `./docker/qemu/run-aarch64-boot-test-strict.sh` (script default) | **PASS: 20/20 boots succeeded** |
| `./docker/qemu/run-aarch64-prod-profile-boot-test.sh` | exit 0 -- `PASS: production profile reached bsshd with the futex oracle seam absent` |

### 9.4 What this re-smoke does NOT claim

* It does not claim a new fixture re-record. R182 applies when a scorer
  contract changed on either side of the merge; the merge added 0 commits, so
  0 of the 2 sides' scorer contracts changed.
* It does not claim the one-TID production-strand variance (§6.5) means
  anything different here. §9.2's prod-gate `stranded=7` sits inside the same
  1-boot-per-side, not-scored reading the original round already declined to
  interpret.
* It does not re-run the 5-boot parallel gate or the mutations of §5.3/§5.4;
  the land step's re-smoke scope is the suite list above, not a repeat of the
  fix round's full gate table.

### 9.5 Receipts

```
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/gates/CRITICAL-PATH-DEBT-PR1-2026-09-06.md -> exit 0
claim-lint: scripts/claim-lint.py                                                                                -> exit 0
```
