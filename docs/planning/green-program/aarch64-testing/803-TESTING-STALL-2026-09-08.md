# Issue 803: testing-profile remeasurement, 2026-09-08

Result: **0/12 issue 803 stalls**, **12/12 issue 761 loader-marker reds**,
**0/12 issue 562-family outcomes**, **0/12 profile passes**. This is a
docs-only delivery under the requested non-reproduction rule. No kernel,
scorer, test, or gate source was changed. No mechanism is assigned to the
historical 803 stall, and no fix or mutation was attempted.

## Revision and procedure

Branch: `testfw/803-testing-profile-stall`.
Measured revision: `4394409fca3932296f3468914b5be325ce0d48a6`, the fetched origin/main at
measurement start. `git merge-base --is-ancestor fbb1cd3b HEAD` exited 0;
this revision contains PR 967's merged CPU-local deferral work.
The issue bodies and comments for issues 803, 562, and 761 and
[the 891 round](../irq-locks/891-KSOFTIRQD-DEFERRAL-2026-09-08.md)
were read before measurement. Historical issue line numbers are not used
as current source citations below.

Command: `bash docker/qemu/run-aarch64-testing-profile-boot-test.sh 12`.
The script builds `--features testing --target aarch64-breenix-kernel.json`
with `-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
and runs four-CPU guests with its default 45-second window per boot.
The requested fork library was exported; aarch64 userspace ELFs and the
ext2 image were copied from the supplied checkout. TMPDIR and
BREENIX_GATE_TMP were scoped to this worktree. The shared QEMU lock
remained enabled, including waits between some boots. No other lane's
process was terminated. No structure-preflight skip was set.

[testing-12-gate.txt](serials/803/testing-12-gate.txt) records the measured
revision, build, no-NEON guard, 12 verdicts, and gate exit 1.
The build completed with 0 project-source warnings. Its pinned nightly
core future-incompatibility notice is the accepted toolchain notice under
the supplied issues 559/945 precedent; nothing was suppressed.
[inputs.json](serials/803/inputs.json) records kernel, disk, and ELF SHA256s.
[manifest.json](serials/803/manifest.json) binds each serial to its revision,
raw SHA256, line count, and milestone line numbers. The serials retain their
captured bytes; Git staging uses command-local `core.autocrlf=false`.
For V-2, `serials/803/boot-01.txt:4` contains a timestamp-based BUILD_ID,
not a Git revision; the same limitation applies to all 12 serials. Revision
attribution comes from the revision-bearing manifest, not serial text.
All 12 raw SHA256 values were rechecked against that manifest during landing.

## Classification rule

The shared missing-marker verdict is insufficient to identify issue 803.
Classification uses the serial milestones, with panic/lockup evidence
checked before assigning a no-panic category:

- **803 stall:** reaches the SMP-online marker from
  `kernel/src/main_aarch64.rs:1320`, then has no logged boot progress,
  no panic, and no lockup dump. Timer breadcrumbs and resume-PC census
  records alone are not boot progress. Reaching the deferral oracle or
  loader excludes this signature.
- **562-family:** softirq self-test/ksoftirqd panic. A lost or starved
  deferral oracle would be reported separately for investigation, not
  counted as a successful deferral or silently assigned to 803.
- **761 loader-marker red:** enters the loader but lacks its catalog and
  completion markers, without panic or lockup. This labels the measured
  marker boundary; it is not a new loader RCA.
- **Pass:** the committed profile gate succeeds with its required markers
  and no red reason. Any other signature requires separate inspection.

Current marker definitions are
`docker/qemu/run-aarch64-testing-profile-boot-test.sh:62` and
`docker/qemu/run-aarch64-testing-profile-boot-test.sh:63`; missing markers
are red at lines 138 and 139 of that file. The loader entry and return-side
completion print are `kernel/src/main_aarch64.rs:1503` and
`kernel/src/main_aarch64.rs:1505`; the catalog print is
`kernel/src/main_aarch64.rs:1723`.

## Per-boot tally

All rows ran `4394409fca3932296f3468914b5be325ce0d48a6` for 45 seconds.
The linked serial's line 173 contains the deferral oracle and line 185
contains loader entry in 12/12 captures. The catalog and completion counts
are 0 in 12/12 captures; panic and lockup counts are also 0 in 12/12.

| Boot / serial | Classification | Deferral | Wait ticks | Dispatches | Iterations | Catalog / completion |
| --- | --- | --- | ---: | ---: | ---: | --- |
| [01](serials/803/boot-01.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [02](serials/803/boot-02.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [03](serials/803/boot-03.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [04](serials/803/boot-04.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [05](serials/803/boot-05.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [06](serials/803/boot-06.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [07](serials/803/boot-07.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [08](serials/803/boot-08.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [09](serials/803/boot-09.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [10](serials/803/boot-10.txt) | 761 loader-marker red | ok | 4 | 1 | 41 | 0 / 0 |
| [11](serials/803/boot-11.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |
| [12](serials/803/boot-12.txt) | 761 loader-marker red | ok | 2 | 5 | 25 | 0 / 0 |

Boot 1 additionally records one binary loaded at `serials/803/boot-01.txt:207`.
That line does not satisfy the catalog or completion requirement.
[offline-scores.txt](serials/803/offline-scores.txt) records revision and
commands: the unchanged profile classifier exits 1 for the retained
population; `scripts/score-softirq-deferral.py` exits 0 separately for
12/12 serials. This replay validates deferral evidence without turning the
profile reds into passes.

## Source context and limits

[source-citations.txt](serials/803/source-citations.txt) re-derives source
lines using `git show` at the measured revision. The current sequence is
online-daemon initialization at `kernel/src/main_aarch64.rs:1329`, kthread
tests starting at `kernel/src/main_aarch64.rs:1345`, workqueue testing at
`kernel/src/main_aarch64.rs:1361`, and softirq testing at
`kernel/src/main_aarch64.rs:1363`.

`kernel/src/task/softirqd.rs:343` excludes the preemption-pinned boot CPU
from early daemon publication; `kernel/src/task/softirqd.rs:351` creates
the CPU-pinned daemon. `kernel/src/task/softirq_tests.rs:273` places the
aarch64 deferral probe on CPU1. Its success predicate is at
`kernel/src/task/softirq_tests.rs:233`, delivered-tick budget at
`kernel/src/task/softirq_tests.rs:239`, counter-clock backstop at
`kernel/src/task/softirq_tests.rs:243`, and oracle producer at
`kernel/src/task/softirq_tests.rs:297`.
These are current source facts, not evidence that PR 967 caused the
historical 803 symptom to disappear. With no matching stall in this sample,
there is no stalled guest on which to perform the conditional GDB RCA.

## Gates, mutation, and claim checks

- Testing-profile build and no-NEON guard: completed; 0 project diagnostics.
- Testing-profile live gate: exit 1, 12/12 loader-marker reds.
- Offline profile replay: exit 1, same 12/12 classifications.
- Deferral scorer replay: exit 0 for 12/12 serials.
- Mutation: not applicable; 803 did not reproduce and no fix was made.
- Structure suites, strict, production, and x86 gates: not run for this
  docs-only measurement. No boot_tests artifact was booted after the
  testing build; a future strict run must rebuild boot_tests first.

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/803-docs-message.txt -> exit 0

After the docs commit, source and serial citations are checked again with
`git show HEAD:<file>` against the retained manifest and source excerpts.
The measured revision remains the boot provenance; a docs commit is not
represented as a new boot population.

## Not claimed

- Issue 761 fixed, loader completion, or a passing testing-profile gate.
- Issue 803 fixed, a historical RCA, permanent absence, or a lower
  recurrence rate outside these twelve 45-second windows.
- Closure at the initial measurement: issue 803 remained open, so the R245
  closure objective was not delivered by that docs commit (V-1). Landing
  will close issue 803 only after the required landing gates are green and
  the 12 original boots plus the required landing population have no observed 803 stalls; a recurrence reopens it. Issues
  562 and 761 remain separate follow-up work.
- A new kernel fix, ratchet, mutation, or GDB stall capture.
- Production or x86 validation; initial measurement did not run strict or
  structure suites. Landing validation is recorded below.

## Landing

Deferred code findings: 0 (supplied list: []).

V-1 is resolved as a prose scope correction above, not a claim of issue
closure. V-2 is resolved by explicitly identifying manifest-based provenance;
raw serials are unchanged so their hashes and line citations remain valid.

claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/803-TESTING-STALL-2026-09-08.md docs/planning/green-program/aarch64-testing/serials/803/boot-*.txt -> exit 0
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/803-prose-message.txt -> exit 0

Fetch and integration: `git fetch origin` succeeded; `git merge --no-ff
--no-commit origin/main` reported already up to date. Origin/main was
`4394409fca3932296f3468914b5be325ce0d48a6`, already an ancestor, so Git
created no integration merge commit and encountered no conflicts.
Landing gate revision: `341dd25d8cc49a82346c2dfec428ebce2f2cb6e0`.
The branch has 0 changed kernel, docker, script, or test files relative to
origin/main; R182 fixture re-recording is not applicable because scorer
requirements did not grow. The conditional six testing-profile boots are
not applicable to this docs-only branch. No production or x86 gate was
requested in this landing population.

`bash scripts/run-structure-tests.sh` passed 69/69 suites, exit 0, recorded
in [landing/structure.txt](serials/803/landing/structure.txt).
The boot_tests soft-float kernel rebuild exited 0 with 0 project-source
diagnostics; the accepted pinned-core future-incompatibility notice is
retained in [landing/build.txt](serials/803/landing/build.txt).
After commit 341dd25d, source citations were re-derived with `git show HEAD`
and matched the retained source excerpts; 12/12 committed serial SHA256s
matched the manifest again.

`bash docker/qemu/run-aarch64-boot-test-strict.sh 1` exited 0: **1/1 GREEN**,
0/1 stalls, 0/1 inconclusive outcomes, 29 seconds. Its independent preflight
passed 69/69 suites. [landing/strict.txt](serials/803/landing/strict.txt)
records the revision and verdict; [landing/serial.txt](serials/803/landing/serial.txt)
is the raw serial, bound to the revision and kernel SHA256 by
[landing/inputs.json](serials/803/landing/inputs.json).
The strict serial reaches boot-test completion and clonevm exec PASS, which
exclude the pre-self-test issue 803 stall signature. This is a different
profile from the original 12 boots: it is not a thirteenth testing-profile
pass. Combined issue 803 stall tally is 0/13 across 12 testing-profile boots
and 1 strict landing boot at their respective recorded revisions.

The issue 803 closure condition supplied for this docs-only delivery is
satisfied by those observations. The requested issue comment and closure
will explicitly say a recurrence reopens it; historical RCA remains
unassigned. Issue 562's latest comment already distinguishes successful
deferral from missing loader markers and leaves its profile-pass condition
unmet; this measurement does not change that disposition, so no comment
on issue 562 is planned. Issue 761 remains open.

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/803-landing-message.txt -> exit 0

Not claimed at landing: a kernel repair, historical RCA, permanent absence
of issue 803, a testing-profile gate pass, loader completion in the original
12 boots, production validation, or x86 validation.
