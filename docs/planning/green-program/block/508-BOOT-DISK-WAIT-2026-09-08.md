# 508: preserve the x86 boot continuation during disk waits

Source revision: `b346196295db73f9fe4b44c8f080e75217f27693`.
Baseline: `4394409fca3932296f3468914b5be325ce0d48a6` plus the switch-counter and completion-marker instrumentation. The baseline's scheduler and completion policy were unchanged. Source citations below were re-derived after the code commit; a later documentation-only commit does not move these source lines.

## Problem and measured mechanism

The instrumented baseline passed `bash docker/qemu/run-x86-boot-tests.sh`, but its new oracle reported `switched_away=367:tests_completed=0:FAIL`. Its sampled switch preempt count was 0. The existing terminal tally was `exited=110 nonzero=0 failed=[]`. These readings are in `serials/508/baseline/markers.txt`; the complete serials and gate transcript are retained as gzip files beside it. The tally therefore did not establish that the boot registration block returned.

The current mechanism is more specific than the historical busy-spin description in issue 508:

1. `kernel/src/main.rs:590` calls `init_with_current`; `kernel/src/task/scheduler.rs:5472` registers the running boot thread as idle. It is not an ordinary sleepable worker with an independent idle task available.
2. `kernel/src/main.rs:1309` takes the boot scheduling brake. The registration block has 64 `test_exec` calls, from `kernel/src/main.rs:1843` through `kernel/src/main.rs:2082`.
3. `kernel/src/drivers/virtio/block.rs:401` waits through `Completion::wait_timeout_uninterruptible`. The existing non-idle sleep path at `kernel/src/task/completion.rs:301` identifies syscall context using a positive count, then releases one count at `kernel/src/task/completion.rs:342`. Before this repair, idle boot took that same path. The caller's count was a boot brake, not evidence of syscall context.
4. `kernel/src/interrupts/timer.rs:65` requests rescheduling when the quantum expires. The existing interrupt-return guard at `kernel/src/interrupts/context_switch.rs:178` already refuses kernel switches with a held count. The measured count of 0 distinguishes a released brake from a timer ignoring a still-held brake.
5. `kernel/src/interrupts/context_switch.rs:860` checks Ring 3 confirmation; once confirmed, the idle branch declines to restore the saved boot continuation. Thus allowing another process to run during a boot disk wait can strand the remaining registration calls.

The 673 / 718 disposition was read with `gh issue view 508 --comments`. This repair retains its blocked-state admission set, including `BlockedOnTimer` and excluding unconditional `BlockedOnIO` admission. It does not assert the all-producers invariant that disposition declined to assume.

## Chosen repair

Option (b), with the completion-side brake preservation required by the measurement. Option (a) would require converting boot from idle into a separately resumable worker; its present scheduler identity does not meet that prerequisite.

`kernel/src/task/completion.rs:253` recognizes the idle task before registering a sleeping waiter. `kernel/src/task/completion.rs:147` retains the scheduling brake, masks interrupts while checking the request's completion token and timeout, then executes `STI; HLT`. The IRQ completion at `kernel/src/drivers/virtio/block.rs:678` publishes the token. Interrupts can wake the CPU, while the boot continuation stays on it. The helper restores its added count and the caller's original interrupt state on return. This is an IRQ wait with the CPU halted, not a scheduler-blocking wait.

`kernel/src/per_cpu.rs:1373` applies the existing kernel-count refusal before the exception/blocked-state admission arms. This Tier-2 edit puts the count rule at the admission site; the completion-side change prevents the actual count donation. The other Tier-2 edit is `kernel/src/interrupts/context_switch.rs:402`, where the requested switch counter is published after same-thread decisions have been excluded. Both additions use no logging. Rustfmt also normalized existing formatting in those files. Tier-1 files were read only.

## Oracle and scorer

Grammar emitted by `kernel/src/boot/disk_wait_oracle.rs:34`:

```text
[BOOT_DISK_WAIT_ORACLE:x86:switched_away=<unsigned decimal>:tests_completed=<unsigned decimal>:PASS|FAIL]
```

The accepted line is exactly `[BOOT_DISK_WAIT_ORACLE:x86:switched_away=0:tests_completed=1:PASS]`. `tests_completed` counts completed registration blocks, not passing userspace programs. The companion final marker is `[TEST_EXEC_BLOCK_COMPLETE:x86:calls=64]`, emitted after `test_fbinfo` returns at `kernel/src/main.rs:2084`.

`kernel/src/boot/disk_wait_oracle.rs:4` declares plain atomics. `kernel/src/main.rs:1312` arms the boot identity before RING3_SMOKE; `kernel/src/main.rs:2261` closes the window before the final preemption release at `kernel/src/main.rs:2267`. Departures of that identity during the open window increment the counter. If boot is lost, later departures of the same idle identity also count; 367 is not claimed to mean 367 departures from the original disk instruction. A nonzero value establishes that the protected identity departed at least once.

`kernel/src/boot/disk_wait_oracle.rs:27` reports once, either at the final boot marker or through `kernel/src/syscall/handlers.rs:254` at the terminal tally if boot did not finish. The diagnostic `BOOT_DISK_WAIT_TRACE` line reports the sampled preempt count outside the switch path. Its `u64::MAX` sentinel means no switch sample was recorded; it is not a preempt-count reading. The oracle is compiled with noninteractive x86 `testing`, including `boot_tests`, so the ordinary testing image used by the parallel runner is covered too.

`scripts/score-boot-disk-wait.py:12` requires one accepted oracle plus one preceding final marker. It rejects missing, duplicate, malformed, incomplete, switched-away, and FAIL evidence. `scripts/x86-gate-verdict.sh:28` calls it, so both x86 runners consume the oracle. `tests/boot_disk_wait_structure.rs` pins the wait, admission ordering, publication site, final-call boundary, and scorer rejection cases.

## Busy-spin mutation

The mutation restores the baseline Completion policy and replaces its x86 `Cpu::halt_with_interrupts()` wait with interrupt-enabled `core::hint::spin_loop()`. It does not add timer logging or force a reschedule. The exact patch is `serials/508/mutation/source.patch`; its SHA-256 is `92a32304e6e40053340091be0ca161f6dde58a355e68d0c50107f2ac54b9a352`. The built image hash is retained in `serials/508/mutation/image.sha256`.

The wait ratchet rejected the mutant with exit 101 (1 failed, 3 passed). After building the mutant image, the source was restored to the committed revision and `git diff --exit-code` succeeded before the 70/70 structure preflight and `bash docker/qemu/run-boot-parallel.sh 5`. The transcript records both the source revision and mutation patch hash; the parallel runner boots the existing image without rebuilding it.

The mutation job launched at 1-minute load 1.32. The gate exited 1, with 0 passed and 5 failed. Each retained `markers.txt` under `serials/508/mutation-1/` through `serials/508/mutation-5/` contains an explicit FAIL oracle and no final block marker:

| Boot | switched_away | tests_completed | Final markers |
|---|---:|---:|---:|
| 1 | 606 | 0 | 0 |
| 2 | 601 | 0 | 0 |
| 3 | 626 | 0 | 0 |
| 4 | 597 | 0 | 0 |
| 5 | 568 | 0 | 0 |

This mutation tests the unsafe donation of boot's count during a busy-spin wait. Since the baseline already used HLT on its positive-count path, this is not evidence that HLT by itself is the repair. The restored brake and idle-specific wait are the relevant difference.

## Validation records

