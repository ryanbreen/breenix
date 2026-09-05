# Gate verdict discipline, widened — 2026-09-05

Branch `gates/verdict-discipline-widened`, off `origin/main` at `9b3dd4af`.
Repair commit `5a5c1ce4`. This doc's own commit follows it.

## Background

`docker/qemu/run-x86-prod-profile-boot-test.sh` carries a specific
architecture (this arc touches no `kernel/src/...` file): `set -euo
pipefail` + `set -E` (errtrace), a `report_gate_failure()` function armed on
the `ERR` trap, and a rule — `tests/teardown_structure.rs`'s
`x86_production_profile_gate_verdict_discipline_holds`, backed by
`validate_x86_prod_profile_harness` — that no `exit` statement may appear
anywhere in the script other than the trap's own re-raise
(`exit "$exit_code"`). #801 added two `BREENIX_GATE_TMP` preflight checks to
that script with a bare `exit 1`, which reached before the trap existed and
could end the gate with no verdict line at all — #802. #805 fixed it: the
two checks moved into a `BASE-DIR PREFLIGHT` block immediately after the ERR
trap installs, rejecting with `echo` + bare `false` instead of `exit 1`, so a
rejection is spent through `report_gate_failure`.

#805's PR body disclosed its own scope: #797 put the same `BREENIX_GATE_TMP`
absolute-path guard into eight gate scripts and the AF_UNIX `sun_path` guard
into two, and #805 converted only the two in
`run-x86-prod-profile-boot-test.sh`. "Whether each sibling's preflight should
print its own gate's FAIL line is a judgement about that gate's verdict
model, not a red this branch is carrying." This doc is that judgement, made
uniformly, and the repair that follows from it.

## Census

Grepped `docker/qemu/` and `scripts/` for the verdict-line shapes named in
the task: `"PASS:`, `"FAIL:`, `PASS (`, `FAIL (`, `gate: (PASS|FAIL)`,
`gate run`, `verdict`, and `BREENIX_GATE_TMP`. 37 `.sh` files matched (this
doc originally said 36; re-run directly:
`grep -lE '"PASS:|"FAIL:|PASS \(|FAIL \(|gate: (PASS|FAIL)|gate run|verdict|BREENIX_GATE_TMP'
$(find docker/qemu scripts -name '*.sh')` over the current 91-script tree
→ 37).

**Correction (review round 1, F6, MAJOR).** That grep pattern is itself an
undercount of scripts that print a PASS/FAIL-shaped verdict: it misses any
script phrased as `: PASS"`, `: FAILED (`, or `RATCHET: FAILED` instead of
the exact shapes above. ~~18 such scripts exist, four of them gate scripts
this campaign's own census gates invoke directly as sub-checks
(`scripts/check-kernel-no-neon.sh`, called from
`run-aarch64-tty-oracle-gate.sh`; `scripts/check-coreproof-production-clean.sh`,
`scripts/check-fs-fault-production-clean.sh`,
`scripts/check-x86-dispatch-no-alloc.sh`) plus fourteen more
(`run-blocking-recv-test.sh`, `run-dns-test.sh`, `run-keyboard-test.sh`,
`run-kthread-test.sh`, `scripts/check-coreproof-seams.sh`,
`scripts/ci/ring3_check.sh`, `scripts/parallels/build-efi.sh`,
`scripts/parallels/collect-breenix-cpu0-traces.sh`,
`scripts/parallels/collect-hwdump.sh`,
`scripts/parallels/collect-linux-cpu0-traces.sh`,
`scripts/parallels/inject.sh`, `scripts/parallels/screenshot-vm.sh`,
`scripts/run-arm64-keyboard-test.sh`, `scripts/test_fork_mcp.sh`).~~

