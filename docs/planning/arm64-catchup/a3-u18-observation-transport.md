# A.3 observation transport repair

A.3 remains incomplete. This unit repairs the two observation defects in the void
measurement at `efa83364`; it does not identify the recovery or animation
failure mechanism. Coordinator line 208 remains the latest ruling.

## Retained capture

Vigil boot `20261111T213711Z-qemu-82836` is present in the boot store. Its raw
serial SHA-256 is
`41a8d9fd687a19f30e7b269a367f18fdd620e5662f1fee37b8491619074ec837`.
The run record says standalone window-manager, ARM64, display=false.
The cached summary stops at nine cases; the raw serial contains:

| Record | Observed result |
| --- | --- |
| recovery/stack-restored | FAIL, outer watchdog 10000 ms |
| recovery/supervisor-restarts | FAIL, outer watchdog 10000 ms |
| performance/latency-under-animation | FAIL, 20002 ms against 20000 ms |
| DONE | passed=78 failed=3 skipped=0 total=81 |
| MEMORY | reclaimed=PASS, 10 pages against allowance 48 |
| Standalone completion | READY window-manager after DONE and MEMORY |

Those watchdog records do not measure the inner recovery deadlines or explain
where their time went. The latency case exceeds its outer deadline by 2 ms;
its internal latency distribution is not established by that watchdog.

The serial has no `VALUE wm-case-budget` rows and has 14576 `disk-irq-entry`
rows. Timestamp-prefix counts in the QEMU trace are:

| Family | Timestamped / total |
| --- | --- |
| Read handler | 0/74336 |
| Read/write completion | 0/87459 |
| Request completion | 0/87716 |
| MMIO IRQ setting | 0/175895 |

## Source causes and forcing checks

`case_budget` in `libs/libbreenix/src/suite.rs` passed formatted arguments
directly to `File::write_fmt` via `writeln!`. The formatter emits separate
writes for the literal prefix, timestamp, separators, phase, label and newline.
`screen_evidence_write` in `kernel/src/fs/devfs/mod.rs` dispatches each write
separately: the prefix-only write reaches the command parser without a
numeric timestamp and is ignored; later fragments lack the command prefix.

Check-first commit `9ea01e64` adds one host check using the production writer,
the production per-write dispatch branch and the retained ring. Only the file
handle is substituted. Four phase calls generate 28 writes on the old source,
and the assertion fails because the ring has no case budget. Repair commit `2bd95040` builds
the command in memory and calls `write` once. The check then observes four
writes, retained begin/logic stamps and diagnostic duration, including the
unfinished logic interval a watchdog would leave. The evidence-disabled path
has no writes in this fixture. The existing window-manager-only activation in Suite::run
limits this producer's runtime reach to that suite on both architectures.

`scripts/qemu-arm64.py` enabled log-backend trace events without enabling their
timestamps. The installed QEMU help reports timestamps off by default. QEMU's
[tracing documentation, Log section](https://www.qemu.org/docs/master/devel/tracing.html#log)
specifies `-msg timestamp=on` for the thread/timestamp prefix. The branch-local
launcher check now requires this option and fails on the old launcher. Under
G2 this strengthens the existing observation check; no main expectation is
changed. The repair sets timestamp=on for serial-captured launches, preserving
other message settings and explicit log paths. It leaves accelerator, native
QoS launcher, pre-exec gate and renderer attestation selection intact.

These are observation transport causes. No completion timestamp can be recovered
from the old un-timed trace, and missing budget commands cannot be reconstructed
from serial fragments into authoritative kernel records.

## Validation and status

- Red: the two scoped checks fail on the old source for missing retained budgets
  and missing timestamp configuration; log retained locally.
- Green: `tests/a3_irq_observe_test.py`, 5 checks pass, including the new writer
  check and the existing gate refusal/order check.
- `tests/qemu_arm64_test.py`: 11 checks pass.

The normal ARM build-only path passed, including userspace compilation, ext2
staging and the production kernel/FP-SIMD check (0 forbidden instructions,
32 allowlisted). The affected x86 suite-window-manager release build passed.
Neither build emitted compiler warnings or errors; the ARM kernel command
reports the existing pinned-core future-compatibility advisory covered by
G35(2). The first build-only invocation was refused before compilation because
it omitted --gpu-gl; the corrected headless GL build-only invocation passed.

The ARM release suite, installed ELF and /sbin/suite-window-manager on the
staged ext2 image are byte-identical, with the required observation labels.
The kernel retains its expected observation labels and its prior hash. These
builds validated the source tree committed as `2bd95040`; no boot identity at
that commit is claimed. SHA-256 values:

| Artifact | SHA-256 |
| --- | --- |
| ARM suite (build/install/disk) | `74bb338c4763979acb29e898357ad28ccdab12c6e32c612b9eddf17209926b04` |
| ARM kernel | `29c77dffc3133b82aa349a8adb874df680d845bf1f60112517eaab4c8ce79175` |
| ARM staged disk | `25e441f8574a54e91c698aebbdcdc1523a1f5c57e073439a71075bdf3c5750bd` |
| x86 suite | `125c2e2a9a5f07302fdb817adaa29832228a708593512b4819443d6b982d1bf7` |

Lint records:

- `python3 scripts/claim-lint.py --files tests/a3_irq_observe_test.py`: exit 0.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-observation-check-msg.txt`: exit 0.
- The initial whole-file lint of suite.rs, launcher and report returned 24 findings:
  20 inherited comments in suite.rs and four in the new report. The report wording
  was corrected; inherited comments were left outside this repair.
- `python3 scripts/claim-lint.py --base 9ea01e64`: exit 0 on changed hunks.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-observation-fix-msg.txt`: exit 0.
- Final report, handoff commit message and PR body are checked before publication.
- `git diff --check`: exit 0 before the implementation commit.

No new guest boot, R240 deployment, acceptance verdict, board update or merge
is claimed.
The coordinator-208 measurement allowance was consumed by the void capture;
this workflow retry does not override its stop. The ledger asks for one renewed
instrumented measurement under the artifact/load gates, with complete budgets
and timed host completion/IRQ events. That measurement would answer which side
holds the long disk requests and where each outer-case budget is spent. A causal
forcing check and runtime fix must follow that evidence before subsequent acceptance boots.