The baseline launched at 1-minute load 0.36. Its structure preflight passed 69/69 suites and its x86 build had no project diagnostics. The repaired testing-profile build completed without project diagnostics. The local repaired structure run passed 70/70 suites through `scripts/run-structure-tests.sh`; rustfmt checks passed for the changed kernel files. A separate original-HEAD snapshot with the new ratchet produced 3 failures and 1 pass (exit 101), while the repaired snapshot passed 4/4; the transcripts are `serials/508/validation/ratchet-original-head.txt` and `serials/508/validation/ratchet-repaired.txt`.

The repaired boot gate launched at load 5.85. The retained `serials/508/repaired-boot/gate.log.gz` records `REVISION=4394409fca3932296f3468914b5be325ce0d48a6` at decoded line 3. It contains no `SOURCE_REVISION` record or repair commit identifier. That transcript therefore does not establish that the boot ran repair commit `b346196295db73f9fe4b44c8f080e75217f27693`; the earlier attribution is withdrawn (V-3).

The first repaired parallel batch launched its gate wrapper at load 6.78 and then waited for the shared QEMU lock. The structure preflight passed 70/70 suites before boot. Its five boots each reached the final 64-call marker: 0 incomplete registration blocks. Boots 1, 2, 3, and 5 emitted accepted oracles. Boot 4 panicked later at `kernel/src/clock_gettime_test.rs:77`, with an elapsed reading of 1,992,843 ns, before the oracle-report site. Its missing oracle is rejected, not inferred as a pass. The full parallel gate exited 1 (0 passed, 5 failed). The retained evidence is under `serials/508/parallel-1-1/` through `serials/508/parallel-1-5/`; the first directory includes the complete batch transcript.

The second repaired parallel batch waited one five-minute load interval (10.71 initially, 3.94 at launch). Its preflight passed 70/70 suites. Its five boots each reached the final marker: again 0 incomplete registration blocks. Boots 1, 2, 3, and 5 emitted accepted oracles; boot 4 hit the same later kernel assertion with an elapsed reading of 4,392,079 ns and no oracle report. The full batch exited 1 (0 passed, 5 failed). Evidence is under `serials/508/parallel-2-1/` through `serials/508/parallel-2-5/`.

| Repaired run | Registration blocks completed | Oracle accepted | Full gate |
|---|---:|---:|---|
| Single boot | 1/1 | 1/1 | exit 1 |
| Parallel batch 1 | 5/5 | 4/5 | exit 1 |
| Parallel batch 2 | 5/5 | 4/5 | exit 1 |
| Total | 11/11 | 9/11 | no green full-gate claim |

`serials/508/validation/parallel-1-scorer.txt` and `serials/508/validation/parallel-2-scorer.txt` record the scorer results against the retained serial bytes. The two missing oracle reports are failures. The evidence establishes zero incomplete registration blocks in these 11 repaired boots; it does not establish an 11/11 passing protected-window oracle or userspace suite.

The production gate ran last with `bash docker/qemu/run-x86-prod-profile-boot-test.sh`. It waited one five-minute load interval (10.22 initially, 1.33 at gate launch), passed 70/70 structure suites, built without project diagnostics, and waited for the shared QEMU lock. Its actual QEMU launch load was 0.26. The gate exited 0, including console prompt liveness from 1 to 2 over 60 seconds. Evidence is in `serials/508/prod-last/`. No later kernel build or boot was performed.

No structure-preflight timeout occurred, so the 900-second retry provision was not needed. Structure preflight was never bypassed. The code commit and both repaired parallel batches used the same repaired image; its hash is `serials/508/validation/repaired-image.sha256`. The production build replaced that image only after both batches completed.

## Expanded-cohort failures

The repaired single-boot run reached the final block marker and the accepted boot-disk oracle, but the full x86 gate failed after exhausting its poll loop. Its transcript records `ended_by=poll_exhausted` and exit 1; the actual QEMU launch load was 6.50. `serials/508/repaired-boot/followup-markers.txt` retains the separate `argc: 0` / `ARGV_TEST_FAILED` reading and an `EXT2_LOCK_SPIN_STALL` for `ROOT_EXT2_write`. The latest retained dispatch census names 13 saved blocked threads; it is a snapshot, not evidence that those threads can never resume.