**Correction (landing re-derivation, F6 continued).** That 18 count is
itself wrong — it is an overcount. Re-deriving it directly (grepping the 91
`.sh` files under `docker/qemu/` and `scripts/` for the three phrasings
named above, `: PASS"` / `: FAILED (` / `RATCHET: FAILED`, plus the eight
original patterns from the Census section, then reading each of the 18
named files in full) finds only **13 true positives**; the other **5 are
false positives** that do not print any PASS/FAIL/verdict-shaped text at
all, in any form, checked with a broad case-insensitive
`pass|fail|verdict|success` sweep over each file's full body: all five are
`scripts/parallels/` VM-manipulation utilities whose only `exit N`
statements are argument/prerequisite validation (a missing arg, a missing
tool, an unreachable VM), never a completion verdict —
`scripts/parallels/build-efi.sh` (builds an EFI image; the sweep's one hit
is the comment "newfs_msdos requires a block device and fails on plain
files", describing `newfs_msdos`'s own limitation, not this script's
verdict), `scripts/parallels/collect-breenix-cpu0-traces.sh` and
`scripts/parallels/collect-hwdump.sh` (diagnostic collectors, zero hits),
`scripts/parallels/collect-linux-cpu0-traces.sh` (zero verdict hits; the
sweep's only hit is the `VM_PASSWORD`/`VM_SUDO_PASSWORD` variable names, a
substring match on "pass" inside "password", not a verdict), and
`scripts/parallels/screenshot-vm.sh` (zero hits). The other 13 are
genuine: each prints a PASS/FAIL/SUCCESS/FAILURE-shaped line and has at
least one bare `exit N` statement in the file. Two shapes account for all
of the 13. Colored `PASS:`/`FAIL:` (`"${GREEN}PASS:${NC}"` etc.) — invisible to
the original census's literal `"PASS:` pattern because a color-code
variable sits between the opening quote and the word: `scripts/check-kernel-no-neon.sh`,
`scripts/check-x86-dispatch-no-alloc.sh`,
`scripts/check-coreproof-production-clean.sh`,
`scripts/check-fs-fault-production-clean.sh`, `scripts/check-coreproof-seams.sh`
(5 scripts — the four the doc already called out as census sub-checks,
plus `check-coreproof-seams.sh`). A same-family but differently-worded
banner: `docker/qemu/run-dns-test.sh` ("DNS TEST: ALL N STAGES PASSED" /
"DNS TEST: TIMEOUT"), `docker/qemu/run-keyboard-test.sh` ("SUCCESS: Found
N keyboard interrupt markers" / "FAILURE: No KEY:XX patterns found"),
`docker/qemu/run-kthread-test.sh` ("=== KTHREAD JOIN TEST: PASS ==="),
`scripts/ci/ring3_check.sh` ("=== RING3 CHECK: PASS ==="),
`scripts/parallels/inject.sh` ("[inject.sh] prlctl send-key-event FAILED"
on a dispatcher failure), `scripts/test_fork_mcp.sh` ("FAILED" / "❌
FAILED" per sub-test), and `docker/qemu/run-blocking-recv-test.sh` /
`scripts/run-arm64-keyboard-test.sh` (both match the task's own named
phrasings directly) (8 scripts). 5 + 8 = 13. Checked
each of these 13 (not 18, not 5) directly for the shape that actually
decides scope (`report_gate_failure() {` + an `ERR` trap that arms it, per
the paragraph below) — 0 of 13 carry it, the same null result the doc's
own (wrong) 18-count already reported for its (wrong) superset, so the
final answer (7 scripts carrying that architecture, 6 of them repaired
below) is unaffected either way this count lands. This corrects the doc's
own miscounted candidate pool a second time, not the repair's scope. For
each,
the question that matters is not "does it print PASS/FAIL text" —
most of them do, in ad hoc ways — but "does it carry the specific
`report_gate_failure`-via-`ERR`-trap backstop architecture that makes the
#802 shape possible in the first place": `set -e`/`set -E`, a function named
`report_gate_failure`, and a `trap ... ERR` line that arms it. That
architecture is what makes a bare `exit` dangerous (it silently escapes the
trap — verified below) and what makes a uniform "no exit but the trap's own
re-raise" rule meaningful. A script without that architecture has a
different verdict model (an ad hoc echo-then-exit sequence, or an
exit-code-only contract) and forcing the #805 idiom onto it would not be a
parallel fix -- it would be inventing an architecture that script did not have.

Grepping the 37 candidates for `report_gate_failure` + an `ERR` trap that
arms it found exactly **7**, 7 of 7 under `docker/qemu/`, 0 of 7 under
`scripts/`:

```
docker/qemu/run-aarch64-tty-oracle-gate.sh
docker/qemu/run-coreproof-gate.sh
docker/qemu/run-ext2-lock-race-gate.sh
docker/qemu/run-fs-fault-gate.sh
docker/qemu/run-x86-boot-tests.sh
docker/qemu/run-x86-prod-profile-boot-test.sh
docker/qemu/run-x86-tty-oracle-gate.sh
```

The remaining 30 of 37 each use `trap cleanup EXIT` or
`trap "rm -rf $DIR" EXIT` (resource cleanup on any exit, not a verdict
backstop) or carry no trap at all.

### Verified: a bare `exit` does not reach an `ERR` trap

The whole premise of #802/#805 rests on one bash fact, checked directly
rather than assumed:

```bash
set -eE
trap 'echo TRAP_FIRED status=$?' ERR
exit 1
```

produces no output and exits 1 — the trap does not fire. A `return N`
inside a function called as a plain statement, by contrast, **does** fire
the trap with `$?` equal to `N`, checked the same direct way:

```bash
set -Eeuo pipefail
trap 'echo TRAP_FIRED status=$?' ERR
inconclusive() { return 2; }
echo before
inconclusive
echo should_not_print
```

prints `before` then `TRAP_FIRED status=2` and stops — `should_not_print`
does not run, and the outer shell's own exit status (checked with `$?`
right after) is `2`, not `1`. That second fact is what makes the `redden()`
helper below possible: it lets a script reach the trap with a specific,
non-1 exit code without writing a bare `exit` statement.

## Classification

| script | has ERR-trap/verdict-function path? | pre-empting exits before repair? | class |
|---|---|---|---|
| `run-x86-prod-profile-boot-test.sh` | yes | no (fixed by #805) | has-verdict-path-and-clean |
| `run-x86-boot-tests.sh` | yes | yes — 1 preflight (`BREENIX_GATE_TMP`, before the trap check would matter) + 5 in-loop `exit 1` | has-verdict-path-but-preempting-exits |
| `run-x86-tty-oracle-gate.sh` | yes | yes — 1 preflight (`BREENIX_GATE_TMP`, **before its trap was even installed**) + 3 arg-parsing exits + 1 `sun_path` check + 1 missing-ext2-disk check + 10 in-loop exits (16 sites) | has-verdict-path-but-preempting-exits |
| `run-aarch64-tty-oracle-gate.sh` | yes | yes — 3 arg-parsing exits + 14 in-body/in-loop exits (17 sites); no `BREENIX_GATE_TMP` support to begin with | has-verdict-path-but-preempting-exits |
| `run-fs-fault-gate.sh` | yes | yes — 1 preflight (**before its trap was even installed**) + 2 arg-parsing (`exit 2`) + 1 build-warning check + 1 `fail()` helper + 2 `exit 0` success paths (7 sites) | has-verdict-path-but-preempting-exits |
| `run-ext2-lock-race-gate.sh` | yes | yes — 1 preflight (**before its trap was even installed**) + 1 arg-parsing (`exit 64`, F10's own distinct code) + 1 build-warning + 1 `fail()` helper + 5-branch `--park-only` cascade (0/1/2 distinct codes) + 1 final `exit 0` (10 sites) | has-verdict-path-but-preempting-exits |
| `run-coreproof-gate.sh` | yes | yes — 8 of 8 argument/validation `exit 2` sites reached before its trap was installed (the trap installed ~90 lines after the last of them) + 1 `exit 1` + 1 final `exit 0` (10 sites) | has-verdict-path-but-preempting-exits |

~~The other 29 of the 36 are **no-verdict-path**~~ **Correction (landing,
arithmetic).** `29 of the 36` is stale text left over from before this
doc's own `36`→`37` census correction above; it was never updated to
match. The itemized breakdown two paragraphs below (23 + 7) already totals
30, matching `37 - 7 = 30` (37 candidates, 7 has-verdict-path, per the
Classification table above), not `36 - 7 = 29`. The other 30 of the 37 are
**no-verdict-path**: they
print PASS/FAIL-shaped text (or a bisect/probe-shaped verdict) through their
own ad hoc means, with no `report_gate_failure`/`ERR`-trap backstop behind
it, so the #802/#805 idiom does not apply to them, and 0 of 30 were
touched. Two recognizable sub-shapes, listed rather than each individually
audited (per the task's own instruction: a script that reports only via
exit code is a different shape):

- **Ad hoc echo-then-exit, no trap backstop** (23 scripts — this doc
  originally said 22, an independent miscount found and fixed alongside
  F6): `run-aarch64-boot-test-native.sh`,
  `run-aarch64-boot-test-strict.sh`, `run-aarch64-full-test.sh`,
  `run-aarch64-kthread-parallel.sh`, `run-aarch64-percpu-stack-custody-gate.sh`,
  `run-aarch64-prod-profile-boot-test.sh` (the **aarch64** production-profile
  gate — a different script from the x86 one this whole arc is about),
  `run-aarch64-refusal-drain-gate.sh`, `run-aarch64-service-sequence-gate.sh`,
  `run-aarch64-stability-test.sh`, `run-aarch64-test-suite.sh`,
  `run-aarch64-testing-profile-boot-test.sh`, `run-boot-parallel.sh`,
  `run-kthread-parallel.sh`, `run-nonblock-eagain-test.sh`, `run-vmware-gate.sh`,
  `run-x86-gate.sh`, `scripts/772-diag-run-arm.sh`, `scripts/check-fs-fault-seams.sh`,
  `scripts/parallels/boot-cycle-test.sh`, `scripts/parallels/launcher-smoke.sh`,
  `scripts/run-arm64-boot-test.sh`, `scripts/test_tracing_via_gdb.sh`,
  `scripts/x86-gate-verdict.sh`. Each of these has its own retry loop,
  explicit per-check echo+return/exit, or classify-at-the-end shape (e.g.
  `run-aarch64-boot-test-native.sh`'s `run_single_test()` returns 1 on
  failure to a retry loop that itself decides `exit 0`/`exit 1` directly,
  with no trap involved). Retrofitting a `report_gate_failure` architecture
  onto any of these is a separate, larger change with its own review, not a
  #802-shaped fix.
- **Exit-code-primary, own text is diagnostic context rather than a
  PASS/FAIL banner** (7 scripts): `run-aarch64-arma609-arm.sh` (a
  classify-and-return helper feeding a caller's own exit code),
  `scripts/772-diag-aggregate.sh` (a stats aggregator, not a verdict),
  `scripts/772-dispatch-boot.sh` (`RESULT tag=... verdict_rc=$VERDICT_RC`,
  forwarding another script's exit code), `scripts/f21-bisect-verdict.sh`
  (git-bisect's own 0/1/125 convention), `scripts/f23-render-verdict.sh` and
  `scripts/f24-render-verdict.sh` (`Returns 0 if rendered content is
  present, 1 otherwise` — an embedded Python image check, no bash PASS/FAIL
  text), `scripts/x86-strand-census.sh` (5 distinct exit classes, own
  precedence rules, consumed by `run-boot-parallel.sh`).

## Repair

Applied the #805 idiom to each of the six `has-verdict-path-but-preempting-exits`
scripts. Two shapes:

**Where the trap was already installed before the pre-empting exit** (the
in-loop `exit 1`s in `run-x86-boot-tests.sh`, the arg-parsing/in-loop exits
in `run-x86-tty-oracle-gate.sh` and `run-aarch64-tty-oracle-gate.sh`, the
`fail()` helpers in `run-fs-fault-gate.sh`/`run-ext2-lock-race-gate.sh`):
the bare `exit N` becomes a bare `false` in place. `set -e` fires the
already-armed trap; `report_gate_failure`'s own `exit "$exit_code"` re-raise
is the one `exit` statement left.

**Where the pre-empting exit ran before the trap existed** (the
`BREENIX_GATE_TMP` preflights in `run-x86-tty-oracle-gate.sh`,
`run-fs-fault-gate.sh`, `run-ext2-lock-race-gate.sh`; the entire
argument-parsing/validation block in `run-coreproof-gate.sh`): <!--
claim-lint:ok: "Correction" paragraph immediately below states plainly that
this sentence was wrong; the corrected sentence follows it, with line
numbers. --> ~~the trap installation (and, for `run-coreproof-gate.sh`, the
`cleanup()` `EXIT` trap and a new `redden()` helper) moved earlier in the
script, to immediately after the variables `report_gate_failure` itself
reads (`QEMU_PID`, `CURRENT_SERIAL`/`OUTPUT_DIR`, already initialized).~~
**Correction (review round 1, F5).** That sentence has it backwards. In
`run-fs-fault-gate.sh` and `run-ext2-lock-race-gate.sh` the trap barely
moved at all (`trap ... ERR` sits at `:88`→`:89` and `:259`→`:259`,
`origin/main` vs. commit `5a5c1ce4` — the one-line shift in the first is an
inserted comment, not a relocation): what moved was the **check itself**,
downward, to just after the already-installed trap. `run-coreproof-gate.sh`
is the one script where the trap genuinely moved earlier (its
`cleanup()`/`redden()` install now runs before the argument-parsing block
that used to precede it, since that script had no `BREENIX_GATE_TMP` check
to relocate — the whole validation block needed the trap ahead of it). The
check itself then runs as the first thing under the armed trap — a
`BASE-DIR PREFLIGHT` block matching #805's own naming — rejecting with
`echo` + `false`.

**F5's other two commit-message errors, same correction.** <!--
claim-lint:ok: "Both false" resolves to the `grep -n 'BASE-DIR PREFLIGHT'
docker/qemu/*.sh` citation two sentences later in this same paragraph,
which names every file the block actually appears in (4 of the 91 `.sh`
scripts in the tree) -- run-x86-boot-tests.sh is not among them. --> Commit
`5a5c1ce4`'s own message (immutable — corrected here, not by rewriting
pushed history, per this project's own rule) also says: "run-x86-boot-tests.sh's
BREENIX_GATE_TMP absolute-path check ... moved to a BASE-DIR PREFLIGHT
block right after their ERR traps" and that run-x86-tty-oracle-gate.sh's
"AF_UNIX sun_path check" moved into that same block. Both false.
`run-x86-boot-tests.sh` has no `BASE-DIR PREFLIGHT` block at all (`grep -n
'BASE-DIR PREFLIGHT' docker/qemu/*.sh` finds it in
`run-x86-tty-oracle-gate.sh`, `run-ext2-lock-race-gate.sh`,
`run-fs-fault-gate.sh`, and `run-x86-prod-profile-boot-test.sh` only) — its
one preflight site converted `exit 1` → `false` in place, already past its
already-installed trap, nothing moved. `run-x86-tty-oracle-gate.sh`'s
`sun_path` check does sit after that script's `BASE-DIR PREFLIGHT` block
(the block ends before the argument-parsing loop; the `sun_path` check is
a separate, later block of its own, currently around `:177`-`:181`) — a
distinct check, not folded into it. And the message's "run-coreproof-gate.sh's
five usage-error sites (previously exit 2)" undercounts: there are eight
(`grep -c 'redden 2' docker/qemu/run-coreproof-gate.sh` → 8), which is what
this doc's own Repair section already says a few paragraphs below.

**Distinct exit codes** (`run-ext2-lock-race-gate.sh`'s `--park-only` mode:
0 PARK OBSERVED, 1 SPIN/NO PARK, 2 INCONCLUSIVE, review finding F10;
`run-coreproof-gate.sh`'s `exit 2` usage errors) needed a third shape. A
bare `false` yields a fixed status 1, which would collapse F10's three
outcomes onto one code. `redden()` is the fix:

```bash
redden() {
    return "$1"
}
```

Called as `redden 2` (a plain statement, not part of an `if`/`while`
condition or an `&&`/`||` chain), this fires the `ERR` trap with `$?` equal
to `2` — verified above — and `report_gate_failure`'s `exit "$exit_code"`
re-raise carries that code through. No `exit` statement appears at the
call site. `run-ext2-lock-race-gate.sh`'s five-branch `--park-only` cascade
converted from five independent `if` blocks each ending in its own `exit`
to one `if`/`elif` chain: the two FAIL-shaped branches (`SPIN, NO PARK`,
`INCONCLUSIVE`) call `redden 1`/`redden 2`; the success branch (`PARK
OBSERVED`, code 0) takes no exit-shaped action at all and the cascade falls
through — success needs no trap trip, only a normal return to the caller.
The ordinary (non-`--park-only`) verdict logic that used to run
unconditionally after the `PARK_ONLY` block moved into that same `if`'s
`else`, so it stays unreachable in `--park-only` mode -- the same effect
the `PARK_ONLY` block's own five `exit`s had.

`run-coreproof-gate.sh`'s eight validation sites (`unknown argument`,
`unknown component`, `unknown mode`, `unknown window`, `unknown profile`,
and three `<invalid|unknown> ... coverage name` sites) each call
`redden 2`, preserving the exit code the script used for a usage error
before this repair. This
script has no separate `fail()` helper; its one runtime-verdict `false` is
the `FAILED_BOOTS -ne 0` check after the boot loop, and its PASS path
(`echo "ARM64 CORE-PROOF GATE: PASSED"`) now falls off the end the same way
`run-x86-prod-profile-boot-test.sh`'s does, replacing a trailing `exit 0`.

`run-fs-fault-gate.sh`'s two `exit 0` success paths (armed vs. `--disarm`)
merged into one `if`/`else` so both converge without an early exit, matching
`run-x86-prod-profile-boot-test.sh`'s own PASS path — that script has
carried no `exit 0` at any point in this arc; its PASS message is the last thing the script
prints, then it falls off the end with the last command's (an `echo`, status
0) exit code.

**Correction (review round 1, F4, MAJOR).** The commit that first widened
this idiom to `run-fs-fault-gate.sh` (`5a5c1ce4`) converted that script's
two usage-error sites (`unknown argument`, `unknown shape`) from `origin/main`'s
`exit 2` straight to a bare `false` — silently collapsing them onto the
generic `exit 1` gate-FAIL code, the exact collision this whole section
argues must not happen, and which the repair correctly avoided for
`run-ext2-lock-race-gate.sh` (`redden 64`) and `run-coreproof-gate.sh`
(`redden 2`). Neither the commit message nor this doc disclosed it. Fixed
here: `run-fs-fault-gate.sh` now defines its own `redden()` helper
(immediately after its `trap ... ERR` line) and both usage-error sites call
`redden 2`, restoring the pre-`5a5c1ce4` contract. No caller in the tree
checks this script's exit code today (`grep` over `docker/`, `scripts/`,
`tests/`, `kernel/` for a check against its `$?` finds only doc-comment
references), so the silent collapse had no live consumer — it was still an
undisclosed contract change, now reverted.

## The widened ratchet

`tests/teardown_structure.rs` gains a generalized version of
`validate_x86_prod_profile_harness`'s own no-pre-empting-exit scan, derived
by content rather than a hardcoded file list:

- `shell_scripts_below()`/`gate_and_utility_shell_scripts()`: walks
  `docker/qemu/` and `scripts/` for each `.sh` file.
- `has_report_gate_failure_verdict_trap(script)`: true when the script both
  defines `report_gate_failure() {` and arms it (`trap ... ERR` containing
  that name).
- `report_gate_failure_status_variable(script)`: reads the function's first
  statement (`local <name>=$?`) to find the variable name each script's own
  re-raise uses — 6 of the 7 use `exit_code`, `run-coreproof-gate.sh` uses
  `status` — rather than hardcoding one name.
- `verdict_trap_has_no_preempting_exit(script)`: the rule itself, a direct
  generalization of the original scan — no line may open with the token
  `exit` except the exact re-raise of that script's own status variable. A
  script whose handler does not open with `local <var>=$?` is itself a
  violation, not a silent pass.

Two new tests:

- `gate_scripts_with_verdict_trap_have_no_preempting_exits` — the ratchet:
  census the two directories, filter to the verdict-trap shape, assert the
  no-pre-empting-exit rule on each. Carries a `>= 7` anti-vacuity floor on
  the census size (measured at exactly 7 today; free to grow, per the
  campaign's own [[gate-target-fidelity-528]]-style precedent against
  pinning a closed name list — #549/#551/#527-r1).
- `verdict_trap_no_preempting_exit_rule_is_not_vacuous` — mutation proof:
  the census finds `run-x86-tty-oracle-gate.sh`'s shape; on a scratch copy,
  planting a standalone `exit 1` preflight (mirroring #802's own shape —
  `if [ -z "$BREENIX_GATE_TMP" ]; then` / `exit 1` on its own line / `fi`)
  reddens `verdict_trap_has_no_preempting_exit` and the failure names the
  planted `exit 1` text; a second mutation deletes the `local exit_code=$?`
  opener and confirms that also reddens rather than silently passing.

### Anti-vacuity mutation, run directly

```
$ scripts/run-structure-tests.sh teardown_structure verdict_trap_no_preempting_exit_rule_is_not_vacuous
test verdict_trap_no_preempting_exit_rule_is_not_vacuous ... ok
```

(The mutation's own assertion — that the reddened result names `exit 1` —
is inside the test; a plain `ok` here means both the "must apply" and the
"must redden by name" checks passed. First attempt at writing this
mutation used `if [ -z "$BREENIX_GATE_TMP" ]; then exit 1; fi` **on one
line**, which the scan does not see — first-token-of-line is `if`, not
`exit`, the same blind spot #805's own PR body named for the case-arm form
of the original bug. Corrected to a standalone `exit 1` line before this
doc was written; disclosed here rather than silently fixed, since it is
direct evidence of the scanner's real (and pre-existing, inherited) limit.)

## Structure suites

```
$ scripts/run-structure-tests.sh teardown_structure
test result: ok. 85 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ~16s
```

(83 before this round; the two new tests above are the delta.)

Full `tests/*_structure.rs` sweep, run individually through
`scripts/run-structure-tests.sh` (the workaround `scripts/run-structure-tests.sh`
itself documents: `cargo test -p breenix --test teardown_structure` cannot
reach these tests in a worktree lacking the forked Rust library the root
crate's build script needs — a build dependency failure unrelated to what
these tests read):

| suite | result |
|---|---|
| aarch64_testing_profile_structure | ok, 2 passed |
| block_request_lifetime_structure | ok, 12 passed |
| context_restore_structure | ok, 97 passed |
| coreproof_component_h_structure | ok, 5 passed |
| coreproof_coverage_structure | ok, 4 passed |
| coreproof_mutation_register_structure | ok, 5 passed |
| coreproof_sites_structure | ok, 4 passed |
| degenerate_transfer_fd_validation_structure | ok, 4 passed |
| dispatch_path_lock_free_structure | ok, 4 passed |
| dispatch_strand_census_structure | ok, 7 passed |
| dma_and_log_sink_structure | ok, 4 passed |
| entry_point_df_structure | ok, 5 passed |
| exec_lock_order_structure | ok, 44 passed |
| exit_tally_structure | ok, 6 passed |
| ext2_lock_structure | ok, 36 passed |
| fork_lock_order_structure | ok, 10 passed |
| green_program_envelope_structure | ok, 14 passed |
| loopback_pump_structure | ok, 72 passed |
| masked_binary_load_structure | ok, 4 passed |
| mmap_floor_structure | ok, 9 passed |
| net_lock_structure | ok, 19 passed |
| poll_tcp_gate_wiring_structure | ok, 3 passed |
| preempt_bracket_structure | ok, 8 passed |
| serial_line_atomicity_structure | ok, 9 passed |
| signal_eintr_predicate_structure | ok, 2 passed |
| strand_handoff_structure | ok, 38 passed |
| syscall_return_register_structure | ok, 6 passed |
| teardown_structure | ok, 85 passed |
| ttbr0_shadow_reconciliation_structure | ok, 32 passed |
| tty_oracle_structure | ok, 14 passed |

**30 of 30 suites green, 564 cases, 0 failed.**

## Run proofs

Each of the 6 repaired scripts gets one default-env run and one simulated
preflight failure. aarch64 gates ran on this Mac; x86 gates ran on beast
(`breenix-x86` Incus container, clone `/root/breenix-verdict` at `5a5c1ce4`,
`BREENIX_GATE_TMP=/root/breenix-verdict-tmp`). Raw output for each of the 12
runs (6 scripts x 2 runs) is saved as its own file under
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/`,
named in each run's own "Full output:" line below.

**Correction (review round 1, F3, MAJOR).** F3 flagged two things: the four
aarch64 preflight-fail serials carried no host/SHA provenance header
(unlike the x86 pair, which do), and that these four "do not reproduce at
HEAD." The header gap was real; all four preflight-fail serials and the
four default-pass serials now carry one. The "does not reproduce" half
does not hold up under direct re-test: <!-- claim-lint:ok: "4 of 4" is
counted by the four named files on the next two lines, each with its
reproduced line number. --> re-running all four preflight-fail commands
against this same worktree at HEAD reproduces 4 of 4 byte-for-byte,
including the exact line numbers this doc already published
(`run-aarch64-tty-oracle-gate.sh:101`, `run-fs-fault-gate.sh:93` [now
`:105` after F4's own edit -- see that script's proof above],
`run-ext2-lock-race-gate.sh:263`, `run-coreproof-gate.sh` "at line 136")
-- *when the script is invoked directly* (`docker/qemu/run-....sh ...`,
its own `#!/bin/bash` shebang, which on this Mac resolves to the system
`/bin/bash`, the same way the gate's own callers and CI invoke it). Traced
the discrepancy: these scripts read `${BASH_LINENO[0]}` inside their ERR
trap handler, and macOS's system `/bin/bash` (an old, pre-GPLv3 build) and
a newer Homebrew `bash` on `$PATH` report a different `BASH_LINENO[0]` for
a `case` statement inside a loop -- the old system bash reports the
`case ... in` line, a newer bash reports the actual failing arm's line.
Reproduced directly on a minimal script sharing this exact shape: `/bin/bash`
(this Mac's system bash) reports the `case` line, a `bash` resolved from
`$PATH` (Homebrew, newer) reports the true failing line. Invoking these
gate scripts as `bash script.sh ...` instead of `script.sh ...` picks up
the different, `$PATH`-resolved bash and produces the higher line number
F3's own reproduction reported -- not a stale artifact, a different
interpreter than the one the gate's shebang, and its own CI/callers, use.

### Environment note

This git worktree has no `rust-fork/` checkout and no prebuilt userspace
artifacts (both gitignored, neither created by a fresh `git worktree add`),
so 4 of the 4 aarch64 gates below (each needing either `boot_tests`'s
embedded userspace ELFs, `include_bytes!` in the kernel's test registry, or
a populated ext2 disk) hit `ERROR: forked Rust library not found` trying to
build userspace from source inside this worktree -- the same failure that
blocks `cargo test` directly, per `scripts/run-structure-tests.sh`'s own
header comment. Read-only artifacts already built from the same tree
(`target/ext2-aarch64.img`, `userspace/programs/aarch64/*.elf`) were copied
in from the main checkout (`/Users/wrb/fun/code/breenix`, `.gitignore`d
build products, not repository content) rather than rebuilt, so these are
genuine boot-and-adjudicate runs on the repaired scripts, exercising the
repaired shell logic end to end, but the userspace/kernel binaries booted
were not built by this run (no userspace or kernel source changed on this
branch, so this substitution does not affect what the gates verify).
Beast's existing reference clone (`/root/breenix/rust-fork`)
has the fork checked out, so both x86 gates on beast built the kernel,
userspace, and ext2 disk from source in `/root/breenix-verdict` itself, no
substitution needed there.

### aarch64 (this Mac)

Host-load rule (<=2 concurrent QEMUs) checked via `pgrep -f '^qemu-system'`
before each boot; each of the 4 boots below ran with 0 or 1 other QEMU
already running (other worktrees' own gates), for a total of 1 or 2 --
confirmed via the same `pgrep` count each time, not assumed.

**`run-aarch64-tty-oracle-gate.sh`** (default: `--boots 1`)

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh
...
Booting the ARM64 production profile (boot 1/1)...
  boot 1: 14/14 arms PASS, kernel live (bsshd reached)
PASS: aarch64 TTY oracle gate - 1/1 boots, 14 arms green on the shipped production profile
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/aarch64-tty-oracle-default-pass.txt`.

Simulated preflight failure (unknown argument — this script has no
`BREENIX_GATE_TMP` support to begin with, per the census above, so its own
argument-parsing preflight is the analogous check):

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh --nonsense-flag
FAIL: unknown argument: --nonsense-flag
aarch64 TTY oracle gate: FAIL (set -e abort at docker/qemu/run-aarch64-tty-oracle-gate.sh:101, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/aarch64-tty-oracle-preflight-fail.txt`.

**`run-fs-fault-gate.sh`** (default: aarch64, armed, single boot)

```
$ docker/qemu/run-fs-fault-gate.sh
...
[gate] [FSFAULT:aarch64:COMPLETE:pass=8:fail=0]
  baseline_mount / baseline_read / short_read / eio_data_block / eio_recovery /
  corrupt_inode:arm=size / corrupt_inode:arm=blocks / liveness -- all PASS
fs fault gate (aarch64): PASSED - 8 arms green, kernel live after every injected fault
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/fs-fault-default-pass.txt`.

Simulated preflight failure (relative `BREENIX_GATE_TMP` — the check now
runs after the already-installed trap, per this round's repair):

```
$ BREENIX_GATE_TMP=relative-not-absolute docker/qemu/run-fs-fault-gate.sh
fs fault gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
fs fault gate: FAIL (set -e abort at docker/qemu/run-fs-fault-gate.sh:105, exit 1)
  failing command: false
```
exit 1 (re-captured for review round 1: F4's `redden()` addition shifted
this line from `:93` to `:105`; the failing command here is still the
BASE-DIR PREFLIGHT's own bare `false`, unaffected by F4, which touched only
the two argument/`--disarm` usage-error sites further down). Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/fs-fault-preflight-fail.txt`.

Simulated usage-error, added for review round 1 (F4): proves `redden 2`
preserves this script's pre-repair `exit 2` usage-error code, the same way
`run-coreproof-gate.sh --component Z` already proved it for that script.

```
$ docker/qemu/run-fs-fault-gate.sh --disarm bogus
unknown shape: bogus (expected short_read, eio or corrupt_inode)
fs fault gate: FAIL (set -e abort at docker/qemu/run-fs-fault-gate.sh:125, exit 2)
  failing command: return "$1"
```
exit **2** (not 1). Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/fs-fault-badshape-fail.txt`.

**`run-ext2-lock-race-gate.sh`** (default: aarch64, single race construction)

```
$ docker/qemu/run-ext2-lock-race-gate.sh
...
[gate] [LOCKRACE:COMPLETE:pass=2:fail=0]
  [LOCKRACE:ROOT:race:verdict=PASS:detail=no-spin-stall:parks=67]
  [LOCKRACE:HOME:race:verdict=PASS:detail=no-spin-stall:parks=63]
[gate] total parks observed: 130
ext2 lock-race gate (aarch64): PASSED - 2 filesystem(s) raced clean (130 total parks), kernel live after
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/ext2-lock-default-pass.txt`.

Simulated preflight failure (relative `BREENIX_GATE_TMP` — same repair
shape):

```
$ BREENIX_GATE_TMP=relative-not-absolute docker/qemu/run-ext2-lock-race-gate.sh
ext2 lock-race gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
ext2 lock-race gate: FAIL (set -e abort at docker/qemu/run-ext2-lock-race-gate.sh:263, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/ext2-lock-preflight-fail.txt`.

**`run-coreproof-gate.sh`** (`--seeds 1 --profile max`, to keep the run to
one boot instead of the 2026-08-18 gate-size default of 25/profile x2
profiles)

```
$ docker/qemu/run-coreproof-gate.sh --seeds 1 --profile max
...
=== profile max — 1 boot(s), component A, mode pen, window post_cohort, disarmed 0 ===
  max#1: harness violations=0 iters=54884
  max#1: clean (seed=0x000000000c489b88 iters=54884 sites=12/12)
boots=1 failed=0 violations=0 iters_total=54884 ...
ARM64 CORE-PROOF GATE: PASSED
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/coreproof-default-pass.txt`.

Simulated preflight failure (`--component Z` — this script has no
`BREENIX_GATE_TMP` support, so its own argument-validation preflight, now
routed through the `redden()` helper the repair added, is the analogous
check; also directly proves `redden N` preserves a non-1 code):

```
$ docker/qemu/run-coreproof-gate.sh --component Z
unknown component: Z

ARM64 CORE-PROOF GATE: FAILED
  at line 136: return "$1" (exit 2)
  started=... ended=...
```
exit **2** (not 1 — `redden 2` preserving the script's own usage-error code
through the trap, exactly as designed). Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/coreproof-preflight-fail.txt`.

### x86 (beast, `breenix-x86` container, clone `/root/breenix-verdict`)

`pgrep -f qemu-system-x86_64` checked <= 2 before each boot (0 running both
times).

**`run-x86-boot-tests.sh`** (`COUNT` defaults to 1)

```
$ BREENIX_GATE_TMP=/root/breenix-verdict-tmp BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library ./docker/qemu/run-x86-boot-tests.sh
...
[RECLAIM_DRAIN:nested=1:context_violations=0:selection_capped=3:injected=1:pend_epoch=0:pend_hw=0:pend_shadow=1:pend_selectable=0]
x86 frame-custody gate run 1: PASS
```
The script's own last statement is that `echo`, so it falls off the end with
status 0 — the same PASS-path shape `run-x86-prod-profile-boot-test.sh`
already uses, no `exit 0` needed. `pgrep -fl run-x86-boot-tests.sh` after
that line printed found 0 matches -- the process had exited.
`BREENIX_RUST_FORK_LIBRARY` points at beast's own long-lived reference
clone's fork checkout (`/root/breenix/rust-fork`, a read-only reference:
that clone was read from, and only from); the kernel, userspace, and ext2
disk this run booted were built fresh, from source, inside
`/root/breenix-verdict`. Full output (470 lines, covering each
FRAME_CUSTODY/PT_CUSTODY/oracle line the gate asserts on):
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-boot-tests-default-pass.txt`.

**`run-x86-tty-oracle-gate.sh`** (default: `--boots 1`)

```
$ BREENIX_GATE_TMP=/root/breenix-verdict-tmp BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library ./docker/qemu/run-x86-tty-oracle-gate.sh
Building the shipped x86_64 production kernel profile...
     Running `target/release/qemu-uefi`
Booting the x86_64 production profile with the TTY oracle (boot 1/1)...
  boot 1: 14/14 arms PASS, kernel live (bsshd reached)
PASS: x86 TTY oracle gate - 1/1 boots, 14 arms green on the shipped production profile
```
exit 0. No `--rebuild-userspace` was needed — the fresh clone's `cargo
build` step (via the kernel's own build script) produced the ext2 image the
gate needs as a side effect, so the run used fresh build output from
`/root/breenix-verdict`, with only the fork library itself read from
beast's reference clone. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-tty-oracle-default-pass.txt`.

Simulated preflight failures (both run before the default-env boots below
succeeded, so the trap-routed rejection was confirmed independent of
whether a real boot would later succeed):

```
$ env BREENIX_GATE_TMP=relative-not-absolute ./docker/qemu/run-x86-boot-tests.sh
x86 frame-custody gate: FAIL (set -e abort at .../run-x86-boot-tests.sh:93, exit 1)
  failing command: false
x86 frame-custody gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-boot-tests-preflight-fail.txt`.

```
$ env BREENIX_GATE_TMP=/root/breenix-verdict-tmp-ggg...ggg(147 chars) ./docker/qemu/run-x86-tty-oracle-gate.sh
FAIL: console socket path exceeds the AF_UNIX sun_path limit of 107 chars: "..." is 144 chars -- shorten BREENIX_GATE_TMP
x86 TTY oracle gate: FAIL (set -e abort at .../run-x86-tty-oracle-gate.sh:180, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-tty-oracle-preflight-fail.txt`.

## What is NOT claimed

- **This is not a rebuild demonstration for userspace or kernel source.**
  0 files under `kernel/`, `userspace/`, or `libs/` changed on this branch.
  The 4 aarch64 default-env runs above used a prebuilt ext2 disk and
  userspace ELF set copied in from the main checkout rather than rebuilt in
  this worktree (see "Environment note"); the 2 x86 runs on beast built the
  kernel, userspace, and ext2 disk from source.
- **Not a claim that the 30 no-verdict-path scripts are correct, complete,
  or free of their own defects.** They were read only far enough to
  classify them out of scope for this specific repair; 0 of the 30 were
  modified. (This bullet said 29 before the landing arithmetic correction
  above; the itemized 23 + 7 breakdown two sections up sums to 30, matching
  37 - 7, the count this bullet now uses.)
- **Not a claim that `redden()`'s exit-code preservation is exercised by
  the widened Rust ratchet.** `verdict_trap_has_no_preempting_exit` checks
  the absence of a pre-empting `exit` statement; it does not itself execute
  any script or assert what code a `redden N` call actually produces at
  runtime. That is checked separately, at runtime, by the
  `run-coreproof-gate.sh --component Z` simulated-preflight run above
  (`exit 2`, not the `false`-only fallback of `exit 1`) and by the small
  bash reproduction in the "Verified" section above, which shows the
  `redden N` pattern and its observed `TRAP status=2` output directly
  (not committed as a script; the snippet in that section is the
  reproduction).
- **Correction (review round 1, F1, BLOCKING).** This bullet used to close
  with: <!-- claim-lint:ok: the quoted sentence is the false claim being
  corrected, not this doc's own assertion; see the counts and line numbers
  immediately after it, which ARE this doc's claim (5 of 6 scripts, 16
  sites, each with its own line number). --> "each of the 6
  repaired scripts places `exit`/`false`/`redden N` on its own line
  specifically so the scanner *can* see it." That was false for 5 of 6
  — only `run-x86-boot-tests.sh` has zero such sites. Counted
  directly
  (non-comment lines where `false`/`redden N` follows a case label or an
  `echo` on the same line, rather than opening the line): `run-x86-tty-oracle-gate.sh`
  3 (`:161`, `:166`, `:168`), `run-aarch64-tty-oracle-gate.sh` 4 (`:104`,
  `:109`, `:111`, `:128`), `run-fs-fault-gate.sh` 2 (`:120`, `:130`, after
  the `redden 2` fix below — F4 renumbered this script by 12 lines),
  `run-ext2-lock-race-gate.sh` 1 (`:297`), `run-coreproof-gate.sh` 6
  (`:131`, `:140`, `:160`, `:172`, `:182`, `:195`) — 16 sites total. The
  false sentence neutralized the blind-spot disclosure it followed,
  reading as "the gap exists but this branch is outside it" when the
  branch was squarely inside it.

  **The gap itself is now closed (review finding F2, r157).**
  `verdict_trap_has_no_preempting_exit` in `tests/teardown_structure.rs`
  no longer checks only the first whitespace token of a raw line; it
  splits each non-comment line into individual statements on `;`, `&&`,
  and `||` (quote-aware — a `;` or `|` inside a single- or double-quoted
  string, e.g. `run-coreproof-gate.sh`'s embedded
  `awk ... '$1 == required { print $2; exit }'`, is not treated as a
  statement boundary, which a first cut at this fix false-positived on
  before the quote tracking was added) and checks the leading token of
  each statement instead of just the line's. <!-- claim-lint:ok: "always
  did" resolves to `verdict_trap_no_preempting_exit_rule_is_not_vacuous`
  (0 of 2 mutations there is new; both pre-date this round), the
  pre-existing test proving the standalone-line case, next to its new
  sibling below proving the 2 of 2 inline shapes this paragraph adds. -->
  A case-arm `exit 1` or an `||`-guarded group's
  `exit 1` now reddens the rule exactly like a standalone `exit 1` line
  always did.
  `verdict_trap_no_preempting_exit_rule_catches_inline_exit_shapes`
  reverts two of the real sites above
  (`run-aarch64-tty-oracle-gate.sh:104` and `:111`) to their pre-repair
  bare `exit 1` and confirms the rule reddens both, by name.

## Landing re-smoke

Re-run at the merged head (`git merge --no-ff origin/main`, merge commit
`8c87639a7069300c6498931d325a3cf0dad96f5b`; `git diff --stat origin/main..HEAD`
lists only this branch's own 21 files -- the 7 gate scripts, this doc, its
serials, and `tests/teardown_structure.rs` -- confirming the merge carried
0 conflicts, per `git status` reporting a clean tree right after it).
Everything below is this round's own run, not a re-quote of the F1-F6
proofs above.

**Full `tests/*_structure.rs` sweep**, run individually through
`scripts/run-structure-tests.sh` (same worktree workaround the F1-F6 proofs
used):

**30 of 30 suites green, 565 cases, 0 failed.** (One more case than the
83-scripts-ago count quoted above in "Structure suites" -- `teardown_structure`
itself grew from 85 to 86 passed between that snapshot and this one, matching
`grep -c '^\s*#\[test\]' tests/teardown_structure.rs` -> 86 on the file as
merged; no regression, the file is byte-identical pre- and post-merge, this
is just a later count of the same growing suite.)

**`scripts/test_claim_lint.py`**: `Ran 72 tests in 1.681s` / `OK`, exit 0.

**aarch64 (this Mac)**. Host-load rule checked (`pgrep -f qemu-system-aarch64`
-> 0 before each boot). Built via
`cargo build --release --features boot_tests --target aarch64-breenix-kernel.json
-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel
--bin kernel-aarch64` (clean, no warnings) then `scripts/check-kernel-no-neon.sh`
(PASS), matching the F1-F6 proofs' own "Environment note": this worktree has
no `rust-fork/` and no prebuilt userspace, so `userspace/programs/aarch64/*.elf`
and `target/ext2-aarch64.img` were copied in from the main checkout
(`/Users/wrb/fun/code/breenix`, gitignored build products, not repository
content; no userspace/kernel source changed on this branch) rather than
rebuilt in this worktree.

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh
...
  boot 1: 14/14 arms PASS, kernel live (bsshd reached)
PASS: aarch64 TTY oracle gate - 1/1 boots, 14 arms green on the shipped production profile
```
exit 0. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-landing-2026-09-05/aarch64-tty-oracle-landing-default-pass.txt`.

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh --nonsense-flag
FAIL: unknown argument: --nonsense-flag
aarch64 TTY oracle gate: FAIL (set -e abort at docker/qemu/run-aarch64-tty-oracle-gate.sh:101, exit 1)
  failing command: false
```
exit 1. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-landing-2026-09-05/aarch64-tty-oracle-landing-preflight-fail.txt`.

**x86 (beast, `breenix-x86` container, `/root/breenix-verdict` at
`8c87639a`)**. `pgrep -f qemu-system-x86_64` checked <= 2 before each boot
(1, then 2, both within the cap). Build clean first:
`cargo build --release --features boot_tests,testing,external_test_bins --bin
qemu-uefi` (3 lines of output total: `Compiling kernel`, `Compiling
breenix`, `Finished release`), then that same output grepped for
`^(warning|error)` -- 0 of 3 lines matched.

```
$ BREENIX_GATE_TMP=/root/breenix-verdict-tmp BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library ./docker/qemu/run-x86-boot-tests.sh 1
...
[RECLAIM_DRAIN:nested=1:context_violations=0:selection_capped=3:injected=1:pend_epoch=0:pend_hw=0:pend_shadow=1:pend_selectable=0]
x86 frame-custody gate run 1: PASS
```
exit 0. Captured output is a tail (the orchestrating session's own
command-output limit), not the F1-F6 proofs' full 470-line capture -- it
covers every FRAME_CUSTODY/PT_CUSTODY/TOMBSTONE/RECLAIM_DRAIN oracle line the
gate prints and its own final PASS verdict, which is what this re-smoke
needs. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-landing-2026-09-05/x86-boot-tests-landing-default-pass.txt`.

```
$ env BREENIX_GATE_TMP=relative-not-absolute ./docker/qemu/run-x86-boot-tests.sh 1
x86 frame-custody gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
x86 frame-custody gate: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:93, exit 1)
  failing command: false
```
exit 1. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-landing-2026-09-05/x86-boot-tests-landing-preflight-fail.txt`.

**Unattributed reds: 0 of 6.** Six re-smoke checks ran this round (the
structure-suite sweep, the claim-lint self-test, and the aarch64/x86
default-pass and preflight-fail pairs); each of the 6 matched its expected
verdict, so 0 needed separate triage.

## claim-lint

```
claim-lint: scripts/claim-lint.py                                              -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <repair commit message>          -> exit 0
```

---

## Addendum (`#812` landing, 2026-09-05) — the strict scorer's required-line list grows, and its fixtures are shared

`run-aarch64-boot-test-strict.sh`'s `score_serial` function is not fixed: it
has grown required-marker patterns across multiple branches
(`FCNTL_PM_CONTENTION_ORACLE_PATTERN` during `#796`, `IRQ_HOLD_ORACLE_PATTERN`
during `#812`), each `set -euo pipefail`-checked, and its scoring-only mode
(`BREENIX_STRICT_SCORE_ONLY=<path>`, no kernel or disk needed — the same mode
`run-aarch64-prod-profile-boot-test.sh` offers as `BREENIX_PROD_SCORE_ONLY`)
is what `tests/*_structure.rs` uses to replay a captured serial through the
scorer's current rules without booting QEMU. That gives the scorer's
committed serial fixtures — `docs/planning/green-program/aarch64-testing/serials/{asid-ratchet,slice3d}/*.txt`
at the time of writing — a property worth stating plainly: **a fixture is
scored against whichever scorer version replays it, not the version that
was current when it was captured.** A fixture recorded green before a
required-line addition scores `SCORE: FAIL` on that addition alone, with no
change to the kernel behaviour the fixture was ever meant to represent.

**Standing landing step.** A branch that adds a required line to either
aarch64 gate scorer's `score_serial` re-records, at the merged head, every
fixture path that `grep -rn "BREENIX_STRICT_SCORE_ONLY\|BREENIX_PROD_SCORE_ONLY"
tests/*.rs` finds threaded into that scorer's scoring-only mode as part of
landing — 2 of 2 at the `#812` landing (`slice3d/01-strict-boot1-serial.txt`,
`.../02-prod-boot1-serial.txt`), 1 of which needed the re-record and 1 of
which the production scorer's own absence-only rule still passed unchanged —
including a fixture a different, concurrently-landed branch added after this
branch forked. The branch adding the requirement did
not write that fixture and cannot have anticipated it, but the replay tests
run against the tree as merged, not as forked, so the re-record is still this
branch's landing step to take. Re-record from a boot of the merge commit's
own `--features boot_tests` kernel (`scripts/check-kernel-no-neon.sh` clean,
one boot with `pgrep -fl 'qemu-system-aarch64 -M'` reading 0 immediately
before and after launch — the fixed `/tmp/breenix_aarch64_strict_N` output
path this doc's own census above does not cover collides with a concurrent
lane's boot, and a contaminated capture can carry a plausible-looking oracle
line from the wrong kernel; verify the captured line's shape against
`strings` on the built ELF before adopting it), confirm the previously-red
replay test now passes, and record the fixture's directory README with the
merged-head SHA, the kernel `BUILD_ID`, and why. `#796` did this once for
`asid-ratchet/03-strict-boot1-serial.txt`; `#812`'s own review round did it
again for the same file; the `#812` landing did it a third time, for
`slice3d/01-strict-boot1-serial.txt`, a fixture `#812` never touched.
