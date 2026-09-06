# x86 prod gate: naming the console-prompt liveness failure cause (#884)

Branch `gate/884-x86-prod-prompt-verdict`, commits `d4686336` (the fix) and
`93266a10` (a same-round repair to the `#884` fix itself, described below).
Base: `26641429` (origin/main).

## What #884 reported

`docker/qemu/run-x86-prod-profile-boot-test.sh`'s console-prompt liveness
check ended in two bare `test` statements:

```sh
test "$PROMPT_BEFORE" -eq 1
test "$PROMPT_AFTER" -eq 2
```

#884's report, from a PR #883 landing attempt on beast: a boot with
`PROMPT_BEFORE=0`/`PROMPT_AFTER=1` (the steady-state prompt not yet printed
when the liveness sample was taken -- a real race under host contention,
not a defect in the sampling itself) left the run "with no proper error
reporting, GATE_BOOT_FACTS record, or ended_by marker."

## What this round found, and what it did not find

This round could not reproduce a shape where the bare `test` assertion
itself failed to reach `report_gate_failure`. `set -E` (errtrace) plus the
`trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR` armed at the top
of the script cover a failing top-level `test` the same way they cover
the ~40 other bare assertions in the file -- confirmed by a minimal
isolated repro (same three lines: `set -euo pipefail; set -E; trap ...;
test "$X" -eq 1`; script and captured output committed at
`884-x86-prod-prompt-verdict/minimal-isolated-repro.sh` and
`minimal-isolated-repro-output.txt` in this directory -- `TRAP_FIRED
exit=1 line=11 cmd=[test "$X" -eq 1]`, not a silent `set -e` death) and by
the fact that this script's own
`gate_scripts_with_verdict_trap_have_no_preempting_exits` /
`verdict_trap_has_no_preempting_exit` census (added by #818,
`tests/teardown_structure.rs`) already covered this script and already
passed at `26641429` before this round's changes.
claim-lint:ok: #818, #884; local run
`scripts/run-structure-tests.sh teardown_structure gate_scripts_with_verdict_trap_have_no_preempting_exits`
recorded below in the claim-lint section.

What this round DID find, reading the script's own control flow: the
GATE_BOOT_FACTS line (`gbf_emit_line`, `docker/qemu/lib/gate-boot-facts.sh`)
is computed and written exactly once, immediately after the steady-state
poll loop -- well before `PROMPT_BEFORE`/`PROMPT_AFTER` are even sampled.
Its `ended_by` field is derived purely from that poll loop's own break
reason (`scored_pass`/`crash_marker`/`qemu_exited_early`/`poll_exhausted`).
A boot that reaches steady state (poll loop scores `scored_pass`) and then
fails the later liveness check leaves that line reading `ended_by=
scored_pass` on disk and on stdout -- correct for what it measured, but
silent about the run's real, later outcome. Combined with the bare
`test`'s own generic diagnosis (a failing-command line and a re-grep of
today's marker counts, `print_observed_values`), a reader had no line
naming the liveness failure's cause and no way to tell a starved boot from
a kernel regression without re-deriving it from the raw serial tail by
hand. That gap -- not a set -e/trap bypass -- is what #884's "no ...
ended_by marker" line was pointing at, and it is what this round fixes.

## Fix

`docker/qemu/run-x86-prod-profile-boot-test.sh`:

1. The `-gt`/two `-eq` assertions are now one check-and-false block (the
   #818/#805 idiom already used elsewhere in this same script's BASE-DIR
   PREFLIGHT block): on failure it classifies the cause
   (`prompt_absent` when `PROMPT_BEFORE=0` -- #884's own reported shape;
   `prompt_stimulus_unanswered` when the liveness stimulus earned no new
   prompt; `prompt_count_unexpected` otherwise), re-emits GATE_BOOT_FACTS
   (file and stdout) with a fresh host-clock/guest-heartbeat/QEMU-lane
   sample and that `ended_by` value, prints a named FAIL line with the
   before/after counts, prints a #826-style host-contention read (elapsed
   host time, guest's last heartbeat, concurrent QEMU-lane count and host
   load at boot start vs. now), and only then reaches the same ERR trap
   via a bare `false`, not `exit` -- a mechanical description of the diff
   in commit `d4686336`, readable in the diff itself.
   claim-lint:ok: #884, commit `d4686336` (readable in
   `docker/qemu/run-x86-prod-profile-boot-test.sh` at that commit).
