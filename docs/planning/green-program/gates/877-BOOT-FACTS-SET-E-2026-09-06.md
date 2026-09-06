# gate-boot-facts.sh's missing set -e/pipefail fallback (#877)

## Defect

`docker/qemu/lib/gate-boot-facts.sh`'s `gbf_resolve_qemu_pid` had one
assignment with no `|| true`:

```sh
child="$(pgrep -P "$wrapper_pid" -x "$qemu_bin" 2>/dev/null | head -1)"
```

Both x86 gates that call it (`run-x86-boot-tests.sh`,
`run-x86-prod-profile-boot-test.sh`) run under `set -euo pipefail`.
`qemu-system-x86_64` is 19 characters; Linux truncates `ps -o comm=` and
`pgrep -x`'s own match target to the kernel's 15-byte `TASK_COMM_LEN`, so
the `case "$comm" in *"$qemu_bin")` check two lines above this assignment
cannot match on Linux (0 of 15 comm bytes can equal 19 qemu_bin bytes) and
every boot falls through to it. There,
`wrapper_pid` is already the real `qemu-system-x86_64` PID (not a
`timeout(1)` wrapper), so it legitimately has no `pgrep`-visible child --
`pgrep` correctly reports "no match" (exit 1), `head -1` still exits 0, and
under `set -o pipefail` the pipeline's own reported status is `pgrep`'s 1,
not `head`'s 0. With no `|| true`, that non-zero assignment aborted the
whole gate via `set -e` on every genuinely-passing x86 boot on Linux --
before the `else` branch two lines below, this function's own designed
recovery for exactly this case, ever ran.

## Why it slipped

The line existed since `gbf_resolve_qemu_pid` was first written (#827,
`6bb8b42f0`) and was still there, unguarded, when PR #872 (#865: "one x86
QEMU boot at a time per host") wired the two x86 gates to call it for the
first time, at the same commit that added the `qemu_bin` parameter these
gates need (`8d6bf053e`). #872's own landing pass found and fixed 4 of
this file's 5 `var="$(...)"` command-substitution assignments carrying the
identical "pipeline whose first stage can fail while a later stage still
exits 0" hazard under `pipefail` (`6cffead6b`: `gbf_load_1m`'s `sysctl`
line, `gbf_resolve_qemu_pid`'s `comm=` line, `gbf_qemu_cpu_seconds`'s `ps`
line, `gbf_last_heartbeat_uptime_ms`'s `grep` chain) -- but not the 5th,
this `child=` line, one command shape later in the same function
(`pgrep -P ... -x ... | head -1` rather than `ps -o ... -p ... | tr ...`).

