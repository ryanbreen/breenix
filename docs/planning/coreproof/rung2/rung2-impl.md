# Core-proof RUNG 2 — Component C implementation notes

Branch `feat/coreproof-rung2-c`, based on `origin/main` @ `579a10ccd73156d378a6f241d4b867222018749f`
(matches the spec's own anchor SHA exactly — no rebasing needed).

Committed here (per rung 1's own precedent of landing the spec alongside the
code, rung 2 review finding m7) at the FIX-ROUND head, i.e. after the
corrections in "Fix round (2026-08-30)" below were applied. Two sentences in
the original implement-slot narrative below have been corrected in place
rather than left to stand as written, because the review that follows this
slot (`rung2-review.md`) found them to be factually false, not merely
optimistic — see the inline `[CORRECTED …]` markers. Everything else in
sections 1-7 and "Files touched" describes what the *implement* slot actually
shipped, unedited.

## What shipped

1. **`kernel/src/proof/driver_c.rs` (new)** — Component C's driver, spawned
   from `proof::start()` when `coreproof_component_c` is the active build
   feature (mutually exclusive with `driver_a`, matching the existing
   compile-time `MODE`/`WINDOW`/`SEED` selection pattern). Shape: no
   synchronous probe (Component C has none); instead, every driver-loop
   iteration re-arms the `ScheduleEntry` seam on every online PEER cpu (via
   the new `mod.rs::arm_cpu`/`disarm_cpu` cross-CPU arming primitives — see
   below), so whichever peer next executes `scheduler::schedule()` via the
   *existing, unmodified* `KernelSchedule`/`Steal` adversarial ops finds a
   freshly drawn vector waiting for it. Scores three markers each cadence
   tick and once more at close: `CPU_IDENTITY_SPLIT_EVENTS` (M2's own
   predicate), `PERCPU_STACK_ALIEN_REFUSALS` (postcondition 2's own
   predicate, promoted from Component A's side-channel read to a first-class
   check), and `RET_STAGE_REFUSALS` (folded into the identity record's
   `detail` bit 63 when it moves alongside a split, per spec; reported as
   its own `RET_STAGE_REFUSED` finding on the — unanticipated by the spec —
   shape where it moves alone, rather than silently dropped).

2. **`kernel/src/proof/sites.rs`, `kernel/src/proof/mod.rs`, `kernel/src/proof/record.rs`
   generalized to be component-scoped**, per spec §4's explicit "one real,
   required framework change":
   - `sites.rs` now declares TWO mutually-exclusive `SiteId` definitions
     (`#[cfg(not(feature = "coreproof_component_c"))]` for Component A's
     unchanged 12-variant enum; `#[cfg(feature = "coreproof_component_c")]`
     for Component C's single-variant `{ ScheduleEntry }`), with the shared
     machinery (`SiteClass`, `mark_visited`, `visited_count`, `DECLARED`)
     left generic over whichever concrete type is compiled in.
   - `record.rs`'s `emit_seed_line`/`emit_run`/`violation` now take a
     `component: u8` parameter (printed via `component as char`) instead of
     hardcoding `comp=A`; all 13 call sites in `driver_a.rs` updated to pass
     `COMPONENT_A` explicitly.
   - `mod.rs` gained `arm_cpu(cpu, vector)`/`disarm_cpu(cpu)`, generalizing
     the existing self-arm `arm()`/`disarm()` (now `#[cfg(not(feature =
     "coreproof_component_c"))]`, since only Component A's synchronous
     self-probe shape needs them) to target an arbitrary online cpu's
     `ArmedSlot` — the mechanism Component C's driver needs to arm a *peer*
     ahead of that peer's own seam visit.

3. **`ScheduleEntry` seam** at the top of `scheduler::schedule()` (aarch64
   arm), before the call to `run_deferred_reclamation()`, gated
   `#[cfg(feature = "coreproof_component_c")]` (it cannot be unconditional —
   Component A's build doesn't declare that `SiteId` variant at all).
   `SiteClass::Open`, matching the spec's reasoning: every caller reaches
   `schedule()` with interrupts enabled, masking is `schedule_from_kernel`'s
   own job further down a call chain this harness may never seam. Component
   A's own nine `proof_point!` calls already living in `scheduler.rs`
   (`BlockEntry` etc.) needed the *opposite* gate
   (`#[cfg(not(feature = "coreproof_component_c"))]`) added at all nine call
   sites, since a Component C build's `SiteId` has no such variants — this
   was the first compile error the Component C build hit and is now fixed.

4. **M2 (`coreproof_mut_cpu_identity`)** — already fully present at
   `context_switch.rs:6500-6535` per the spec's own verification; untouched.
   No new predicate needed; rung 2's entire M2 contribution is the stimulus
   (`driver_c.rs` + the `ScheduleEntry` seam) described above.

5. **M7 (`coreproof_mut_masked_lock_bare`, new)** — the faithful #609
   completion the spec calls for: a new sibling register entry (not a
   rewrite of M6) plus the two real sites named in spec §3.2, moved
   together:
   - `kernel/src/memory/kernel_stack.rs`: `ARM64_STACK_BITMAP` becomes a
     bare `spin::Mutex` under the new feature (was `IrqSafeMutex`); all
     `.lock()` call sites unaffected (both types expose the same API).
   - `kernel/src/task/scheduler.rs::release_reclaimed_threads`: the
     existing M6 `coreproof_mut_masked_lock` cfg arm (unmasked drop) is
     *reused* under `any(...)` rather than duplicated, so both mutations
     share the one unmasked-drop code path.
   - `kernel/src/proof/coverage.rs` gained a 7th `MutSite::MaskedLockBare`
     variant with its own `proof_cover!` placement. **[CORRECTED —
     rung 2 review, B2: this was originally written up as "fired
     unconditionally alongside M6's existing one." It is not, and never
     was: the placement carries its own
     `#[cfg(feature = "coreproof_mut_masked_lock_bare")]` guard, same as
     every other mutation-specific cover site in this file. The cfg gate is
     the correct, intended design — a cover site for a feature that isn't
     built in would be meaningless — the original sentence was simply wrong
     about what the code does, not about what it should do.]**
   - Per spec, M7's expected outcome is explicitly **not** a
     `[COREPROOF:VIOLATION:...]` line — it is the gate's own
     missing/malformed-RUN-record condition (a wedged boot). The register
     entry's `predicate` field says so explicitly
     (`"NONE_EXPECTED:missing_run_record_is_the_catch"`) rather than being
     left to imply a false negative.

6. **Ratchets**: `scripts/check-coreproof-seams.sh` and
   `scripts/check-coreproof-production-clean.sh` needed zero changes (both
   are generic prefix/file-list scans, as the spec predicted) and both pass,
   including their own anti-vacuity (`--prove`) legs.
   `tests/coreproof_mutation_register_structure.rs` and
   `tests/coreproof_coverage_structure.rs` needed zero changes (pure
   census, additive-safe) and both pass. `tests/coreproof_sites_structure.rs`
   *did* need real changes (as spec §4 anticipated) — rewritten to discover
   every `pub enum SiteId { ... }` block in `sites.rs` (currently two) and
   run every check once per block, plus a genuinely corrected version of the
   scheduler-seam-classification check.

7. **`docker/qemu/run-coreproof-gate.sh`**: `--component C` now
   translates into the `coreproof_component_c` build feature automatically
   (previously the flag existed but did nothing beyond a display string),
   and the script's own default `MODE` is now per-component (`pen` for A
   unchanged, `adversarial` for C) rather than always `pen`, matching
   `quiesce.rs`'s own new build-time default. An explicit `--mode` still
   wins over either default.

## A bug fixed en route, not introduced

`tests/coreproof_sites_structure.rs`'s original
`every_scheduler_seam_is_classified_masked` test computed its "text of the
Open arm" window one arrow too late — `head.rfind("=>")` landed on the Open
arm's *own* arrow, so the captured slice ran from just after that arrow to
just before `"SiteClass::Open"`, i.e. pure whitespace. The
`!open_body.contains(...)` assertion was therefore checking a variant name
against an empty string and could never fail — the check was vacuous on
`main` before this rung touched it. Verified by hand-evaluating the original
extraction against `origin/main`'s `sites.rs`
(`repr(...) == ' {\n                '`). Fixed in the rewrite by walking back
a second arrow (or to the enclosing `match self {` for a first arm), which
correctly captures the arm's own pattern text. Re-verified against both the
original file (now correctly includes `Self::DriverPreCycle | ...`) and the
new Component C block (correctly includes `Self::ScheduleEntry`). This
directly affects the property this rung's own `ScheduleEntry` addition needs
enforced, so it was in scope to fix rather than carry forward broken.

**[Fix-round footnote — B3:** this same rewrite shipped a NEW vacuous branch
of its own: a single-site block with no `SiteClass::Open` arm at all was
`continue`-skipped instead of checked, so a single-seam component that
mis-classified its own seam would pass silently. See "Fix round
(2026-08-30)" below.]

## Verification performed

- `cargo check`/`cargo build --release` for the aarch64 kernel target,
  clean (zero warnings) across: production (no features), `boot_tests,coreproof`
  (Component A, unchanged), `boot_tests,coreproof,coreproof_component_c`
  (Component C), `+coreproof_mut_cpu_identity` (M2), and
  `+coreproof_mut_masked_lock_bare` (M7, both with and without
  `coreproof_component_c`), plus the pre-existing `coreproof_mut_masked_lock`
  (M6) combination.
- `scripts/check-coreproof-seams.sh` (plain and `--prove`): PASSED.
- `scripts/check-coreproof-production-clean.sh` (plain and `--prove`, legs 1+2):
  PASSED. (`--bytes` leg 3 not run — it is excluded from `cargo test` by the
  test file's own design and was not part of this task's build budget; legs
  1+2 already prove the production ELF carries zero harness symbols/markers.)
- All four `tests/coreproof_*.rs` host-side ratchet files (16 `#[test]`
  functions total): PASSED. Run via a standalone `rustc --edition 2021 --test`
  invocation per file rather than `cargo test`, because `cargo test` at the
  repo root requires building the top-level `breenix` package's x86 kernel
  artifact dependency, which is broken on this Mac independent of this
  change (confirmed by reproducing the identical failure — `uefi@0.38.0
  requires rustc 1.91`, current toolchain is `1.90.0-nightly` — against an
  untouched `origin/main` checkout via `git stash`). This matches the
  project's own "x86 = beast only" convention; the four test files
  themselves only use `std::fs`/`std::path` and have no dependency on the
  `kernel` crate, so the standalone `rustc --test` path exercises the exact
  same assertions `cargo test` would.
- **One smoke boot**, `docker/qemu/run-coreproof-gate.sh --component C --seeds 1 --profile max`
  (unmutated tree), after confirming no `qemu-system-aarch64` process was
  running first:
  ```
  === profile max — 1 boot(s), component C, mode adversarial, window post_cohort, disarmed 0 ===
  max#1: harness violations=0 iters=164071
  max#1: clean (seed=0x0000000009b89df0 iters=164071 sites=1/1)
  ARM64 CORE-PROOF GATE: PASSED
  ```
  **[CORRECTED — rung 2 review, B1 and m8:** the implement slot's own
  transcript above has no committed backing (`evidence/gate-smoke-max1.txt`
  records a *different* run, `iters=44605 seed=0x…939a9f0`, from a different
  worktree — m8) and, more importantly, the claim that followed it — "the
  non-vacuity gate is satisfiable, not merely declared" — was false at this
  SHA: `sites=1/1` was satisfied by ordinary boot traffic alone
  (`scheduler::schedule()` runs constantly regardless of which component is
  driving), so a boot that never dispatched the driver at all would have
  reported the identical `sites=1/1` (`evidence/mutation-m7-round2.txt`'s
  `max#3` boot did exactly that: `iters=0`, scored clean). The fix round
  closed this by giving Component C a THIRD, driver-only census site
  (`DriverPreCycle`, visited only from inside `driver_c.rs`'s own loop) and
  by making `iters=0` its own gate-failing condition in `adjudicate()`,
  independent of which sites any component declares. The re-quoted, honest,
  currently-committed smoke boot against the fixed tree is:
  ```
  === profile max — 1 boot(s), component C, mode adversarial, window post_cohort, disarmed 0 ===
  max#1: harness violations=0 iters=84393
  max#1: clean (seed=0x000000000987e548 iters=84393 sites=3/3)
  ARM64 CORE-PROOF GATE: PASSED
  ```
  see `evidence/gate-smoke-fix2-max1.txt`.]**

## What was explicitly NOT done in this pass, and why

Per the task's own scope ("run docker/qemu/run-coreproof-gate.sh for ONE
smoke boot only") and the spec's own framing (§3.1: "a miss is reported
honestly... never a dead end"), this pass built and smoke-tested the
mechanism but did **not** run the mutation-hunt validation campaign — i.e.
did not attempt to actually catch M2 or M7 within their ≤5-minute bars
across a seed set, measure replay hit rate, or run the full pass-bar matrix
from spec §6. That is empirical work distinct from "build the mechanism,"
consistent with rung 2 being an *implement* slot in a larger workflow. The
mechanism is wired exactly as designed and verified to run cleanly and
non-vacuously; whether it actually re-finds M2 within budget is an open,
honestly-unmeasured question for the validation phase — reporting otherwise
would be exactly the kind of unearned claim the project's intellectual
honesty policy forbids.

One disclosed simplification in `driver_c.rs`, stated in its own doc
comment: `victim_tid` is the driver's own current thread id (matching
Component A's convention) rather than a dedicated spawned victim kthread, so
`Unblock`/`Steal`'s unblock half is inert for this component (the driver
never blocks mid-loop). The useful half of `Steal` — the dispatch attempt it
always makes on the stealer's own cpu, reaching `ScheduleEntry` exactly like
`KernelSchedule` does — is unaffected. This keeps the implementation inside
the "reuse existing antagonist machinery verbatim" constraint; a dedicated
victim thread is a plausible future sharpening, not claimed as already done.

**[Fix-round footnote — M5:** the committed PROVE record went further than
this paragraph and credited a real "victim/stealer pairing" by name for the
M2 measurement in §3 of `PROVE-2026-08-30.md`. That pairing did not exist at
measurement time — this paragraph's own disclosure is what was actually
true. The fix round built the real pairing (see below); the PROVE record has
been corrected to say so.]**

## Files touched

- `kernel/Cargo.toml` — two new features:
  `coreproof_mut_masked_lock_bare`, `coreproof_component_c`.
- `kernel/src/proof/driver_c.rs` (new).
- `kernel/src/proof/{mod,sites,record,quiesce,coverage,mutations,driver_a}.rs`.
- `kernel/src/task/scheduler.rs` (ScheduleEntry seam + 9 existing-seam gates + M7's reused cfg arm).
- `kernel/src/memory/kernel_stack.rs` (M7's bitmap type swap).
- `tests/coreproof_sites_structure.rs` (component-scoped rewrite + the
  vacuity-bug fix above).
- `docker/qemu/run-coreproof-gate.sh` (`--component C` wiring).

No Tier-1 file touched. `context_switch.rs`/`context.rs` (both
architectures) remain untouched, matching the seam-placement non-goal.

## Fix round (2026-08-30): review findings closed

`rung2-review.md` (the review slot's record, workflow scratchpad) found
three blocking issues, five major issues and eight minor issues against the
state above. A dedicated fix round closed all of them; full detail is in
that round's own `fix2-notes.md` (workflow scratchpad) and in the commit
history on this branch from that point forward. Summary, for a reader
cross-referencing this document against the current code:

- **B1** (tautological non-vacuity gate): Component C's `SiteId` gained a
  third, driver-only census site (`DriverPreCycle`, matching Component A's
  own shape) plus a new `iters=0` gate-failing condition in
  `run-coreproof-gate.sh`'s `adjudicate()`.
- **B2** (M7's "non-vacuous miss" had no coverage measurement behind it):
  the enabling correction (this document, above) is done; the actual
  `--require-cov masked_lock_bare` re-run is the PROVE slot's own follow-up
  work, not this fix round's.
- **B3** (a new vacuous branch in the sites ratchet): fixed by hoisting the
  block-shape decision before the `SiteClass::Open` lookup.
- **M1** (the ratchet keyed a safety rule on block cardinality, not
  placement): `tests/coreproof_sites_structure.rs` now derives Masked/Open
  from where a seam actually sits in `scheduler.rs` (inside
  `impl Scheduler { .. }`, or after a lock-taking construct in its own free
  function, versus genuinely before any lock).
- **M2** (the cross-CPU arming protocol had no real acquire/release pairing
  and a fire/re-arm splice window): `mod.rs` gained a seqlock generation
  counter on `ArmedSlot`, `seam()`'s site load is now `Acquire`, and `fire()`
  validates the read as one coherent snapshot before applying it, dropping
  (never splicing) a torn read.
- **M3** (VIOLATION records named an unrelated fresh draw instead of the
  vector that actually fired): `mod.rs` now tracks the most recently fired
  vector per cpu (`LAST_FIRED`), and `driver_c.rs`'s violation reports carry
  that vector instead of a synthetic one.
- **M4** (cheaper aim levers were never tried before reaching for the icount
  escalation): a second `SiteId` (`PreDispatchMask`) now sits immediately
  before `schedule_from_kernel()`, past `run_deferred_reclamation()`'s five
  drains; `stimulus::materialize` now biases the `ticks` draw toward the low
  end of its range specifically at `Open` sites. The driver's own module
  header no longer claims "a handful of cycles."
- **M5** (the record credited a victim/stealer pairing the implementation
  did not build): built for real — `quiesce.rs` now designates the
  lowest-numbered online peer as victim (forced `KernelSchedule`) and the
  next as stealer (forced `Steal` against the victim's own live tid), for
  Component C only; Component A's path is unchanged (`PeerRole::Ordinary`
  unconditionally).
- **Minors m1-m8**: the kernel-side per-component mode default (dead under
  the gate) was removed so the gate script is the one owner; an
  effective-aimed-sample estimate now accompanies `iters_total` in the PROVE
  record; the parser-drift floor (`variants.len() >= 2`) is restored now
  that Component C has three real sites; a new structural test requires a
  multi-file mutation's register entry to gate code in as many files as it
  names; `adjudicate()` now checks `comp=` against `--component`; a
  structural note documents the `ForceResched` re-entry's bounded-by-disarm
  reasoning; this document and `rung2-spec.md` are committed here; the
  smoke boot above is re-quoted from committed evidence.

None of this touches production `.text` — every new site and seam is
`#[cfg]`-gated exactly like the ones it sits beside, and the production
build was re-verified clean (zero warnings, unchanged feature set) after
every change.