The direct x86 argv initialization failure is tracked in 973. The separate kernel clock precision panic is tracked in 979. The ext2 symptom is relevant to the existing investigations in 728 and 748; this round has not established an identical root cause. No allowlist or terminal-verdict criterion was relaxed to turn these results green. Returning from the registration block and completing the userspace suite remain distinct claims.

## Claim lint

Literal command records before the code commit:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-code-commit.txt -> exit 0
```

The first documentation lint found an unquantified suite-count statement; it was changed to the measured 70/70 count:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 1
```

Final documentation-commit checks:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-docs-commit.txt -> exit 0
```

The tree command checks changed text hunks; gzip artifacts are not linted as prose. It does not validate runtime claims.

## Not claimed

- Green full testing gates, or 11/11 accepted boot-disk oracle reports.
- A root cause for the later ext2 wait symptoms or kernel clock precision panics.

- That changing a spin instruction to HLT alone repairs a released boot brake.
- That the boot thread now uses a scheduler-blocking completion wait.
- That returning from the 64 registration calls constitutes a passing userspace verdict; the terminal tally is separate evidence.
- A new `BlockedOnIO` all-producers invariant, or closure of 666 or the staged-registry work in 533.
- Removal of pre-scheduler polling, or changes to ordinary non-idle completion waits.
- Aarch64 runtime coverage or SMP x86 coverage.

## Astra fix pass, 2026-09-08: V-1 and V-2

The input revision was `48610ba15e16a3a0785c7398e758cde9e2ed43a0`.
Tier-2 changes are confined to the defects in `kernel/src/interrupts/context_switch.rs`
and `kernel/src/per_cpu.rs::can_schedule`. V-1 needs a publication-site change;
nonintrusive debugging cannot repair the premature increment. V-2 needs removal
of the emitting diagnostics themselves. This pass does not edit the five Tier-1
files named in the lane brief.

V-1: the oracle now reads the resolved per-CPU thread identity after
`switch_to_thread` returns. It increments only when the outgoing thread is the
recorded boot thread and the resulting thread differs. TLS and first-entry
rollback restore the outgoing identity before that observation. The preemption
count is sampled before the switch and published only with a departure.
`tests/boot_disk_wait_structure.rs` pins that ordering and executes the extracted
measurement block against rollback, departure, non-boot, and absent-pointer
inputs. These are host tests of the publication logic, not injected live TLS
failures.

V-2: the context-switch implementation no longer contains direct logger calls
or raw serial writers. `can_schedule` loses its warning, periodic port writes,
and their diagnostic-only counters. Refusal counters, dispatch abandonment
records, and scheduler safety actions remain. The refusal helper's removed
formatting arguments also require a caller update in
`kernel/src/tracing/providers/teardown.rs`. Per-CPU initialization diagnostics
and the idle thread's deferred log drain are outside the switch/admission path.

The diagnostic ratchets are updated with the code they pin:
`tests/critical_path_logging_census_structure.rs` removes 15 logger sites from
its expected total (119 to 104); `tests/serial_line_atomicity_structure.rs`
removes the deleted writers; `tests/dispatch_strand_census_structure.rs` now
requires an empty logger census in the context-switch file.
`tests/dispatch_fact_census_structure.rs` retires the serial-only KernelFrame
split while retaining the `IdleRestoreError` publication check.
`tests/context_restore_structure.rs` retains the missing-CR3 refusal counter
and safety checks and retires the requirement to print a breadcrumb.

Regression evidence: `serials/508/review-fix/ratchet-original-head.txt` records
4 passing and 2 failing tests on the original source, one failure per finding.
`scripts/test-boot-disk-wait-mutations.py` reruns eight named source mutations
through `scripts/run-structure-tests.sh`; the exit-101 results are in
`serials/508/review-fix/mutations.txt`. Its finally blocks restore each changed
source before the next case. Scorer rejection cases and the existing in-suite
mutations run with the structure suites.

Not claimed by this review pass: live TLS-failure injection, aarch64 runtime
coverage, SMP coverage, removal of initialization diagnostics, or retirement
of the runtime limitations recorded earlier in this document. Gate results
and the final source revision are recorded below after validation.

Pre-commit validation: the local shared preflight passed 70/70 suites
(`serials/508/review-fix/structure.txt`); the focused suite passed 7/7
(`serials/508/review-fix/ratchet-repaired.txt`). Rustfmt checks passed for the
three changed kernel files. The x86 testing-profile build completed with
exit 0 and no project compiler diagnostics. The first x86 gate attempt
launched at load 1.14 and exited 1 in preflight: a diagnostic-publication
array still declared 16 entries after its serial-only row was removed.
The corrected array has 15 entries; its suite then passed 7/7 locally.
No QEMU process was launched by that failed preflight.

Code-commit checks:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-review-code-commit.txt -> exit 0
```