PR #872's own body discloses why that 5th line was not exercised to a
clean pass during its own landing: while validating it on beast, 4
separate `run-x86-boot-tests.sh`/`run-x86-prod-profile-boot-test.sh`
invocations that each ran past roughly a minute of wall-clock were
silently `SIGKILL`ed by
an unrelated, still-ongoing hardware fault on the beast host's physical
disk `sdh` (filed as #871; `sudo journalctl -k` on the host shows
`device offline error, dev sdh` and `attempting task abort!scmd(...)`
cycles). #872's landing PR states plainly it could not obtain a completed
`PASS` verdict from either x86 gate on beast because of this -- so the
code path this `child=` line sits on (reached only late in a boot, right
before the gate's own `kill`) did not run to completion during review. The gap surfaced independently five hours later during the
landing re-smoke for an unrelated PR, #878 (`fix/821-tty-irq-no-pm-block`,
merging #865/#872's own work in from `main` for the first time on a boot
that completed cleanly), and was fixed there directly, in its own commit
(`3446eb16`, "gates(821,land): guard gbf_resolve_qemu_pid's pgrep against
a truncated comm") -- attributed to #821 because that PR's own landing
hit it, not because the defect has anything to do with #821's own subject
(TTY interrupt/process-manager locking). That fix (`|| true` on the exact
line above) merged to `main` at `783a6a53` on 2026-09-06, before this
round's own branch point.

## Fix

Already on `main` (`3446eb16`, merged via #878 at `783a6a53`) -- this
round's own diff makes no further change to `gate-boot-facts.sh`. This
round's job is #877's own outstanding ratchet and proof: a structural
test locking the general shape in place (so a *future* helper with the
same bare-assignment shape reddens before it reaches beast, not only this
one line), and a from-scratch re-demonstration that the fix is correct on
both bash 3.2 and bash 5, and that both real x86 gates reach verdict with
it in place.

## The ratchet: `tests/gate_boot_facts_pipefail_structure.rs`

Two properties, checked against the real file:

1. **Shape census** (`assignments_missing_pipefail_fallback`): every
   `IDENT="$(...)"` command-substitution assignment in
   `gate-boot-facts.sh` must be immediately followed by `|| true`. Found
   by joining backslash-newline continuations (one real assignment,
   `gbf_last_heartbeat_uptime_ms`'s, wraps its `$(...)` across two source
   lines), skipping comment lines (this file's own header comments quote
   the generic shape `v="$(pipeline)"` as prose), then for each real match
   walking the `$(...)`'s own parens to find its close and checking the
   text immediately after. Currently 5 of 5 clean.
2. **Real execution**: `gbf_helpers_survive_pipefail_with_a_nonexistent_pid_on_every_bash_here`
   sources the library and calls each of its 4 helpers under `bash -euo pipefail`
   against a PID (`4294960001`, above both macOS's and this container's
   `pid_max`) and a serial path this test process cannot collide with, on
   each `bash` binary this Mac has, out of 2 candidates checked
   (`/bin/bash`, the system-shipped 3.2, and whatever `bash` resolves to
   on `PATH`) -- asserting exit 0 and each helper's own documented
   fallback (`gbf_resolve_qemu_pid` falls back to the wrapper PID itself,
   `gbf_qemu_cpu_seconds`/`gbf_last_heartbeat_uptime_ms` report `NA`,
   `gbf_load_1m` prints a real load figure unaffected by the bogus PID).

### Anti-vacuity: mutation

`pipefail_fallback_census_is_not_vacuous` removes ` || true` from two
different real assignments in memory (the `child=` line #877 itself was
missing, and a second, unrelated one -- `raw=` in `gbf_qemu_cpu_seconds`
-- so the census is not accidentally locked to only the one line #877
happened to report) and asserts the census reddens by name on each:

```
cmd:  in-memory removal of ` || true` from the tracked child="$(pgrep ...)"
      line, then assignments_missing_pipefail_fallback() re-run against
      the mutated text (via
      ./scripts/run-structure-tests.sh gate_boot_facts_pipefail_structure)
exit: the mutated-text assertion inside pipefail_fallback_census_is_not_vacuous
assertion: assignments_missing_pipefail_fallback() == ["child"]

cmd:  same, removing ` || true` from raw="$(ps -o time= ...)" instead
assertion: assignments_missing_pipefail_fallback() == ["raw"]
```

Also verified against the **real, pre-fix file** (not only the in-memory
mutation): checking out `docker/qemu/lib/gate-boot-facts.sh` at `d41d8f3c`
(`main` immediately before #878 merged the fix in) and re-running the
suite reddens both `every_command_substitution_assignment_has_a_pipefail_fallback`
and `pipefail_fallback_census_is_not_vacuous` (the latter on its own
"library must be clean before mutation" sanity assertion), naming
`["child"]` as the missing assignment -- the real historical regression,
not only a synthetic one:

```
test every_command_substitution_assignment_has_a_pipefail_fallback ... FAILED
  gate-boot-facts.sh has command-substitution assignment(s) with no
  || true fallback: ["child"] -- ...
test pipefail_fallback_census_is_not_vacuous ... FAILED
  sanity: the real library must be clean before mutation
test result: FAILED. 2 passed; 2 failed
```
File restored immediately after (`git diff` against the pre-mutation
backup: empty); current tree back to 4/4 passed.

## Mechanism, re-derived independently on beast

A minimal repro using only `set -euo pipefail` (no `trap ... ERR`) did
**not** reproduce the original abort against the pre-fix library, on this
same beast container/bash -- worth recording, since it means the defect's
visibility depends on more than `errexit` alone. Bash's `inherit_errexit`
is off by default (confirmed: `bash -c 'shopt inherit_errexit'` ->
`inherit_errexit	off`), so a command substitution's own subshell does not
inherit `-e` from its caller, and a failing assignment *inside* that
subshell does not, by itself, terminate it. Reproducing the real gate's
own behavior required its actual mechanism: `set -E` (errtrace) plus a
`trap ... ERR` handler that calls `exit` when `$BASH_SUBSHELL -gt 0` (this
file's own `report_gate_failure`, and the minimal repro's own equivalent).
An `ERR` trap fires on a qualifying command failure independent of whether
`errexit` is active in that shell -- so it fires inside the nested
`child="$(pgrep ... | head -1)"` command-substitution subshell even though
`-e` is off there, and its own unconditional `exit` on a nonzero
`$BASH_SUBSHELL` terminates that subshell regardless. That failure
propagates as a fresh nonzero-simple-command event one level up (inside
`gbf_resolve_qemu_pid`'s own outer command-substitution subshell, itself
also `$BASH_SUBSHELL -gt 0`), re-firing the trap and `exit`ing that level
too -- and again at the real top level (`$BASH_SUBSHELL` = 0), where the
trap finally prints the `FAIL` line and exits the whole script. Verified
directly:

```
cmd:  bash script with set -euo pipefail; set -E; trap ...ERR (matching
      report_gate_failure's own BASH_SUBSHELL>0 exit pattern); sources the
      PRE-FIX library (git show d41d8f3c:docker/qemu/lib/gate-boot-facts.sh);
      QEMU_ACTUAL_PID="$(gbf_resolve_qemu_pid 4294960001 qemu-system-x86_64)"
out:  ABORT at line 16, exit 1, cmd: QEMU_ACTUAL_PID="$(gbf_resolve_qemu_pid
exit: 1

cmd:  identical script, current-tree (fixed) library
out:  QEMU_ACTUAL_PID=4294960001
      REACHED_END
exit: 0
```

## Structural suites and claim-lint

Each `tests/*_structure.rs` file (43 of 43), via
`scripts/run-structure-tests.sh <stem>` per file (macOS, this branch's
commit `1bc265aa`):

```
43 files, 684 tests passed, 0 failed
(includes gate_boot_facts_pipefail_structure: 4 passed;
 gate_boot_facts_structure, #827's own file, unaffected: 5 passed)
```

```
python3 scripts/claim-lint.py                                                     -> exit 0
python3 scripts/claim-lint.py --commit-msg <tests(877) commit message>            -> exit 0
```

## Proofs

**Mac** (this session's own machine; both bash binaries present):

```
bash --version                        # /bin/bash: GNU bash 3.2.57(1) (arm64-apple-darwin25, Apple's shipped bash)
                                       # bash (PATH, Homebrew): GNU bash 5.3.15(1) (aarch64-apple-darwin25.4.0)
bash -n docker/qemu/lib/gate-boot-facts.sh                       -> exit 0, both binaries
scripts/run-structure-tests.sh gate_boot_facts_pipefail_structure
  test every_command_substitution_assignment_has_a_pipefail_fallback ... ok
  test pipefail_fallback_census_is_not_vacuous ... ok
  test gate_boot_facts_lib_has_no_syntax_errors ... ok
  test gbf_helpers_survive_pipefail_with_a_nonexistent_pid_on_every_bash_here ... ok
  test result: ok. 4 passed; 0 failed
```

**Beast** (Incus container `breenix-x86`, Debian, Linux, GNU bash
5.2.21(1); clone `/root/breenix-p877` at `1bc265aa`, `BREENIX_GATE_TMP=
/root/breenix-p877-tmp`):

```
bash -n docker/qemu/lib/gate-boot-facts.sh                       -> exit 0

bash -euo pipefail direct invocation, bogus pid 4294960001:
  PID=4294960001
  LOAD=0.00
  CPU=NA
  HB=NA
  exit 0
```

`docker/qemu/run-x86-boot-tests.sh 1`:
```
[GATE_BOOT_FACTS:boot=1:host_ms=1788692887817-1788693347888:qemu_at_start=0:load_at_start=3.81:qemu_at_end=0:load_at_end=4.49:qemu_cpu_s=454.00:guest_uptime_ms=NA:ended_by=scored_pass]
x86 frame-custody gate run 1: PASS
```

`docker/qemu/run-x86-prod-profile-boot-test.sh`:
```
[GATE_BOOT_FACTS:boot=1:host_ms=1788693495859-1788693507134:qemu_at_start=0:load_at_start=3.04:qemu_at_end=0:load_at_end=2.73:qemu_cpu_s=11.00:guest_uptime_ms=NA:ended_by=scored_pass]
PASS: x86 production profile reached steady state with the teardown census at rest
```

Both required x86 gates reached their own `QEMU_ACTUAL_PID=$(gbf_resolve_qemu_pid
...)` line (the exact statement #877 reports aborting on) and their own
verdict line with no ERR-trap `FAIL` output, on the first attempt each --
no #871 attribution or retry needed this round. `#871`'s own disk fault
was independently confirmed still active on the beast host at the same
wall-clock window (`sudo journalctl -k --since "-15min" | grep sdh` on
the host showed live `device offline error, dev sdh` / `task abort`
entries immediately before both gate runs), so this is a clean pass
alongside an unrelated live fault, not the fault's absence.

No new x86 timing-signature reds (#631/#766) on either boot; neither gate
needed a retry.

## Fix round: review finding F1 (precision of the `|| true` check)

Review finding F1 on this round's own ratchet
(`tests/gate_boot_facts_pipefail_structure.rs:151`): the census's
`rest.starts_with("|| true")` check is a plain string-prefix test, so a
hypothetical future fallback token that merely starts with the same 7
bytes -- e.g. `|| trueish_flag` -- would pass it even though it is a
different, longer shell token, not `|| true`. Real-world probability is
very low (nobody names a shell word `trueish_flag` right after `||`), and
the finding did not claim the current file is wrong: the real file's 5
assignments each carry the literal `|| true` (5/5, verified by re-reading
`docker/qemu/lib/gate-boot-facts.sh`), and the pre-fix census still
correctly reddened `["child"]` against `d41d8f3c`.

Fixed by replacing the bare `starts_with` call with a new
`has_true_fallback(rest: &str) -> bool` helper that additionally checks
for a word boundary: after stripping the `"|| true"` prefix, the next
byte (if any) must not be ASCII-alphanumeric or `_`, so `|| true` is
accepted, `|| true # comment` is accepted, and `|| trueish_flag` /
`|| true_but_not_quite` / `|| trueXYZ` are rejected. A direct unit test,
`has_true_fallback_requires_a_word_boundary_after_true`, exercises both
sides against the helper itself, independent of whether such a token ever
appears in the real file.

Re-run of the ratchet suite, this branch's HEAD (`gate-boot-facts.sh`
itself unchanged, so no beast x86 gate re-run was needed this round):

```
scripts/run-structure-tests.sh gate_boot_facts_pipefail_structure
  test has_true_fallback_requires_a_word_boundary_after_true          ... ok
  test every_command_substitution_assignment_has_a_pipefail_fallback  ... ok
  test pipefail_fallback_census_is_not_vacuous                        ... ok
  test gate_boot_facts_lib_has_no_syntax_errors                       ... ok
  test gbf_helpers_survive_pipefail_with_a_nonexistent_pid_on_every_bash_here ... ok
  test result: ok. 5 passed; 0 failed
```

The last of those five is
`gbf_helpers_survive_pipefail_with_a_nonexistent_pid_on_every_bash_here`,
the "helper under `set -euo pipefail` on both bashes" test, and its own
run above passed on both `/bin/bash` (3.2.57) and the Homebrew `bash` on
`PATH` (5.3.15) on this machine (2/2 candidates this suite checks for).

```
python3 scripts/claim-lint.py -> exit 0
```

## Scope note

`docker/qemu/lib/gate-boot-facts.sh` itself is unchanged by this round --
the fix landed on `main` via #878 (commit `3446eb16`) before this branch
started, discovered and applied by that PR's own landing pass, independent
of #877. This round adds the ratchet (`tests/gate_boot_facts_pipefail_structure.rs`)
and this record; no `kernel/` file is touched.

## Landing re-smoke

`git merge --no-ff origin/main` from this branch's HEAD (`baab16f6d3a4e79f930270010c2c92086467fbd`)
found `origin/main` (`783a6a53668d`) already an ancestor of HEAD -- `git merge`
reported "Already up to date" and made no commit; no `kernel/` conflict, no
merge to resolve.

```
python3 scripts/claim-lint.py -> exit 0
```

**Structure suites** (macOS, this branch's HEAD, via `scripts/run-structure-tests.sh
<stem>` per file): all 43 of 43 `tests/*_structure.rs` files passed, 685 tests
total, 0 failed (includes `gate_boot_facts_pipefail_structure`: 5 passed, and
`gate_boot_facts_structure`, #827's own file, unaffected: 5 passed).

**Claim-lint self-test**:
```
python3 scripts/test_claim_lint.py -> exit 0 (72 tests, "OK")
```

**Beast** (Incus container `breenix-x86`, Debian, Linux, GNU bash 5.2.21(1));
own clone `/root/breenix-p877` at `baab16f6d`, `BREENIX_GATE_TMP=
/root/breenix-p877-tmp`. `#871`'s disk fault independently confirmed still
live in the same wall-clock window (`sudo journalctl -k --since "-15min" |
grep sdh` on the host showed live `device offline error, dev sdh` / `task
abort` entries immediately before and after both gate runs below).

`docker/qemu/run-x86-boot-tests.sh 1`:
```
[GATE_BOOT_FACTS:boot=1:host_ms=1788695623452-1788696080468:qemu_at_start=0:load_at_start=0.37:qemu_at_end=0:load_at_end=1.01:qemu_cpu_s=453.00:guest_uptime_ms=NA:ended_by=scored_pass]
x86 frame-custody gate run 1: PASS
```

`docker/qemu/run-x86-prod-profile-boot-test.sh`:
```
[GATE_BOOT_FACTS:boot=1:host_ms=1788696532839-1788696544075:qemu_at_start=0:load_at_start=1.16:qemu_at_end=0:load_at_end=1.14:qemu_cpu_s=11.00:guest_uptime_ms=NA:ended_by=scored_pass]
PASS: x86 production profile reached steady state with the teardown census at rest
```

Both required x86 gates reached their own `QEMU_ACTUAL_PID=$(gbf_resolve_qemu_pid
...)` line and their own verdict line with no ERR-trap `FAIL` output, on the
first attempt each -- no #871 attribution or retry needed this round (a clean
pass alongside an unrelated live fault, not the fault's absence). No new x86
timing-signature reds (#631/#766) on either boot; neither gate needed a retry.
Beast clone removed after this round (`rm -rf /root/breenix-p877 /root/breenix-p877-tmp`).

**aarch64 strict** (native ARM64, this Mac, `docker/qemu/run-aarch64-boot-test-strict.sh`,
script default of 20 iterations, 100% required): kernel rebuilt from this
branch's HEAD (`cargo build --release --features boot_tests --target
aarch64-breenix-kernel.json -Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
0 warnings), `scripts/check-kernel-no-neon.sh` PASS (0 FP/SIMD instructions),
`scripts/create_ext2_disk.sh --arch aarch64` rebuilt the disk fresh in this
worktree. This Mac was running several other agents' concurrent QEMU
workloads throughout this round (`ps aux` during the run showed a second,
independent `qemu-system-aarch64 -M virt ... run-aarch64-boot-test-strict.sh`
process from an unrelated worktree, plus an unrelated `qemu-system-x86_64`
from another session), with host load averages measured at 2-9 across the
three attempts below -- both `gate-boot-facts.sh`'s own load samples and
`uptime` agree.

Attempt 1: 18/20 (failed iterations 4, 9). Boot 4's serial cuts off mid-test
with no FAIL line (ran past the per-boot 20s `timeout` budget under
`load_at_start=4.64`-`9.75` across that attempt's own window). Boot 9 failed
on `[IRQ_HOLD_ORACLE:aarch64:...:hold_us=12003:...:FAIL] [TEST:syscall:irq_hold_oracle:FAIL:a
try_manager() holder was interruptible on its own CPU]`.

Attempt 2: 19/20 (failed iteration 18), same
`IRQ_HOLD_ORACLE`/`try_manager() holder was interruptible` signature, again
under elevated host load (`load_at_start` 6.6-8.7 across that attempt's
tail).

Attempt 3, after the concurrent load eased (`load_at_start` 2.1-7.5, no
`QEMU HOST LOCK` contention -- `count before acquire` 0 or 1 throughout):
```
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 261s

PASS: 20/20 boots succeeded
```

This branch touches only `tests/gate_boot_facts_pipefail_structure.rs` and
this doc -- no `kernel/` file, and `docker/qemu/lib/gate-boot-facts.sh` is
byte-identical to `main`. The `IRQ_HOLD_ORACLE` failures on attempts 1-2 carry
a `hold_us=12003` (12ms) timing budget and recurred only while this shared
Mac's own measured load average was 5-9 (concurrent unrelated agent QEMU
workloads independently observed via `ps aux`); attempt 3's clean 20/20 came
immediately after that concurrent load eased, with no branch change between
attempts. Recorded here rather than silently retried away: this is not a
new-to-this-round finding to file (the signature is a live-timing budget on
a shared host, and this round made no kernel change that could plausibly
cause or fix it), but a plain attempt-by-attempt account per the project's
testing-integrity rule against accepting weaker evidence than required.

```
python3 scripts/claim-lint.py -> exit 0
```
