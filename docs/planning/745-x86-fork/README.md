# #745 — x86 `fork()` refused in the production profile

**Status: fixed, all gates green including the 25-boot arm-14 soak.**
`fix/745-x86-fork` (branch), PR [#753](https://github.com/ryanbreen/breenix/pull/753)
pending review/merge.
`spec.md` and `precheck.md` in this directory are the investigation this
implementation round followed; precheck's sixteen binding conditions
override the spec wherever the two disagree, per the precheck's own
"corrections override the spec" framing.

## What shipped

- `kernel/src/syscall/handlers.rs`: `sys_fork_with_parent_context`
  restructured into aarch64 fork's narrow-window shape — no
  `arch_without_interrupts` wrap (precheck C1), both deferred-reclaim
  passes run with no PM guard live (precheck C4, section 3.2's missing
  call), the PM lock is dropped before any logging or
  `scheduler::spawn_front(` (precheck C5), a defensive teardown arm for
  the (believed unreachable) "no main thread after a successful fork"
  case mirrors `sys_spawn`'s own #713 undo.
- `kernel/src/process/manager.rs`: `fork_process_with_parent_context` and
  `complete_fork` de-gated from `feature = "testing"`; both purged of
  logging to match aarch64 fork's own "no logging under the PM lock"
  invariant (precheck C9); `trace_fork_entry`/`trace_stack_map`/
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
- `tests/fork_lock_order_structure.rs` (new): #745's version of
  `exec_lock_order_structure.rs`'s
  `validate_sys_exec_releases_process_manager` — proves by construction
  that no interrupt mask wraps the fork operation, both reclaim calls run
  with no PM guard live and precede `ProcessPageTable::new(`, and the
  guard is dropped before `spawn_front(`. Seven tests: one positive, six
  delete-mutation proofs, **all six independently confirmed to redden**
  (not merely asserted — see Evidence below), closing #721 review M1's
  "reported met, never reddened" gap for this arc.
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
- `userspace/programs/src/init.rs`: `run_fork_smoke()`, positioned after
  `run_exec_smoke()` and before `start_bsshd()`; corrected the
  `run_tty_oracle()` x86 doc comment's false "fork are all
  production-safe on x86 already" claim (precheck C10).
- `docker/qemu/run-x86-prod-profile-boot-test.sh`: `FORK_SMOKE_*`
  markers following the `EXEC_SMOKE_*` template. The generic
  `[CREATION_LOCK_ORDER:VIOLATION` marker (already in `FAULT_MARKERS`,
  pinned at zero gate-wide) covers fork's own lock-order-at-publish-time
  receipt for free (precheck C5) — no separate pin needed. A raw
  `[COW FAULT #N]` count was tried and removed: the harness's own
  verdict-discipline rule requires every declared marker to be spent with
  an exact `-eq 0`/`-eq 1` assertion, and the real count (8, observed) is
  neither — the CoW-isolation receipt is the intended "or, better"
  alternative precheck C3 itself names, and is what actually ships.
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
- Precheck C9's TLS-registration-under-PM-lock cost (x86-only, no
  unregister counterpart, monotonic growth per fork) and C12's
  `fork()`/`clone(SIGCHLD)` syscall-routing divergence between arches —
  both purely informational in the precheck (no binding "Condition:"),
  left as-is.
- `count_fork()`/`count_cow_fault()` (precheck C11's counter suggestion) —
  left unwired. Verified **N of N observed**: `count_exec()` is *also*
  dead tree-wide (zero call sites), so wiring only `count_fork()` would
  create asymmetry rather than close a fork-specific gap; this is a
  pre-existing, cross-cutting `/proc` counters gap on both arches, not
  something #745 introduced or is scoped to fix.
- `bsshd`/`bcheck`/`bterm`'s own fork call sites (precheck C13) — now
  newly reachable in principle (fork works) but not directly exercised by
  any gate leg in this round; disclosed, not proven.

## Evidence (claim discipline: N of M observed, no "proven" without a named mutation)

- **Build**: x86 zero-feature production profile and `testing,external_test_bins`
  profile both `cargo build` clean on beast — 0 warnings, 0 errors,
  confirmed by explicit `grep -E "^(warning|error)"` on the build output
  (empty both times). aarch64 release build (Mac-native,
  `aarch64-breenix-kernel.json`) also clean, 0 warnings, 0 errors — this
  is the only arch/profile combination this round could build locally;
  every x86 build and boot ran on beast per project policy.
- **Structural ratchets**: the full enumerated `tests/*_structure.rs`
  family (24 files, discovered via `ls tests/*structure*.rs` on beast at
  implementation time, not from memory) — **all 24 pass, 0 failures**,
  after three real regressions found by running the suite (not assumed
  clean) and fixed: two missing census entries
  (`ROW_DESTRUCTOR_CALLS`, `REMOVE_FROM_READY_QUEUE_CALL_SITES`) and one
  gate-script marker whose comparison the harness's own verdict-discipline
  rule rejected (`COW_FAULT_PREFIX`, replaced with the isolation receipt).
- **`fork_lock_order_structure`**: 7/7 tests pass, including all 6
  delete-mutation proofs (reintroduced interrupt mask, dropped either
  reclaim call, reordered `ProcessPageTable::new(` ahead of reclaim, an
  extra `manager()` acquisition simulating a live guard across reclaim,
  and a missing `drop(manager_guard)` before `spawn_front(`) — each
  independently confirmed to redden the validator, run on beast.
- **Production gate, 1 boot**: `run-x86-prod-profile-boot-test.sh` —
  PASS. Real fork observed: `[FORK_SMOKE:CHILD pid=6]`, 8
  `[COW FAULT #N]` lines (both parent and child sides), `[FORK_SMOKE:
  COW_ISOLATION_OK probe=0xfeedfeed]`, `[FORK_SMOKE:PARENT_REAPED
  child=6 code=37]`. Full serial in `serials/prod-profile-gate-pass-2026-09-02.txt`.
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