### Committed-revision rerun

The code revision is `c236b037d86e5d7e0cd04fc3c700d52f53a45bab`, pushed on
`sched/508-boot-thread-disk-wait`. Source citations were re-derived from that
commit in `serials/508/review-fix/source-citations.txt`: the outgoing count
snapshot is at `kernel/src/interrupts/context_switch.rs:363`, the switch call
at line 367, the resolved-ID read at line 381, and the increment at line 384.
The line references in the earlier sections describe their named historical
revisions, not this revision.

The committed-revision x86 gate launched at 1-minute load 0.91. Its preflight
passed 70/70 suites, with the context-restore suite finishing in 258 seconds
under its 300-second budget. There was no structure timeout and no 900-second
suite-timeout rerun. The gate rebuilt without project compiler diagnostics.

The QEMU launch after preflight/build recorded load 2.89. The full gate exited
1 with `ended_by=poll_exhausted`; its terminal completion requirements were not
met. `serials/508/review-fix/gate.log.gz` preserves the transcript with the
revision it ran at. `serials/508/review-fix/gate-result.txt` names the decoded
source file and line for each retained marker.

The saved `serials/508/review-fix/serial_user.txt.gz` contains one final
64-call registration marker followed by one accepted boot-disk oracle:
`switched_away=0`, `tests_completed=1`. Running the scorer on the two decoded
serial files exited 0 (`serials/508/review-fix/oracle-score.txt`). The timer-wake
latency oracle also passed, with 42 ms overrun against its 100 ms bound.
The same user serial later contains `ARGV_TEST_FAILED` and
`EXT2_LOCK_SPIN_STALL lock=ROOT_EXT2_write`; these remain failed runtime
observations. Follow-ups 973, 728, and 748 were confirmed open. This pass does
not establish an identical root cause for the ext2 observations or claim a
green full gate. The gate's tracked QEMU PID was absent after exit; no
process-name kill was used.

V-1 and V-2 are closed at the code-finding scope by the regression tests,
mutation rejections, and source changes described above. This disposition
does not close issue 508 or its runtime follow-ups.

Evidence-commit checks:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-review-evidence-commit.txt -> exit 0
```

## Landing

V-3: corrected the revision attribution inline in Validation records. The
single-run transcript retains its original bytes; its missing revision record
has not been backfilled.

V-4: R245 lane success remains pending. Issue 508 was OPEN with closedAt=null
when checked for this landing attempt. The historical 11/11 final-marker tally,
9/11 accepted oracles, and 5/5 runtime mutation rejections are repair evidence,
not issue closure or a completed atlas transition. No merged PR or completed
atlas-cell update is claimed. The external scratchpad `atlas/atlas-data.json` block/storage x86 summary now
records that pending condition inline; its partial/LOW status is retained.
V-4 is corrected at the prose scope, not fulfilled at the lane-success scope.

Deferred code findings supplied for this landing: [] (0 findings).


Two explicit-file lint attempts exited 1 on pre-existing auto-close phrases in
the external atlas. Those references now insert the word issue. Recheck records:

Prose-correction checks:

```text
claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/block/508-BOOT-DISK-WAIT-2026-09-08.md ../../atlas/atlas-data.json -> exit 0
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-prose-commit.txt -> exit 0
```


Main integration: `git fetch origin` followed by `git merge --no-ff --no-commit origin/main`
merged `e664e4bc` without conflicts. The merge commit also records the prose corrections.

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-merge-commit.txt -> exit 0
```


