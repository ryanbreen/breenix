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
- Closure of issues 803, 562, or 761. They remain open for follow-up;
  the existing issues track the remaining work without a duplicate issue.
- A new kernel fix, ratchet, mutation, or GDB stall capture.
- Strict, production, x86, or structure-suite validation in this round.