2. `PROMPT_BEFORE`/`PROMPT_AFTER` are pre-declared (`""`) near the top of
   the script so `print_observed_values` can report them under this
   script's own `set -u` even on the one failure path that precedes their
   real assignment (the steady-state poll verdict itself).
3. `print_observed_values` now also prints the specific before/after pair
   (`console prompt before/after liveness stimulus: N -> M (expected 1 ->
   2)`), not only today's re-grepped total.
4. A proof knob, `BREENIX_X86_PROD_FORCE_PROMPT_ABSENT`, forces the
   just-sampled `PROMPT_BEFORE` to `0` after a real boot has already
   reached steady state -- reproducing #884's exact reported shape on a
   real boot without needing to catch live host contention in the act.

### A same-round repair to the fix itself

The first version of this fix (commit `d4686336`) pushed the gap between
`CAPTURE_LINES="$(gcd_pass_report)"` and the liveness `kill "$QEMU_PID"`
site from 21 to 32 lines. `tests/gate_capture_drain_structure.rs`'s
`every_guest_kill_site_is_preceded_by_a_drain_decision` requires a
`gcd_drain_and_report`/`gcd_pass_report` call within its own
`KILL_WINDOW=30` lines before a guest kill -- caught live on beast at
commit `d4686336` running this branch's own `GATE_PREFLIGHT`
structure-suite step (`gate_capture_drain_structure` red; that first,
red run's own log was not preserved, only the repaired re-run's was).
Commit `93266a10` trims the knob's comment to one paragraph and the knob
itself to one guarded assignment line, bringing the gap back to 25 lines.
The evidence in `884-x86-prod-prompt-verdict/` in this directory (cited
throughout the rest of this doc) is from the repaired `93266a10` re-run,
not the red `d4686336` run.

## Oracle: `tests/teardown_structure.rs`

New predicate `x86_prod_prompt_liveness_failure_names_its_cause` and two
tests:

- `x86_production_profile_gate_prompt_liveness_failure_routes_through_verdict`
  -- asserts the predicate holds against the real script at HEAD.
- `x86_production_profile_gate_prompt_liveness_ratchet_is_not_vacuous` --
  three mutation legs, each proven to apply (`assert_ne!`) and each proven
  to redden the new predicate:
  1. Reverting the whole check-and-false block back to #884's own bare
     three-`test` shape (`replace_range` over the exact span the guard
     opened). This mutated script is also asserted to still PASS the
     existing, more general `validate_x86_prod_profile_harness` --
     confirming this narrower ratchet, not the pre-existing one, is what
     closes the gap #884 reported.
  2. Renaming the `prompt_absent` cause to `unknown`.
  3. Swapping the terminal `false` for `exit 1` -- asserted to redden
     BOTH this new ratchet AND the pre-existing, more general
     `verdict_trap_has_no_preempting_exit` census.

Local run (`scripts/run-structure-tests.sh teardown_structure`, rustc
--test path, not `cargo test` -- kernel-swap hazard, see that script's own
header):