The initial merged-tree structure run passed 71/71 suites at
`327b42f4ab2ce221a575c226229addb8ebe7a3fc`
(`serials/508/landing/structure.log`). R182: main added the BSSH publickey
requirement to the strict aarch64 scorer. Its fixture change appended the marker
to the older capture. This landing replaces the fixture with the complete retained
`docs/planning/green-program/libbreenix/serials/430-420/landing/strict-1-serial.txt`
capture, which the merged strict scorer accepts in score-only mode (exit 0).
This is reuse of recorded serial bytes, not a new boot or fabricated marker.
The structure suites are repeated after that fixture replacement.

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-fixture-commit.txt -> exit 0
```

Fixture-replacement structure validation passed 71/71 suites (exit 0), recorded
in `serials/508/landing/structure-fixture.log`. The Mac soft-float boot_tests
build exited 0 with the accepted pinned-core future-incompatibility notice and
no project diagnostics (`serials/508/landing/aarch64-build.log`).


### Landing disposition: NOT LANDED

Required gates ran against committed revision
`6cff8cf8c43adeeccd8d6088a5c42af70091233e`, which includes the merge of main.
The aarch64 strict command `bash docker/qemu/run-aarch64-boot-test-strict.sh 1`
exited 1: 0/1 successful boots, 1 failure, 0 inconclusive. Its 71/71 structure
preflight passed. The scorer rejected the missing BSSH publickey oracle after
`ended_by=hard_timeout`. The complete gate transcript is
`serials/508/landing/aarch64-strict.log`; the captured serial is
`serials/508/landing/aarch64-serial.txt`. This is a failed shared-code check;
no causal attribution to this repair or the supplied userspace binaries is made.

The x86 single command `bash docker/qemu/run-x86-boot-tests.sh` launched at
1-minute load 5.40. After the aarch64 failure, its verified process tree was
terminated during structure preflight, before QEMU boot. Its partial transcript
is `serials/508/landing/x86-single-cancelled.log`; it is cancelled, not GREEN.
`bash docker/qemu/run-boot-parallel.sh 5` was not launched under the stop rule.
No structure-timeout retry was required. No production gate was requested or run
in this landing attempt. The required gate set is therefore not satisfied.

PR 980 remains unmerged and issue 508 remains open. The local main comparison
`git log origin/main..HEAD` is nonempty; there is no landed merge SHA or verified
merge timestamp. V-3 and V-4 prose corrections do not establish R245 lane success.
The branch and local worktree are retained for follow-up. The x86 lane clone and
temporary directory were removed after confirming no process used their working
directories, executables, or open descriptors. The gate reported aarch64 QEMU
count 0 at exit; the cancelled x86 run had not launched QEMU.

Not claimed by this landing: passing required boots, an 11/11 accepted historical
oracle tally, closure of 508 or 666, atlas promotion, a new runtime mutation run,
or a root cause for the missing BSSH oracle. Deferred code findings remain []
(0 supplied findings); the failed gate is recorded here and on issue 508.

```text
claim-lint: python3 scripts/claim-lint.py --files .tmp/508-issue-comment.md .tmp/666-comment.md -> exit 0
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/508-landing-commit.txt -> exit 0
```
