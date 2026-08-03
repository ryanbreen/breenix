# Teardown Lifecycle — Parked Branch State

**Branch:** `fix/teardown-grave`
**Base:** `31126c2a` (main, PR #417 merge point)
**Status:** PARKED — checked out, clean working tree, 13 commits ahead of base
**Parked on:** 2026-08-02 (session end of the r12→r18 hardening sequence)

This document is the durable, self-contained record of where this branch
stands. A fresh session picking this up should need nothing beyond this file
plus the artifacts in this directory (`teardown-spec-final.md`,
`teardown-invariant-corpus.md`, `teardown-design-A.md`, `teardown-design-B.md`,
`r16-findings.md`, `r17-findings.md`).

## What this branch is

A redesign of process teardown / reclamation on aarch64 (and the shared
x86/aarch64 process-exit paths it touches): moving from ad hoc frame/resource
freeing at exit time to an intrusive "grave" allocated at process birth,
proof-gated retirement through `kreclaimd`, and centralized TTBR0 lease
transitions so that a retiring address space cannot be reused by another CPU
before teardown is provably complete. See `teardown-spec-final.md` for the
full design spec and `teardown-invariant-corpus.md` for the enumerated
invariants the implementation is checked against. `teardown-design-A.md` and
`teardown-design-B.md` are the two competing design writeups produced during
exploration (A ultimately informed the shipped design; both are kept for the
record).

## Branch state (verified at park time)

- 13 commits over `main@31126c2a` (`git rev-list --count 31126c2a..HEAD` = 13).
- Working tree clean, branch checked out at park time.
- All builds are zero-warning (x86_64 and aarch64), per CLAUDE.md's zero-tolerance
  policy — verified at every round through r18.
- `teardown_structure` invariant check: **9/9 passing** (structural invariants
  from the corpus, checked against source, not just runtime behavior).
- Frozen/gold-master regions (per top-level CLAUDE.md's GOLD-MASTER table):
  **byte-identical** to main — the branch did not touch
  `context_switch.rs` EL0 dispatch site, idle loop sleep gate, the narrow ISB
  placement, the GICv3 SGI enable block, or the timer handler arm-at-top /
  CPU0 regression alarm.
- Tier-1 diff (the two syscall_entry.S hunks) is the **only** Tier-1-listed
  file touched, and both hunks are **operator-approved**: see commit
  `be116df7` — "fix(aarch64): publish TTBR0 leases after the hardware handoff
  in syscall_entry.S (operator-approved Tier-1 edit, 2026-08-02)". No other
  Tier-1/Tier-2 file in CLAUDE.md's prohibited-sections table was modified
  outside this approved commit.

## Commit list (13, oldest first)

```
08414265 fix(aarch64): fence retirement grace and reject empty targets
68cb2438 fix arm64 last-dispatch owner decoding
be116df7 fix(aarch64): publish TTBR0 leases after the hardware handoff in syscall_entry.S (operator-approved Tier-1 edit, 2026-08-02)
ee8e5f17 fix(aarch64): centralize TTBR0 lease transitions
dadb987a refactor(process): make parent rows authoritative for child lookup
89cabcf2 feat(process): preallocate an intrusive grave at process birth
12c396b9 feat(aarch64): retire committed exits through proof-gated kreclaimd
a7ce750f test(aarch64): lock teardown invariants to source structure
8fd02d20 fix(aarch64): close TTBR0 lease and dispatch gaps
450b01c9 fix(process): harden deferred teardown and reclamation
12b1c001 test(aarch64): enforce teardown invariants after merge
76404d88 fix(aarch64): descope teardown hardening and close r17
be42fd54 restore x86 CoW fault and exec drain paths to main behavior
```

## Round history (finding counts)

The branch went through repeated review/verify rounds (Codex deep-review +
verifier passes) hunting correctness gaps between the new teardown design and
the invariant corpus. Finding counts per round:

| Round | Findings raised | Blocking | Notes |
|---|---|---|---|
| r12 | 14 | 5 | First full review pass post-initial-implementation |
| r13 | 14 | 4 | Re-review after r12 fixes |
| r15 | 20 | 9 | Broader sweep, more invariants added to corpus |
| r16 | 21 | 7 | See `r16-findings.md` for full detail |
| r17 | 21 closed + 2 x86 divergences | — | All 21 prior findings closed; verifier flagged 2 new x86-side divergences from main introduced incidentally by the aarch64 hardening work. See `r17-findings.md`. |
| r18 | those 2 restored, sweep found 2 NEW small x86 divergences | — | Commit `be42fd54` restored the 2 r17-flagged x86 divergences to main behavior. A follow-up sweep in the same round then found 2 *additional*, smaller x86 divergences (see "Open items" below) that were NOT yet resolved when the branch was parked. |

r14 is not a distinct review round in this sequence (numbering follows the
Codex review-round convention used in-session; r14 was folded into r15's
sweep).

## THE TWO OPEN ITEMS (verbatim from the r18 verifier)

These are the reason the branch is parked rather than merged. Both must be
resolved before the branch proceeds to final review.

**(a) `kernel/src/task/process_task.rs` — x86 `handle_thread_exit` reparent guard**

The x86 `handle_thread_exit` path gained a `pid != init_pid` reparent guard
that is **not present on main**. This guard was introduced (likely
incidentally, as a side effect of aarch64-focused hardening work sharing this
function) and needs a decision:

- **Keep and justify**: if the guard is actually correct/necessary behavior
  (e.g. prevents init from reparenting to itself, or fixes a latent bug also
  present on main), document why and keep it — but this needs explicit
  review sign-off since it changes shared (non-arch-gated) behavior.
- **Revert**: if it's not actually needed for the aarch64 teardown redesign,
  revert `handle_thread_exit` on the x86 path back to match main exactly, to
  keep the diff minimal and avoid unreviewed x86 behavior changes riding in
  on an aarch64 branch.

**(b) `kernel/src/memory/frame_metadata.rs` — core logic not arch-gated, diverges from main on x86-reachable functions**

The core logic in this file is **not arch-gated** (i.e. it runs on both x86
and aarch64), and it diverges from main's implementation in functions that
are reachable from the x86 path. This is a correctness/scope risk: changes
made for the aarch64 grave/reclamation redesign are affecting x86 frame
metadata behavior without an x86-specific review. Needs one of:

- **Arch-gate**: wrap the diverging logic in `#[cfg(target_arch = "aarch64")]`
  (or equivalent) so x86 keeps main's exact behavior and the new logic is
  aarch64-only, matching the stated scope of this branch.
- **Restore**: revert the x86-reachable functions in this file back to match
  main exactly, if the divergence isn't actually required by the aarch64
  redesign.
- **Reviewed justification**: if the shared-logic change is genuinely needed
  by both architectures (e.g. it's a real bug fix independent of the
  teardown redesign), document the justification explicitly and get it
  reviewed as a deliberate shared-code change — not something that rides in
  silently.

## Next steps (in order)

1. Resolve open item (a): decide keep-and-justify vs revert for the
   `process_task.rs` reparent guard.
2. Resolve open item (b): arch-gate, restore, or justify the
   `frame_metadata.rs` divergence.
3. Run the **final review**: report-everything pass (every finding, not just
   blocking ones) **plus a separate gate pass** (pass/fail on blocking
   criteria only) — these are two distinct review invocations, not one.
4. **12-boot QEMU + x86 gate**: 12 parallel/sequential QEMU boots on aarch64
   covering the teardown paths, plus the x86 zero-warning build + boot gate.
5. **Parallels validation**: 3-boot smoke, then a 6-attempt streak check,
   then a 30-minute soak run on real Parallels hardware (per the
   `parallels-launcher-test` skill / MANDATORY restart protocol in
   top-level CLAUDE.md — fresh epoch-named VM every time via
   `./run.sh --parallels`, never a static VM name).
6. **Lossless merge**: merge to main preserving full commit history (no
   squash) once all of the above is green. A PR body outline already exists
   in the r18 workflow script from this session — reuse it rather than
   drafting from scratch; it enumerates the round history and the frozen/
   Tier-1 verification the same way this document does.

## Artifacts in this directory

| File | Contents |
|---|---|
| `teardown-spec-final.md` | Full design spec for the grave/reclamation teardown redesign |
| `teardown-invariant-corpus.md` | Enumerated invariants (the 9/9 `teardown_structure` checks and others) the implementation must satisfy |
| `teardown-design-A.md` | Design exploration doc A (informed the shipped design) |
| `teardown-design-B.md` | Design exploration doc B (alternative considered, kept for record) |
| `r16-findings.md` | Full findings detail for review round r16 (21 findings, 7 blocking) |
| `r17-findings.md` | Full findings detail for review round r17 (21 closed, 2 new x86 divergences flagged) |

## Why parked instead of continued

The branch reached a clean, fully-green state (zero-warning builds, 9/9
structural invariants, frozen regions untouched, Tier-1 diff limited to two
operator-approved hunks) but the r18 sweep surfaced two small, unresolved
x86-side scope questions (items a and b above) that need a decision before
investing in the full validation matrix (12-boot + Parallels soak) and a
merge. Parking here avoids validating a state that will need to change again
once (a) and (b) are resolved, and keeps the branch + its record intact for
whoever picks it up next.
