# #745 — x86 `fork()` refused in the production profile

**Status: fixed; review round 2 applied.** `fix/745-x86-fork` (branch), PR
[#753](https://github.com/ryanbreen/breenix/pull/753) pending review/merge.
Gate state at `13a1cf27`, the last commit that changes anything the kernel or
the gate executes (the four commits on top of it -- `182937dc`, `84953270`,
`b7ac23bb`, `95c7807a` -- add serials, docs and this file only; `git diff
--stat 13a1cf27 HEAD -- kernel userspace docker tests scripts` is empty):
x86 production-profile gate PASS, and 24/24 structure-test files (496/496
tests) on the same bytes. The TTY-oracle 14/14-arms-over-25-boots soak was run
in round 1, at `3bf42613`, and was **not re-run in round 2** — round 2 changed
`kernel/src/syscall/handlers.rs` (`sys_fork_with_parent_context`) and
`kernel/src/process/manager.rs` (`complete_fork`), the TLS hoist, and TTY-oracle
arm 14 (`cloexec_exec`, `tty_oracle.rs:756`) calls `process::fork()` on x86 --
that exact function. What stands in place of a re-run: the oracle reported
`[TTY_ORACLE:COMPLETE:pass=14:fail=0]` plus `TTY_ORACLE:cloexec_exec:verdict=PASS`
at round-2 bytes, twice each, in two committed prod-gate serials
(`serials/review-round-2/final-gate-pass-serial_user-13a1cf27.txt` and
`serials/review-round-2-prove/prove-slot-gate-pass-serial_user-84953270.txt`).
The x86 production-profile gate that *was* re-run at those bytes deliberately
does not pin the 14-arm tally itself -- its own comment
(`run-x86-prod-profile-boot-test.sh:455`-`478`) explains why (the oracle
double-emits `TTY_ORACLE:COMPLETE:`, so an exact-count pin on it is flaky by
construction under this file's verdict-discipline rule) -- so it cannot by
itself distinguish 14 arms from 13.

**Update, atlas reproof pass, 2026-09-02: the 25-boot soak was re-run at
merged main (`0efa94a9`, this PR's merge commit).** 25 of 25 boots, 14/14
arms PASS every boot, 0 fail --
`docs/planning/green-program/tty/EVIDENCE-x86-14arm-reproof-2026-09-02.md`
and its two serials in that directory's `serials/`. The gap this section
described is closed at merged bytes.
`spec.md` and `precheck.md` in this directory are the investigation this
implementation round followed; precheck's sixteen binding conditions
override the spec wherever the two disagree, per the precheck's own
"corrections override the spec" framing.

<!-- claim-lint:ok: "all in {spec,precheck,README}.md" is a verbatim quote of
     commit 13a1cf27's own message (`git show 13a1cf27 --format=%B -s`), not
     a claim of this paragraph's own; the corrected count is this round's own
     `python3 scripts/claim-lint.py` run, recorded at fix-r3-notes.md. -->
**Correction to commit `13a1cf27`'s own body (review round 2, m9-r2).** That
commit's message records `scripts/claim-lint.py -> 93 findings, all in
{spec,precheck,README}.md`. That count was measured before the same commit's
own README rewrite and no longer reproduces. Re-running
`python3 scripts/claim-lint.py` at any later commit on this branch returns a
different total than 93 (`spec.md` 43 + `precheck.md` 35 + whatever this
directory's own prose then adds, since unlike `spec.md`/`precheck.md` this
README is NOT archived-verbatim and is linted live). Recorded here rather
than by amending the already-pushed commit message.

## What shipped

- `kernel/src/syscall/handlers.rs`: `sys_fork_with_parent_context`
  restructured into aarch64 fork's narrow-window shape — no
  `arch_without_interrupts` wrap (precheck C1), both deferred-reclaim
  passes run with no PM guard live (precheck C4, section 3.2's missing
  call), the PM lock is dropped before any logging or
  `scheduler::spawn_front(` (precheck C5), a defensive teardown arm for
  the (believed unreachable) "no main thread after a successful fork"
  case mirrors `sys_spawn`'s own #713 undo.
<!-- claim-lint:ok: the residue this bullet's "purged" claim does not cover is
     named two sections down and filed as #756. -->
- `kernel/src/process/manager.rs`: `fork_process_with_parent_context` and
  `complete_fork` de-gated from `feature = "testing"`; both purged of
  logging **in this function's own body** to match aarch64 fork's own "no
  logging under the PM lock" invariant (precheck C9; the residue this
  narrowing excludes -- `allocate_kernel_stack`'s own logging, still called
  under the PM lock from both bodies -- is named below and filed as #756); `trace_fork_entry`/`trace_stack_map`/
  `trace_fork_exit` wired to close the tracing-parity gap (precheck C11);
  the CoW setup block's parent-page-table restore fixed on both the x86
  production path (newly reachable) and the aarch64 twin
  `fork_process_aarch64` (already-live production code carrying the
  identical defect, precheck C2) — a CoW allocator failure no longer
  leaves the parent with `page_table == None`, which used to get the
  *parent* killed on its next dispatch.
- `kernel/src/process/fork.rs`: `setup_cow_pages_with_vmas`'s
  `#[allow(dead_code)]` removed — dishonest once it has an unconditional
  production caller (precheck C7).
- `tests/teardown_structure.rs`: the five pre-registered census arrays
  updated in the fix commit, plus two more the precheck's own C-conditions
  and a live `cargo test` run surfaced (`ROW_DESTRUCTOR_CALLS`,
  `REMOVE_FROM_READY_QUEUE_CALL_SITES` — the defensive teardown arm is a
  genuine new call site neither census had).
<!-- claim-lint:ok: 10 of 10 tests in that file pass and 8 of 8 assertions carry
     their own mutation test. -->
- `tests/fork_lock_order_structure.rs` (new): #745's version of
  `exec_lock_order_structure.rs`'s
  `validate_sys_exec_releases_process_manager` — proves by construction
  that no interrupt mask wraps the fork operation, both reclaim calls run
  with no PM guard live and precede `ProcessPageTable::new(`, and the
  guard is dropped before `spawn_front(`. Seven tests: one positive, six
  delete-mutation proofs, **all six independently confirmed to redden**
  (not merely asserted — see Evidence below), closing #721 review M1's
  "reported met, never reddened" gap for this arc.
<!-- claim-lint:ok: the receipt's own ability to fail was run, not asserted --
     serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt -->
- `userspace/programs/src/fork_smoke.rs` (new): arch-neutral acceptance
  program. Forces a real fork()+CoW+voluntary-yield+exit+reap round trip,
  and a **functional** CoW-isolation proof — parent and child each write a
  distinct sentinel to a shared page post-fork; the parent, only after
  reaping the child (race-free), reads the probe back and requires it
  still holds its own value. Precheck C3: the x86 CoW *fault* path had
  never executed in a zero-feature x86 build before this program existed;
  a broken refcount check would silently corrupt memory rather than
  crash, so proving isolation functionally (not just "some fault line
  appeared") is the load-bearing check.
<!-- claim-lint:ok: the pre-fix refusal is quoted in
     serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt -->
- `userspace/programs/src/init.rs`: `run_fork_smoke()`, positioned after
  `run_exec_smoke()` and before `start_bsshd()`; corrected the
  `run_tty_oracle()` x86 doc comment's false "fork are all
  production-safe on x86 already" claim (precheck C13, which corrects spec
  section 3.10; round 1 filed it under C10, a different item — review round
  2 M5).
<!-- claim-lint:ok: the two new pins were each reddened by a mutation --
     serials/review-round-2/b1-mutation-child-exit-38-gate-FAIL.txt and
     serials/review-round-2/m2-mutation-cow-isolation-broken-gate-FAIL.txt -->
- `docker/qemu/run-x86-prod-profile-boot-test.sh`: `FORK_SMOKE_*`
  markers following the `EXEC_SMOKE_*` template. The generic
  `[CREATION_LOCK_ORDER:VIOLATION` marker (already in `FAULT_MARKERS`,
  pinned at zero gate-wide) covers fork's own lock-order-at-publish-time
  receipt for free (precheck C5) — no separate pin needed. Two pins were
  added in review round 2:
  <!-- claim-lint:ok: the mutated run shows parent-reaped 1 and crash markers 0
       while this pin reddens --
       serials/review-round-2/b1-mutation-child-exit-38-gate-FAIL.txt -->
  - `' code=37]'`, `-eq 1` (B1). `[FORK_SMOKE:PARENT_REAPED child=` alone
    matches `code=-1` — a KILLED child — as happily as a clean exit, and the
    userspace-fault kill path emits nothing in `CRASH_MARKERS_PATTERN` or
    `FAULT_MARKERS`, so round 1 reaped the child without ever asserting how
    it exited.
  <!-- claim-lint:ok: 13 `[COW FAULT #` lines and exactly 1 `[COW FAULT #0] addr=`
       in serials/review-round-2/final-gate-pass-serial_user-13a1cf27.txt -->
  - `'[COW FAULT #0] addr='`, `-eq 1` (M2, restoring precheck C3(2)'s
    fault-OCCURRENCE requirement). A raw `[COW FAULT #` prefix was tried in
    round 1 and removed in `411975c9` because the harness's
    verdict-discipline rule requires an exact `-eq 0`/`-eq 1` and the total
    count varies with page-touch behaviour; nothing replaced it, and the
    round-1 comment wrongly described the isolation receipt as C3's own "or,
    better" alternative (that phrase names C11's counter). Pinning fault
    number ZERO makes occurrence exactly countable.
- `docs/planning/green-program/WORKLOAD-ENVELOPES.md`: corrected the
  stale "arm 14 excluded because exec() is ENOSYS" claim (precheck C14 —
  false since #721; fork was the real blocker, closed here) and the
  blended cell's arm-14 exclusion; disclosed the newly-reachable
  fork+CoW+`bsh` surface `fork_smoke` and this round add (precheck C13).
- `userspace/programs/src/tty_oracle.rs`,
  `docker/qemu/run-x86-tty-oracle-gate.sh`,
  `tests/tty_oracle_structure.rs`: arm 14 (`cloexec_exec`) re-admitted on
  x86, per the recipe on #721/#705 (commit `40d3ead8`, reverted by
  `7e2484ce` once fork — not exec — was identified as the real blocker).
  The original diff no longer applied cleanly (the revert's own commit
  had updated several comments from "#721" to "#745" without restoring
  the underlying cfg structure); reconstructed by hand against the
  current tree, same five `#[cfg(target_arch = "aarch64")]` sites the
  issue's own precheck pre-registered.

## Not touched, disclosed

- `ProcessManager::fork_process_with_page_table` and
  `fork_process_with_context` (the two other x86 fork variants the issue
  itself named) — neither is on the live production syscall path
  (`testing`/`boot_tests`-gated only), per this project's minimal-wiring
  precedent (#721). Both carry the identical C2 page-table-restore defect
  `fork_process_with_parent_context`/`fork_process_aarch64` had; filed as
  **#752** rather than widening this PR's scope.
- Precheck C10's TLS-registration cost that remains after the hoist: there
  is still no unregister counterpart, so `TLS_MANAGER.tls_blocks` grows
  monotonically per fork. That half is unchanged and still x86-only
  (`complete_fork_aarch64` registers no TLS at all); what round 2 removed is
  the part that was a hazard — the masked, logging, second-global-lock
  sub-window running inside the PM-held region.
- Precheck C12's `fork()`/`clone(SIGCHLD)` syscall-routing divergence
  between arches — informational in the precheck (no binding "Condition:"),
  left as-is.
<!-- claim-lint:ok: the six PM-held call sites and the no-live-deadlock analysis
     are enumerated in #756. -->
- **#756, filed this round.** `memory::kernel_stack::allocate_kernel_stack`
  emits two live `log::debug!` lines and is called under the PM lock by
  `complete_fork`, by `complete_fork_aarch64`, and by every x86
  `create_user_process`/`create_process_with_argv`. So the "no logging under
  the PM lock" invariant is true of the fork function BODIES and not of the
  region they run in, on both arches. No live deadlock is constructible from
  it (`_log_print` masks interrupts before taking `SERIAL2`, so the serial
  holder cannot be preempted while holding it), it is not something #745
  introduced, and fixing it is a shared-allocator change this PR's x86-only
  battery cannot prove on aarch64. Filed, and each of the four #745 claim
  sites narrowed to say so rather than asserting the region-wide property.
<!-- claim-lint:ok: 0 of 2 counters have call sites tree-wide, which is the
     symmetry this bullet reports; see #745 precheck C11. -->
- `count_fork()`/`count_cow_fault()` (precheck C11's counter suggestion) —
  left unwired. Verified **N of N observed**: `count_exec()` is *also*
  dead tree-wide (zero call sites), so wiring only `count_fork()` would
  create asymmetry rather than close a fork-specific gap; this is a
  pre-existing, cross-cutting `/proc` counters gap on both arches, not
  something #745 introduced or is scoped to fix.
<!-- claim-lint:ok: this bullet's whole point is that these are NOT proven --
     0 of 3 exercised by any leg in this round; see #745 precheck C13. -->
- `bsshd`/`bcheck`/`bterm`'s own fork call sites (precheck C13) — now
  newly reachable in principle (fork works) but not directly exercised by
  any gate leg in this round; disclosed, not proven.

## What review round 2 changed

The round-1 review returned 12 findings, 2 blocking. Dispositions:

| # | Finding | Disposition |
|---|---|---|
| B1 | Gate reaped the child without asserting its exit code, and its own comment said it did | Pin added (`' code=37]'`, `-eq 1`), comment corrected, and the pin reddened by a `CHILD_EXIT_CODE` 37→38 mutation run |
<!-- claim-lint:ok: the row's own disposition column names #756 and the hoist;
     4 of 4 claim sites narrowed. -->
| B2 | Precheck C9 not closed: `register_thread_tls` masked interrupts, took a second global lock and logged, under the PM lock; four claim sites said otherwise | Closed by construction — registration hoisted out of the PM window to `sys_fork_with_parent_context`. All four claim sites narrowed; the residual callee that still logs in that region (`allocate_kernel_stack`, shared with aarch64 fork and every spawn) filed as #756 and named at each site |
| M1 | Safety rationale asserted a false universal about interrupt-context PM access | Census re-derived at these bytes (9 sites: 7 non-blocking, 2 blocking-but-userspace-fault-only) and the sentence rewritten to match, with the grep in the comment |
| M2 | CoW receipt one-sided and interleaving-dependent; C3's fault-occurrence assertion had been replaced, not strengthened | Occurrence pin restored (`[COW FAULT #0] addr=`); receipt made order-independent with a child-only probe plus a child-side mirror check; both reddened by a kernel mutation that genuinely breaks isolation |
| M3 | `LIVENESS_WINDOW_SECONDS` rationale stale | Re-measured post-fix (span 27.3s in a 60s window); number kept, derivation replaced, raw timing artifact committed |
| M4 | README quoted lines no committed artifact contains | Real serial committed; round-1 files relabelled as gate stdout; the mis-citation stated rather than quietly re-quoted |
| M5 | README swapped precheck C9 and C10, and mis-attributed C13 | Both fixed, here and at the in-source copy in `init.rs` |
<!-- claim-lint:ok: 2 of 2 citations re-derived against the current tree. -->
| m6 | Stale line pins in `WORKLOAD-ENVELOPES.md` | Re-derived (`init.rs:123`-`130`, `:131`-`132`), "all three" → four |
| m7 | `print_observed_values` label contradicted its literal | Fixed, plus labels for the two new markers |
| m8 | Stale census sentence in `teardown_structure.rs` | Rewritten to describe the cfg-path change it actually documents |
| m9 | C2's fix had no structural pin | `fork_lock_order_structure.rs` now pins restore-before-`cow_result?` over BOTH production fork bodies, with swap and delete mutations |
| m10 | In-repo evidence predated the shipping bytes | The committed passing run is now at `13a1cf27`, the bytes this PR ships |


## Evidence (claim discipline: N of M observed, no "proven" without a named mutation)

<!-- claim-lint:ok: 0 warnings, 0 errors, from an explicit grep on the build
     output; the gate's own zero-warning assertion is in
     serials/review-round-2/final-gate-pass-stdout-13a1cf27.txt -->
- **Build**: x86 zero-feature production profile and `testing,external_test_bins`
  profile both `cargo build` clean on beast — 0 warnings, 0 errors,
  confirmed by explicit `grep -E "^(warning|error)"` on the build output
  (empty both times). aarch64 release build (Mac-native,
  `aarch64-breenix-kernel.json`) also clean, 0 warnings, 0 errors — this
  is the only arch/profile combination this round could build locally;
  every x86 build and boot ran on beast per project policy.
<!-- claim-lint:ok: 24 of 24 files, 496 of 496 tests, 0 failures. -->
- **Structural ratchets**: the full enumerated `tests/*_structure.rs`
  family (24 files, discovered via `ls tests/*structure*.rs` on beast at
  implementation time, not from memory) — **all 24 pass, 0 failures**,
  after three real regressions found by running the suite (not assumed
  clean) and fixed: two missing census entries
  (`ROW_DESTRUCTOR_CALLS`, `REMOVE_FROM_READY_QUEUE_CALL_SITES`) and one
  gate-script marker whose comparison the harness's own verdict-discipline
  rule rejected (`COW_FAULT_PREFIX`, replaced with the isolation receipt).
- **`fork_lock_order_structure`, round 1**: 7/7 tests pass, including all 6
  delete-mutation proofs (reintroduced interrupt mask, dropped either
  reclaim call, reordered `ProcessPageTable::new(` ahead of reclaim, an
  extra `manager()` acquisition simulating a live guard across reclaim,
  and a missing `drop(manager_guard)` before `spawn_front(`) — each
  independently confirmed to redden the validator, run on beast. (Round 2
  added two more tests/mutations; see "Round-2 structural ratchets" below
  for the file's current 10/10 total.)
<!-- claim-lint:ok: every line quoted in the next two bullets is grep-able in
     the file named beside it; that is the point of the round-2 correction. -->
- **Production gate at the shipping bytes, 1 boot** (`13a1cf27`, review round
  2): `run-x86-prod-profile-boot-test.sh` — PASS. The lines below are in
  `serials/review-round-2/final-gate-pass-serial_user-13a1cf27.txt`:

  ```
  [FORK_SMOKE:LAUNCH]
  [FORK_SMOKE:CHILD pid=7]
  [FORK_SMOKE:COW_ISOLATION_OK probe=0xfeedfeed child_only=0x0]
  [FORK_SMOKE:PARENT_REAPED child=7 code=37]
  [FORK_SMOKE:LAUNCHER_EXIT code=0]
  ```

  That serial carries 13 `[COW FAULT #` lines and exactly 1
  `[COW FAULT #0] addr=`, which is what the new occurrence pin counts. The
  gate's own marker tally is in
  `serials/review-round-2/final-gate-pass-stdout-13a1cf27.txt`, and the kernel
  half of the serial in
  `serials/review-round-2/final-gate-pass-serial_kernel-13a1cf27.txt`.
- **Round 1's own gate runs**: `serials/prod-profile-gate-pass-2026-09-02.txt`
  and `serials/prove-leg1-boot1-gate-stdout-2026-09-02.txt` are gate STDOUT —
  per-marker counts and the verdict line, not serial. Round 1's README quoted
  `[FORK_SMOKE:CHILD pid=6]`, "8 `[COW FAULT #N]` lines" and
  `[FORK_SMOKE:PARENT_REAPED child=6 code=37]` as if from them; those exact
  strings are in neither file (`grep -c '\[FORK_SMOKE:'` → 0). The
  observations were real gate output, the citation was wrong, and the fix is
  the committed serial above rather than a re-quote (review round 2, M4/m10).
  The prove pass's own 10/10 extended run is described in its report; one of
  its ten stdout captures is committed as the `prove-leg1-boot1` file above.
- **Anti-vacuity negative control, 1 boot**: the identical extended gate
  run against the pre-fix kernel (`manager.rs`/`handlers.rs` reverted to
  `origin/main`, everything else — gate script, `fork_smoke`, `init.rs`
  — kept at the fixed shape) — FAIL, as required: `[FORK_SMOKE:
  FORK_FAILED ENOMEM]`, zero child/isolation/reap markers. Confirms the
  gate is not vacuously green on old code. Full serial in
  `serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt`.
- **TTY oracle gate, arm 14 re-admission, 1 boot**: `run-x86-tty-oracle-gate.sh
  --rebuild-userspace` — PASS, **14/14 arms**, including `cloexec_exec`.
  (First attempt without `--rebuild-userspace` failed on a stale ext2
  image carrying the pre-readmission `tty_oracle.elf` — a real instance
  of the #721 K9 landmine class the gate's own header warns about, caught
  by reading the actual failure rather than assuming the mechanism was
  broken.) Full serial in `serials/tty-oracle-14of14-pass-2026-09-02.txt`.
- **Round-2 mutation, B1 (the new exit-code pin can fail)**: `CHILD_EXIT_CODE`
  mutated 37 → 38, userspace rebuilt, gate re-run — FAIL at exactly
  `test "$(marker_count "$FORK_SMOKE_PARENT_REAPED_CODE_LITERAL")" -eq 1`,
  with `fork smoke parent reaped: 1` and `crash markers: 0` in the same run.
  That is the finding in one output: the round-1 pin set stays green while the
  child exits with the wrong code. Mutation reverted.
  `serials/review-round-2/b1-mutation-child-exit-38-gate-FAIL.txt`.
- **Round-2 mutation, M2 (the isolation receipt can fail)**: a kernel mutation
  in `setup_cow_pages_with_vmas` mapping the child with the parent's ORIGINAL
  writable flags instead of `cow_flags` — i.e. genuinely broken CoW isolation.
  Observed:
  `[FORK_SMOKE:COW_ISOLATION_CORRUPTED probe=0xfeedfeed child_only=0xc0ffeeee]`,
  and `fork smoke CoW isolation OK: 0` / `corrupted: 1` in the gate's own
  tally. `probe=0xfeedfeed` is the parent reading its OWN sentinel back —
  round 1's single-probe receipt would have reported OK on that kernel, which
  is exactly M2's argument. The gate reddened, though at the earlier
  `BSSHD_STARTED` pin (line 996) rather than at the isolation pin (line ~1022):
  sharing every writable page also breaks the rest of the boot, so the run
  aborts before reaching the fork block. Mutation reverted.
  `serials/review-round-2/m2-mutation-cow-isolation-broken-gate-FAIL.txt` and
  `...-serial_user.txt`.
- **Round-2 liveness re-measurement** (precheck C13(b)): first-appearance
  timestamps for 13 markers relative to QEMU launch, sampled every 0.25s beside
  a passing gate. Steady state 11.18s, last pinned marker (`bsshd: listening`)
  38.49s, span 27.3s inside a 60s window.
  `serials/review-round-2/liveness-window-remeasure-2026-09-02.txt`.
- **Round-2 structural ratchets**: the same enumerated 24-file family,
  **24/24 files, 496/496 tests, 0 failures** (493 + the three new C2-ordering
  tests). `fork_lock_order_structure` is 10/10, including the two new
  mutations (swap the restore past `cow_result?`; delete the restore) over
  BOTH production fork bodies.
- **TTY oracle gate, 25-boot soak**: launched per spec §5's own
  recommendation (arm 14 is the first test anywhere in the tree of
  "child of a fork immediately calls exec()" on x86 — the interaction
  between fork's plain `publish_to_scheduler()` and exec's
  `ExecSchedCommit` re-publish of that same, still-very-fresh thread had
  no prior coverage). **25/25 boots, 14/14 arms PASS every boot** —
  `run-x86-tty-oracle-gate.sh --boots 25 --rebuild-userspace` on beast,
  final line `PASS: x86 TTY oracle gate - 25/25 boots, 14 arms green on
  the shipped production profile`. Full log:
  `serials/tty-oracle-25boot-soak-2026-09-02.txt`.