```
test x86_production_profile_gate_prompt_liveness_failure_routes_through_verdict ... ok
test x86_production_profile_gate_prompt_liveness_ratchet_is_not_vacuous ... ok
test x86_production_profile_gate_verdict_discipline_holds ... ok
test x86_production_profile_gate_ratchet_is_not_vacuous ... ok
test gate_scripts_with_verdict_trap_have_no_preempting_exits ... ok
...
test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Also re-run clean at `93266a10`, and every other `tests/*_structure.rs`
file that references `run-x86-prod-profile-boot-test.sh` (found via
`grep -l run-x86-prod-profile-boot-test tests/*_structure.rs`):
`capture_bxcap_schema_structure` (24/24), `gate_boot_facts_pipefail_
structure` (5/5), `gate_structure_preflight_wiring_structure` (4/4),
`green_program_envelope_structure` (14/14), `qemu_host_lock_structure`
(3/3), `tty_irq_fg_structure` (10/10), `tty_irq_pm_structure` (9/9) --
`gate_capture_drain_structure` (6/6) is folded into the `93266a10` fix
above.

## Shell-level run: `BREENIX_X86_PROD_FORCE_PROMPT_ABSENT=1`, beast, `93266a10`

Full log: `884-x86-prod-prompt-verdict/force-prompt-absent-proof.txt`,
alongside this doc (612 lines,
`/root/breenix-884` on `breenix-x86`, `BREENIX_GATE_TMP=/root/breenix-884-tmp`).
The relevant lines:

```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
Booting the x86_64 production profile...
  [GATE_BOOT_FACTS:boot=1:host_ms=1788721287460-1788721299693:qemu_at_start=0:load_at_start=1.14:qemu_at_end=0:load_at_end=1.10:qemu_cpu_s=12.00:guest_uptime_ms=NA:ended_by=scored_pass]
  [GATE_BOOT_FACTS:boot=1:host_ms=1788721287460-1788721359977:qemu_at_start=0:load_at_start=1.14:qemu_at_end=0:load_at_end=0.64:qemu_cpu_s=12.00:guest_uptime_ms=NA:ended_by=prompt_absent]
x86 production-profile gate: console prompt liveness check failed -- before=0 after=2 (expected before=1, after=2, after>before)
  Pre-adjudicated host-contention read (#826-style): the steady-state poll loop above already scored this boot's own progress a pass (STEADY_STATE_LITERAL was seen once), so a starved guest that reaches steady state late in its own sampling window, or that is too starved to answer the liveness stimulus inside 60s, produces exactly this shape with no kernel regression involved. Elapsed host time from boot start to this failure: 72517 ms (host_ms=1788721287460-1788721359977); guest's own last observed heartbeat: guest_uptime_ms=NA; concurrent x86 QEMU lanes on this host: 0 at boot start, 0 now; host 1-minute load average: 1.14 at boot start, 0.64 now. A guest heartbeat far behind the elapsed host time, or an elevated concurrent-lane count or load figure, is the contention signature; crash markers or a disturbed teardown census (both still checked below -- this failure does not skip them) would instead point at a kernel regression.
x86 production-profile gate: FAIL (set -e abort at docker/qemu/run-x86-prod-profile-boot-test.sh:1335, exit 1)
  failing command: false
[CAPTURE_DRAIN:capture=absent:seq=-:edge=-:cpu=-:records=-:drain_ms=0]
[CAPTURE_DRAIN_EVENTS:last_events=none]
  preserved failing serial: /root/breenix-884-tmp/breenix_x86_prod_profile_failures/20260906T190240Z_2775915
--- observed values ---
  ...
  console prompt before/after liveness stimulus: 0 -> 2 (expected 1 -> 2)
```

This is the exact defect shape #884 reported (`PROMPT_BEFORE=0`, a real
boot that reached steady state), and each of the five elements DELIVER
item (1) asked for is present in the quoted log above: the verdict trap
fired
(`failing command: false`, not a silent `set -e` death), a second
GATE_BOOT_FACTS line names the cause (`ended_by=prompt_absent`, distinct
from the first line's `ended_by=scored_pass`), the FAIL line names
before/after counts, the #826-style host-contention read is printed, and
`print_observed_values` ran to completion afterward (serial preserved,
PCI/TTY/timer markers printed in the tail) -- no `exit` pre-empted any of
it.

`guest_uptime_ms=NA` in both facts lines on this run: the shipped x86
production profile's heartbeat line does not match
`gbf_last_heartbeat_uptime_ms`'s regex in this boot's captured serial --
true of the FIRST (pre-existing, `ended_by=scored_pass`) facts line too,
so this is not something the fix introduced; not investigated further in
this round (see "Deliberately not done").

Mutation proof for "restore the bare assertion, oracle goes red" is the
Rust-level mutation leg above (`x86_production_profile_gate_prompt_
liveness_ratchet_is_not_vacuous`, mutation 1), not a second beast boot:
the assertion this round restores is a source-text shape
(`test "$PROMPT_BEFORE" -eq 1` as a standalone statement vs. inside the
check-and-false block), which a structure test proves by direct
inspection strictly more reliably than re-triggering the same host-timing
race a second time on shared hardware would.

## Gates at HEAD (`93266a10`), beast, `breenix-x86`, clone `/root/breenix-884`

Both gates below ran with `BREENIX_GATE_TMP=/root/breenix-884-tmp` and no
force knobs -- ordinary runs.

**x86 production-profile gate x1**
(`884-x86-prod-prompt-verdict/x86-prod-gate-head-pass.txt`, 353
lines):

```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
  [GATE_BOOT_FACTS:boot=1:host_ms=1788722205756-1788722219032:qemu_at_start=0:load_at_start=1.91:qemu_at_end=0:load_at_end=1.77:qemu_cpu_s=13.00:guest_uptime_ms=NA:ended_by=scored_pass]
PASS: x86 production profile reached steady state with the teardown census at rest
...
  console prompt count over 60s: 1 -> 2
```

`PROMPT_BEFORE=1`/`PROMPT_AFTER=2` -- the healthy case, unaffected by this
round's changes to the failure path.

**`run-x86-boot-tests.sh 1` at HEAD**
(`884-x86-prod-prompt-verdict/x86-boot-tests-head-pass.txt`, 502
lines):

```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
x86 frame-custody gate run 1: PASS
```

## claim-lint

```
claim-lint: python3 scripts/claim-lint.py                                                -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg <d4686336 msg file>                -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg <93266a10 msg file>                -> exit 0
```

The first tree-wide run initially found 4 findings (2 in the gate script's
new comment block, 2 in the new Rust doc comment) -- all `universal-claim`/
`unproven-claim` hits on prose words ("every", "never", "proven",
"structurally") with no citation in the same paragraph, not correctness
findings. Fixed by adding one `claim-lint:ok: #884, proven by the
mutation legs in x86_production_profile_gate_prompt_liveness_ratchet_is_
not_vacuous ...` annotation to each of the two paragraphs; the run quoted
above (exit 0) is the fixed state.

## Not claimed

- Not claimed: that a bare `test "$PROMPT_BEFORE" -eq 1` ever actually
  failed to reach `report_gate_failure` on this branch's own bash version
  or on beast's. The isolated repro and the pre-existing
  `verdict_trap_has_no_preempting_exit` census both indicate it does not.
  What #884 reported and this round fixes is the missing named cause and
  the stale `ended_by`, not a trap bypass.
- Not claimed: a second, independent beast boot reproducing the pre-fix
  bare-assertion shape. The "restore the bare assertion" proof is the
  Rust mutation leg, which inspects source shape directly; see the
  "Shell-level run" section above for why that is the stronger check
  for this particular property.
- Not claimed: the ~40 other bare `test`/`marker_count` assertions in this
  script were rewritten to the check-and-false idiom. They were audited by
  reading each one (each is a plain top-level statement under the same
  `set -e`/`set -E`/ERR-trap architecture, none inside an `if`/`while`
  condition or an `&&`/`||` chain that would exempt it from `set -e`) and
  left as bare `test` on purpose: #884's own ask is scoped to the
  console-prompt liveness check, the generic verdict-trap architecture
  already covers the rest, and `validate_x86_prod_profile_harness`'s
  marker-assertion scan (`tests/teardown_structure.rs`) requires several
  of them to stay in the exact bare `test "$(marker_count "$X")" -eq N`
  shape it pins. Rewriting the ~40 into named-cause blocks with no
  reported defect behind most of them would be scope creep against a file
  this project's own CLAUDE.md already treats as a hot path for review
  overhead, not a correctness fix.
  claim-lint:ok: #884; audit performed by reading
  `docker/qemu/run-x86-prod-profile-boot-test.sh` line by line, same
  method as `scripts/claim-lint.py`'s own paragraph-level review.
- Not claimed: a root cause for `guest_uptime_ms=NA` on the shipped x86
  production profile. Observed identically on the pre-existing
  `ended_by=scored_pass` facts line (unchanged by this round), not
  introduced by this fix, and out of scope for #884.
- Not claimed: `PROMPT_AFTER=1`/`PROMPT_BEFORE=0` (#884's literal example,
  where the `-gt` check would also fail) was separately reproduced on
  beast. The force knob reproduces `PROMPT_BEFORE=0` with a real
  `PROMPT_AFTER=2`, which exercises the same `prompt_absent` classification
  arm and the same GATE_BOOT_FACTS re-emission and #826 read; the `-gt`
  literal in the combined guard is unchanged from `26641429` and is
  covered by the existing `x86_production_profile_gate_ratchet_is_not_
  vacuous` mutation legs (`"liveness assertion deleted"`).

## Files changed

- `docker/qemu/run-x86-prod-profile-boot-test.sh`
- `tests/teardown_structure.rs`
- `docs/planning/green-program/gates/X86-PROD-GATE-884-ROUND-2026-09-06.md` (this file)
- `docs/planning/green-program/gates/884-x86-prod-prompt-verdict/*.txt`,
  `.../minimal-isolated-repro.sh` (evidence)

No `kernel/` changes.

## Review fix round

Closes four findings from the review of this round's own doc/script (F6,
F8, F9 minors/nits; F12 nit). No finding required a behavior change to the
liveness verdict logic itself -- in order, F6/F8/F9/F12 below sit in the
failure-preservation plumbing, a doc evidence citation, a dead
pre-declaration, and a doc miscount, respectively.

**F6** (`docker/qemu/run-x86-prod-profile-boot-test.sh`, `report_gate_failure`'s
preservation block): `gate_boot_facts.txt` lived only under `$OUTPUT_DIR`,
which the next run's `rm -rf "$OUTPUT_DIR"` deletes, so the liveness-failure
record this round added had no durable copy once a later run started --
unlike `run-aarch64-prod-profile-boot-test.sh`'s own `failure_dir`, which
already carries this file. Fixed by copying `$OUTPUT_DIR/gate_boot_facts.txt`
into `$failure_dir` alongside the serials, guarded on the file existing (a
pre-boot abort has none yet). Proved live, not just read: a beast run with
`BREENIX_X86_PROD_FORCE_PROMPT_ABSENT=1` reproduced the exact `ended_by=
prompt_absent` failure this round's fix names, and the preserved
`failure_dir` (`/root/breenix-884-tmp/breenix_x86_prod_profile_failures/
20260906T203911Z_2964142/`) now contains `gate_boot_facts.txt` reading
`[GATE_BOOT_FACTS:boot=1:host_ms=1788727078348-1788727150935:qemu_at_start=
0:load_at_start=1.00:qemu_at_end=0:load_at_end=0.53:qemu_cpu_s=12.00:
guest_uptime_ms=NA:ended_by=prompt_absent]` alongside `serial_kernel.txt`,
`serial_user.txt` and `capture_drain.txt` -- full log and the failure_dir
listing committed at
`884-x86-prod-prompt-verdict/f6-fix-proof-gate-boot-facts-preserved.txt`.

**F8** (this doc, "What this round found" section): the claimed "minimal
isolated repro" for a bare `test` failure reaching `report_gate_failure`
under this script's `set -euo pipefail; set -E; trap ... ERR` shape did not
trace to any file on the branch. Re-run for real (not merely asserted):
`set -euo pipefail; set -E; trap 'echo "TRAP_FIRED exit=$? line=$LINENO
cmd=[$BASH_COMMAND]"; exit 7' ERR; X=0; test "$X" -eq 1` produced
`TRAP_FIRED exit=1 line=11 cmd=[test "$X" -eq 1]`, confirming the trap
fires rather than a silent `set -e` death. Script and captured output now
committed at `884-x86-prod-prompt-verdict/minimal-isolated-repro.sh` and
`minimal-isolated-repro-output.txt`; the doc paragraph now cites both by
path instead of asserting the repro existed.

**F9** (`docker/qemu/run-x86-prod-profile-boot-test.sh`, around the
`PROMPT_BEFORE`/`PROMPT_AFTER` declaration): two comments gave
contradictory rationales for the same `set -u` safety, and the
pre-declaration (`PROMPT_BEFORE=""` / `PROMPT_AFTER=""`) was dead code --
`print_observed_values` already reads both names as `${VAR:-unsampled}`,
and bash's `:-` form is unbound-variable-safe under `set -u` on an unset
name exactly as it is on an empty one, so the pre-declaration changed
nothing. Removed the pre-declaration and its comment; the one comment that
remains (at `print_observed_values`) is now the sole, correct rationale.
Re-verified by re-grepping every read site: 5 of 5 bare, non-`:-` reads of
these two names sit after their real assignment further down (the
liveness-verdict assertion and its branches, the FAIL line, and the
closing summary echo), matching the comment's own claim.

**F12** (this doc, the "Shell-level run" section's conclusion paragraph):
"each of the four elements ... asked for" was followed by an enumeration
of five (a)-(e). Corrected "four" to "five"; the enumeration itself
(items a-e) was unchanged -- re-counted here: 5 items, matching the
corrected word.

### Oracle + mutation re-run (local, this worktree)

```
$ scripts/run-structure-tests.sh teardown_structure x86_production_profile_gate_prompt_liveness
== compiling teardown_structure ==
== running teardown_structure x86_production_profile_gate_prompt_liveness ==

running 2 tests
test x86_production_profile_gate_prompt_liveness_failure_routes_through_verdict ... ok
test x86_production_profile_gate_prompt_liveness_ratchet_is_not_vacuous ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out; finished in 0.01s
```

The whole `teardown_structure.rs` file (92 tests, this file's full count)
also re-run clean after the F6/F9 script edits:
`scripts/run-structure-tests.sh teardown_structure` ->
`test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered
out`.

### x86 prod gate x1 (beast, `breenix-x86` container, `/root/breenix-884`)

Beast's clone was synced to this branch's HEAD (`40ac73ed`) plus this fix
round's working-tree diff; the two `git diff` outputs for
`docker/qemu/run-x86-prod-profile-boot-test.sh` (local worktree vs. beast
clone) hashed identically (`3674698eeaf4b591edb7872a986edc4d` both sides)
before the gate ran, so the run below exercises the fixed script, not some
other state.
The x86 QEMU host lock was held by two other lanes (`breenix-p766`, an
SMP-enum gate) in sequence during this run; both queue waits are the
lock's own designed behavior (`docker/qemu/lib/qemu-host-lock.sh`), not a
defect, and both are visible in the log as `QEMU HOST LOCK: waiting for
...` lines.

```
$ BREENIX_GATE_TMP=/root/breenix-884-tmp bash docker/qemu/run-x86-prod-profile-boot-test.sh
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
PASS: x86 production profile reached steady state with the teardown census at rest
...
[TIMER_SCALE_ORACLE:x86:ms_per_tick=5:ticks_before=54:ms=270:ticks_after=54:ticks_nonzero=1:in_range=1:PASS]
[INIT_DESIGNATION:x86_64:designated_pid=1:reserved_collisions=0]
  console prompt count over 60s: 1 -> 2
  (informational) total serial bytes at exit: 222441
[CAPTURE_DRAIN:capture=n/a:seq=n/a:edge=n/a:cpu=n/a:records=n/a:drain_ms=0]
[CAPTURE_DRAIN_EVENTS:last_events=n/a]
```

Full 352-line log committed at
`884-x86-prod-prompt-verdict/x86-prod-gate-fixround-pass.txt`. The 48/48
structure-suite preflight count matches this doc's earlier
`structure_suites=48/48` citation (same census, re-run against the fixed
script). Not claimed: a second, independent x86 prod gate run beyond this
one x1 -- the round's ask was x1, and the separate forced-failure run
above (F6) is a different code path (the liveness-failure branch, not the
liveness-pass branch this x1 run exercised), so it is evidence for F6
specifically, not a second pass-path sample.

### Claim-lint

```
claim-lint: scripts/claim-lint.py                                    -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/g884-commit/commit-msg-884-r2.txt -> exit 0
```

Two intermediate runs of `scripts/claim-lint.py` alone (before this
section reached its own final wording) found 1 and then 4 findings, both
sets self-inflicted by this section's own draft text (a universal-claim
quantifier word in the opening paragraph, and two word choices claim-lint
treats as asserting verification without a discharge in the same
paragraph) and closed by rewording, not by weakening the tool or adding a
mute-button annotation. Final tree-wide run, clean:

```
claim-lint: clean (10 file(s) checked, changed hunks vs 266414292ac9).
claim-lint: 192 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```

## Landing re-smoke

Landing this branch (`gate/884-x86-prod-prompt-verdict`, tip `4a378bd8`) into
`main` (`a9d4bd3e`) via `git merge origin/main` produced a clean, conflict-free
merge commit, `20b00edaa6a846b4387ad03b21a55b5b8bb27e5c`, 0 `kernel/`
path conflicts and 0 conflict markers in the 60 files the merge touched
(`git status` reports a clean working tree post-merge, `git diff --check`
finds 0 conflict-marker lines). The only overlap with this round's own
edited file, `docker/qemu/run-x86-prod-profile-boot-test.sh`, was `main`'s
unrelated addition of the `#766` `[TIMER_WAKE_LATENCY_ORACLE:` marker to the
`TEST_ONLY_MARKERS` array a few lines below this round's own edits; `git diff`
of that file between `4a378bd8` and `HEAD` shows exactly that one added line,
0 lines from this round's own script edits touched.

`main` at `a9d4bd3e` brought in two new `tests/*_structure.rs` files
(`dispatch_fact_census_structure.rs`, `timer_wake_dispatch_structure.rs`) and
edited three existing ones (`critical_path_logging_census_structure.rs`,
`dispatch_strand_census_structure.rs`, `tty_irq_pm_structure.rs`), plus a new
`gate-structure-preflight.sh` wiring (R191/PR-1) that now runs each of the 50
`tests/*_structure.rs` files as a preflight inside each of the four x86/aarch64
boot gates before it builds or boots anything. 0 of these are a fixture
replayed against a scorer that this branch's own changes touch -- this
round's own `tests/teardown_structure.rs` additions are static-shape checks
against `docker/qemu/run-x86-prod-profile-boot-test.sh`'s source text, not a
captured-serial replay, and the two new `SerialFixture` structs in
`tests/x86_gate_verdict_test.rs` (also from `main`) synthesize their fixture
content in a temp directory at test run time rather than reading a committed
capture -- so 0 committed fixtures needed re-recording at the merged head.

### `scripts/run-structure-tests.sh` (50/50 suites), local worktree, merge commit `20b00edaa6a846b4387ad03b21a55b5b8bb27e5c`

50 of 50 `tests/*_structure.rs` files present at the merged head, run one at a
time via `scripts/run-structure-tests.sh <stem>` (the script's own per-file
convention -- there is no single "all suites" invocation):

```
aarch64_testing_profile_structure:        ok. 2 passed; 0 failed
block_request_lifetime_structure:         ok. 12 passed; 0 failed
capture_bxcap_schema_structure:           ok. 24 passed; 0 failed
capture_path_lock_free_structure:         ok. 14 passed; 0 failed
context_restore_structure:                ok. 97 passed; 0 failed
coreproof_component_h_structure:          ok. 5 passed; 0 failed
coreproof_coverage_structure:             ok. 4 passed; 0 failed
coreproof_mutation_register_structure:    ok. 5 passed; 0 failed
coreproof_sites_structure:                ok. 4 passed; 0 failed
critical_path_logging_census_structure:   ok. 10 passed; 0 failed
degenerate_transfer_fd_validation_structure: ok. 4 passed; 0 failed
dispatch_fact_census_structure:           ok. 7 passed; 0 failed
dispatch_path_lock_free_structure:        ok. 4 passed; 0 failed
dispatch_strand_census_structure:         ok. 7 passed; 0 failed
dma_and_log_sink_structure:               ok. 4 passed; 0 failed
entry_point_df_structure:                 ok. 5 passed; 0 failed
exec_lock_order_structure:                ok. 44 passed; 0 failed
exit_tally_structure:                     ok. 6 passed; 0 failed
ext2_disk_size_structure:                 ok. 3 passed; 0 failed
ext2_lock_structure:                      ok. 36 passed; 0 failed
fcntl_pm_contention_gate_structure:       ok. 4 passed; 0 failed
fork_lock_order_structure:                ok. 10 passed; 0 failed
gate_boot_facts_pipefail_structure:       ok. 5 passed; 0 failed
gate_boot_facts_structure:                ok. 5 passed; 0 failed
gate_capture_drain_structure:             ok. 6 passed; 0 failed
gate_structure_preflight_wiring_structure: ok. 4 passed; 0 failed
green_program_envelope_structure:         ok. 14 passed; 0 failed
loopback_pump_structure:                  ok. 104 passed; 0 failed
masked_binary_load_structure:             ok. 4 passed; 0 failed
mmap_floor_structure:                     ok. 9 passed; 0 failed
net_lock_structure:                       ok. 19 passed; 0 failed
parallels_kill_by_name_structure:         ok. 4 passed; 0 failed
poll_tcp_gate_wiring_structure:           ok. 3 passed; 0 failed
preempt_bracket_structure:                ok. 8 passed; 0 failed
qemu_host_lock_structure:                 ok. 3 passed; 0 failed
qemu_kill_by_name_structure:              ok. 2 passed; 0 failed
ring_span_report_site_structure:          ok. 6 passed; 0 failed
serial_line_atomicity_structure:          ok. 9 passed; 0 failed
signal_eintr_predicate_structure:         ok. 2 passed; 0 failed
strand_handoff_structure:                 ok. 38 passed; 0 failed
syscall_return_register_structure:        ok. 6 passed; 0 failed
teardown_structure:                       ok. 92 passed; 0 failed
terminal_edge_capture_structure:          ok. 11 passed; 0 failed
timer_wake_dispatch_structure:            ok. 8 passed; 0 failed
trace_ring_depth_structure:               ok. 4 passed; 0 failed
ttbr0_shadow_reconciliation_structure:    ok. 32 passed; 0 failed
tty_irq_fg_structure:                     ok. 10 passed; 0 failed
tty_irq_pm_structure:                     ok. 9 passed; 0 failed
tty_oracle_structure:                     ok. 14 passed; 0 failed
x86_smp_enum_structure:                   ok. 6 passed; 0 failed
```

50/50 files, 0 failures across every suite (this branch's own
`x86_production_profile_gate_prompt_liveness_*` pair lives inside
`teardown_structure`'s 92, already re-run individually in the "Review fix
round" section above).

### x86 prod gate x1 (beast, `breenix-x86` container, `/root/breenix-884`, merge commit `20b00edaa6a846b4387ad03b21a55b5b8bb27e5c`)

Beast's `/root/breenix-884` clone had a stale, uncommitted working tree left
over from an earlier round on `40ac73ed` (the F6/F8/F9/F12 fix content,
already superseded by the pushed `4a378bd8` commit); it was reset to
`origin/gate/884-x86-prod-prompt-verdict` (`4a378bd8`) and then merged against
`origin/main` (`a9d4bd3e`) independently, reproducing the identical merge:
`git rev-parse HEAD^{tree}` matched the local worktree's merge tree exactly
(`ec07c02c37b5d0bddaf644e4c261c48dbc600bcb`, both sides). No `userspace/`
source changed in the merge (`git diff a9d4bd3e 20b00eda --stat -- userspace/`
is empty), so the clone's existing `*.elf`/font build artifacts (already
present from prior rounds' setup) needed no refresh.

```
$ BREENIX_GATE_TMP=/root/breenix-884-tmp bash docker/qemu/run-x86-prod-profile-boot-test.sh
[GATE_PREFLIGHT:structure_suites=50/50:critical_path_lines=259:pinned=120]
...
PASS: x86 production profile reached steady state with the teardown census at rest
...
[TIMER_SCALE_ORACLE:x86:ms_per_tick=5:ticks_before=51:ms=255:ticks_after=51:ticks_nonzero=1:in_range=1:PASS]
[INIT_DESIGNATION:x86_64:designated_pid=1:reserved_collisions=0]
  console prompt count over 60s: 1 -> 2
  (informational) total serial bytes at exit: 227816
[CAPTURE_DRAIN:capture=n/a:seq=n/a:edge=n/a:cpu=n/a:records=n/a:drain_ms=0]
[CAPTURE_DRAIN_EVENTS:last_events=n/a]
```

`PROMPT_BEFORE=1`, `PROMPT_AFTER=2` -- the liveness assertion this round's
fix targets, passing through the verdict trap rather than aborting. Full
353-line log committed at
`884-x86-prod-prompt-verdict/x86-prod-gate-merged-head-landing.txt`.

### `run-x86-boot-tests.sh 1` (beast, `breenix-x86` container, `/root/breenix-884`, merge commit `20b00edaa6a846b4387ad03b21a55b5b8bb27e5c`)

```
$ BREENIX_GATE_TMP=/root/breenix-884-tmp bash docker/qemu/run-x86-boot-tests.sh 1
[GATE_PREFLIGHT:structure_suites=50/50:critical_path_lines=259:pinned=120]
...
[TIMER_WAKE_LATENCY_ORACLE:x86:sleep_ms=10:peers=8:overrun_ms=45:bound_ms=100:quantum_ms=50:round_ms=400:wake_enqueues=1:peers_started=8:peers_spinning=8:backstops=0:setup_ms=547:window_ms=509:measured=1:PASS]
x86 frame-custody gate run 1: PASS
[CAPTURE_DRAIN:capture=n/a:seq=n/a:edge=n/a:cpu=n/a:records=n/a:drain_ms=0]
[CAPTURE_DRAIN_EVENTS:last_events=n/a]
```

`x86 frame-custody gate run 1: PASS`, 1/1 boots. Full 465-line log committed
at `884-x86-prod-prompt-verdict/x86-boot-tests-merged-head-landing.txt`.

### Landing re-smoke summary

| Gate | Where | Verdict | GATE_PREFLIGHT |
|---|---|---|---|
| `scripts/run-structure-tests.sh` (50 suites) | local worktree | 50/50 files, 0 failures | n/a (host-side rustc runner, not a booted gate) |
| x86 prod profile gate x1 | beast, `breenix-x86`, `/root/breenix-884` | PASS | `structure_suites=50/50:critical_path_lines=259:pinned=120` |
| `run-x86-boot-tests.sh 1` | beast, `breenix-x86`, `/root/breenix-884` | PASS (1/1) | `structure_suites=50/50:critical_path_lines=259:pinned=120` |

Not claimed: a second independent run of either beast gate beyond the x1
this landing step asked for; an aarch64 re-smoke (out of scope -- this
branch and its merge touch no aarch64-only path); that the merge commit SHA
above is what ends up on `main` after `gh pr merge` (a `--merge` merge keeps
this exact commit, but that is confirmed in the PR body / issue comment, not
here, since this doc is written before the push).
