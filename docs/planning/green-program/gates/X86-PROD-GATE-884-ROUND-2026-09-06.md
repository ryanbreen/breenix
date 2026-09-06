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
test "$X" -eq 1`) and by the fact that this script's own
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
boot that reached steady state), and each of the four elements DELIVER
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
- `docs/planning/green-program/gates/884-x86-prod-prompt-verdict/*.txt` (evidence)

No `kernel/` changes.
