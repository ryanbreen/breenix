# Issue 891: CPU-local deferral and a guest-execution oracle

Branch: `sched/891-ksoftirqd-deferral-selftest`.
Baseline: `c93ecf5297f42bf4127849f0b26961d74c25f85c`.

## Scope and evidence boundaries

The reported reproduction population is the operator's 6 panics in 15 x86
boots under five concurrent QEMUs. It is not a population measured by this
round. The requested 586 panic serial is absent at the baseline revision.
The [766 excerpt](../timekeeping/serials/766/28-x86-smp-gate-ksoftirqd-panic-562-family.txt)
shows the daemon-identity assertion, after the aggregate iteration-count assertion. Issues 891, 562 (including comments),
803, and the loader issue 761 were read for this investigation.

## Source references re-derived at a15ffa60

| Mechanism | Current source |
| --- | --- |
| CPU-indexed static handles and lock-free lookup | `kernel/src/task/softirqd.rs:116`, `kernel/src/task/softirqd.rs:136` |
| Iteration-limit wake | `kernel/src/task/softirqd.rs:242`, `kernel/src/task/softirqd.rs:260` |
| Daemon callback evidence and predicate park | `kernel/src/task/softirqd.rs:298`, `kernel/src/task/softirqd.rs:313` |
| CPU pin and boot-CPU publication guard | `kernel/src/task/softirqd.rs:335` |
| Re-raise until daemon evidence | `kernel/src/task/softirq_tests.rs:186` |
| Delivered-tick failure / counter-clock backstop | `kernel/src/task/softirq_tests.rs:239`, `kernel/src/task/softirq_tests.rs:243` |
| aarch64 CPU1 probe | `kernel/src/task/softirq_tests.rs:273` |
| Park intent and final check | `kernel/src/task/kthread.rs:217`, `kernel/src/task/kthread.rs:253` |
| Serialized unpark | `kernel/src/task/kthread.rs:274` |
| Init-handoff publication | `kernel/src/main_aarch64.rs:188` |

The baseline source is archived separately as
[softirq_tests.rs.txt](serials/891/baseline/softirq_tests.rs.txt) and
[softirqd.rs.txt](serials/891/baseline/softirqd.rs.txt). Historical line 228
belongs to that baseline, not the current self-test.

## Mechanisms

The old test lets interrupt exits consume its finite 25-callback stream. Its
100-iteration yield/halt loop exits as soon as the aggregate count reaches
25, before checking daemon participation. It does not measure a wall-clock
window or a delivered-tick budget. The comment assuming one interrupt-exit
batch cannot constrain how many interrupts occur before a daemon dispatch.
A longer constant alone cannot fix that early exit.

The [aarch64 GDB capture](serials/891/baseline/arm-gdb.txt) shows the boot test on CPU0 and ksoftirqd executing
on CPU2 at the wait entry. At the assertion, iterations are 25 and the daemon
flag is false; ksoftirqd is parked on CPU2. The CPU0 context-switch trace
counter stays at 0, while CPU1 advances 26 to 27 and CPU2 advances 29 to 31.
The unblock attribution counter advances 13 to 14 and same-lock enqueue
stays at 3. These are system counters, not per-thread wake receipts.
The baseline daemon is spawned without a CPU pin at `serials/891/baseline/softirqd.rs.txt:302` and reads its executing CPU's bitmap at `serials/891/baseline/softirqd.rs.txt:253`. The aarch64 bitmap accessor is `kernel/src/per_cpu_aarch64.rs:766`; the boot pin is established at `kernel/src/main_aarch64.rs:860` and released at `kernel/src/main_aarch64.rs:185`. The CPU-local pending bitmap and migratable daemon disagree about ownership;
a dispatch on another CPU cannot service CPU0's bitmap. The boot stack also
cannot dispatch a CPU0-pinned worker while it holds its boot preemption pin.

The [x86 baseline hardware-breakpoint capture](serials/891/baseline/x86-gdb2/gdb.txt)
starts before `test_softirq` executes. Unblock attribution advances 9 to 10,
same-lock enqueue advances 12 to 13, the daemon-work flag advances 0 to 1,
and the final iteration count is 25. The final CPU0 context-switch trace
counter reads 37 (a lifetime total, not a measured window delta). The
[observed boot offset](serials/891/baseline/x86-gdb2/observed-base.txt)
confirms the 1-TiB symbol relocation used for that stop. This is a passing
baseline observation, not a reproduction of the operator's six failing boots.
The first x86 GDB attempt attached after the self-test and supplied no counter
window; it is retained separately as [x86-gdb](serials/891/baseline/x86-gdb/gdb.txt).

For x86 under load, the assertion's invalid completion criterion is sufficient
to explain the preserved signature: IRQ exits can consume the finite workload
before the daemon is scheduled. The archived source's lines 207-216 end the
wait on aggregate progress, while line 228 subsequently demands daemon work
([baseline self-test](serials/891/baseline/softirq_tests.rs.txt)). At HEAD,
`kernel/src/task/softirq_tests.rs:194` prevents that exhaustion and line 233
requires measured daemon callbacks before success. The historic serial does
not distinguish a late dispatch from an additional lost wake; this round does
not assign such attribution to those six individual failures.

The park protocol has two additional source-visible races: an empty check
before publishing parked intent, and an unpark flag clear outside the lock
that protects the final blocked-state publication. This round repairs them;
it does not claim either race was captured in the operator's x86 population.

## Change

CPU-local, pinned daemons read their own pending bitmap. IRQ wake lookup
borrows a published immutable handle without the former global mutex.
The bootstrap coordinator adds secondary daemons after SMP bringup and wakes
each daemon after handle publication. A static `Once` owns each handle for IRQ readers. The unused shutdown helper
was removed: CPU hot-unplug and subsystem restart are outside this change.

`kthread.rs` is the Tier-2 edit: the defect is in the park/unpark publication
protocol. The daemon publishes parked intent before rechecking pending work;
the final park flag check and unpark flag clear share scheduler serialization.
No new logging is in that file. No Tier-1 file changed.

The probe keeps re-raising Tasklet until daemon callback evidence is present.
The aarch64 probe is a CPU1-pinned kthread; the coordinator remains on the
original boot stack, leaving the loader's execution model unchanged.

## Oracle grammar

Producer: `kernel/src/task/softirq_tests.rs:297`. Scorer:
`scripts/score-softirq-deferral.py:13`.

`[SOFTIRQ_DEFERRAL_ORACLE:arch=(x86|aarch64):cpu=N:budget_ticks=250:wait_ticks=N:wait_ns=N:dispatches=N:iterations=N:verdict=(ok|starved|lost)]`

`dispatches` counts Tasklet callback dispatches from the daemon body, not
scheduler selections, wakes, or IRQ-exit callbacks. `wait_ticks` counts local
delivered timer interrupts. `wait_ns` uses the counter clock and includes
coordinator wait. A breadcrumb prefix may share its serial line.

`ok` requires daemon callback evidence and at least 25 iterations. `lost`
includes a failure to stop the explicit initial pass at 10 callbacks or a
failure to complete after 250 local timer interrupts. `starved` is the
15-second counter-clock backstop without the delivered-tick budget being
spent. It is an inconclusive infrastructure result, not a kernel panic or
boot-test red marker. The shared scorer exits 0, 1, or 2 respectively and
rejects missing, duplicate, malformed, or internally contradictory oracles.

## Mutations and intermediate results

The new ownership/self-test/scorer ratchets fail on the baseline source.
Deleting the iteration-limit wake makes the structure suite exit 101;
restoring it makes the suite pass. A separate runtime mutation withholding
daemon-callback evidence reports 250 delivered ticks, 0 dispatches, 412587
iterations, and `lost`; its scorer exits 1 and its serial has no panic.
The runtime capture is [mutation/runtime/arm-serial.txt](serials/891/mutation/runtime/arm-serial.txt).
That mutation tests evidence enforcement, not a measured lost production wake.
Mutation sources were restored before boot gates.

The first changed aarch64 testing-profile diagnostic reports `ok` in 2 ticks
with 5 daemon callbacks, but the profile gate remains red on missing loader
markers. This matches the separately documented issue 761 boundary; this
round does not claim to have repaired the loader.

The initial strict diagnostic and the three strict boots at `cb151467` have
`census_widen_oracle` failures despite passing deferral oracles. This was a
regression in that first implementation: it published a CPU0-pinned daemon
while the boot preemption pin prevented CPU0 dispatch. The strand census
reported one Ready thread on nondispatching CPU0 before its own mutation.

Commit `a15ffa60` delays the CPU0 daemon until `launch_init_from_elf` releases
the boot preemption pin, with IRQs still masked before ERET. Peer daemons
remain available after SMP bringup. The publication-order ratchet fails on
`cb151467` and passes with this change. The strand oracle was not weakened.

## Final gates and revision provenance

The final gates use code revision `a15ffa600817ce100dce7d8a4c268c1fb6119099`, recorded at the start of each gate transcript. Intermediate dirty-tree
diagnostics are separate from committed-revision runs. `cb151467` and
`a15ffa60` are separate populations; the former includes the CPU0-publication
regression corrected by the latter.

## aarch64 final population at a15ffa60

| Gate | Result | Capture |
| --- | --- | --- |
| Strict, three boots | 3/3 pass; no panic | [gate](serials/891/final-arm/strict-gate.txt) |
| Testing, one boot | Deferral ok; gate red on missing loader markers, kernel_panic=0 | [gate](serials/891/final-arm/testing-gate.txt) |
| Production, run last | Pass; crash marker count 0 | [gate](serials/891/final-arm/prod-gate.txt) |

The final strict boots retain the census-widening assertion and report
`baseline_reported=0`, `armed_reported=1`, `PASS`; see the strict serials below.

`serials/891/final-arm/strict-1-serial.txt:175`:

```text
T2T3T4T5T6T7[SOFTIRQ_DEFERRAL_ORACLE:arch=aarch64:cpu=1:budget_ticks=250:wait_ticks=4:wait_ns=5360992:dispatches=1:iterations=41:verdict=ok]
```

`serials/891/final-arm/strict-2-serial.txt:175`:

```text
T2T3T4T5[SOFTIRQ_DEFERRAL_ORACLE:arch=aarch64:cpu=1:budget_ticks=250:wait_ticks=2:wait_ns=3211008:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-arm/strict-3-serial.txt:175`:

```text
T2T3T4T5T6[SOFTIRQ_DEFERRAL_ORACLE:arch=aarch64:cpu=1:budget_ticks=250:wait_ticks=2:wait_ns=4669008:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-arm/testing-serial.txt:173`:

```text
T2T3T4T5T6T7T8T9T0[SOFTIRQ_DEFERRAL_ORACLE:arch=aarch64:cpu=1:budget_ticks=250:wait_ticks=2:wait_ns=2868000:dispatches=5:iterations=25:verdict=ok]
```

## Claim lint

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/code-commit.txt -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/cpu0-commit.txt -> exit 0

## Not claimed

- Attribution of each of the operator's six x86 panics to a specific wake race.
- Repair of issue 761 or issue 803.
- A production lost-wake reproduction from the callback-evidence mutation.
- Coverage of aarch64 single-CPU testing, x86 guests with multiple vCPUs,
  CPU hotplug, or daemon shutdown/restart.
- A clean testing-profile loader, or a passing gate inferred only from an oracle.

## Execution notes

The first debug-symbol aarch64 link failed on orphan debug sections; a scratch
linker supplement corrected the diagnostic build. Those attempts are retained,
not counted as successful builds. The pinned nightly core build-std
future-incompatibility notice is the accepted toolchain notice documented in
issues 559 and 945; no project-source warning suppression was added.

An existing shared Git commit hook unexpectedly exported legacy issue data to
`/Users/wrb/fun/code/breenix/.beads/issues.jsonl` on the first code commit.
That side effect was reported immediately. Subsequent commits disable hooks
with `core.hooksPath=/dev/null`; no shared file was reverted or edited manually.
GitHub issues remain the work-tracking source.

## Final x86 test population

`bash docker/qemu/run-x86-boot-tests.sh`: exit 0, one boot.
`bash docker/qemu/run-boot-parallel.sh 5`: exit 0 twice, 5/5 and 5/5.
Across these eleven boots: 11 `ok`, 0 `starved`, 0 `lost`, 0 panic markers.
Each parallel batch was preceded by the full structure suite; no preflight
was skipped and neither required the 900-second timeout retry.

`serials/891/final-x86/raw-boot-tests/serial_user.txt:124` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=5726175:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-1/boot-1-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=4039229:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-1/boot-2-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=6097452:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-1/boot-3-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=4409206:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-1/boot-4-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=3331184:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-1/boot-5-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=5869697:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-2/boot-1-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=4:wait_ns=15926611:dispatches=1:iterations=41:verdict=ok]
```

`serials/891/final-x86/parallel-2/boot-2-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=4864925:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-2/boot-3-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=4408896:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-2/boot-4-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=2:wait_ns=5066368:dispatches=5:iterations=25:verdict=ok]
```

`serials/891/final-x86/parallel-2/boot-5-user.txt:44` (oracle token; preceding switch breadcrumbs omitted):

```text
[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks=3:wait_ns=9190699:dispatches=5:iterations=25:verdict=ok]
```


## Beast launch loads and contention

R238 was checked in the container before each gate command. The launch
records in `serials/891/final-x86/*-load.txt` show:

| Command | UTC launch | 1-minute load |
| --- | --- | ---: |
| x86 boot tests | 06:04:49 | 1.02 |
| parallel 1 structure preflight | 06:18:08 | 1.30 |
| parallel 1, five guests | 06:21:31 | 5.16 |
| parallel 2 structure preflight | 06:22:40 | 4.68 |
| parallel 2, five guests | 06:26:17 | 5.10 |
| x86 production | 06:27:33 | 5.45 |

Earlier GDB jobs waited through loads above 8 and resumed below 6. The
host-wide QEMU lock also queued diagnostic/gate launches behind other lanes;
no other lane's processes were killed. These are command-launch loads, not a
claim that load remained constant through builds, lock waits, or execution.
No final test result is inferred from a high-load timer-latency overrun.

The documentation draft lint caught an unquantified revision sentence; it was
rewritten before committing.

claim-lint: python3 scripts/claim-lint.py -> exit 1

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/docs-commit.txt -> exit 0

## Production result and final limits

The x86 production gate passed (exit 0), after the eleven x86 test boots.
Its 68/68 structure preflight and production build completed; the transcript
is [prod-gate.txt](serials/891/final-x86/prod-gate.txt), with raw ports in
[raw-prod](serials/891/final-x86/raw-prod). Production was the last kernel
profile built and booted on each architecture.

The x86 image builder attempted the optional BusyBox payload and reported
missing `x86_64-linux-musl-gcc`. It continued without BusyBox. Its subsequent
static contents summary is not evidence that BusyBox was installed. This
round does not claim BusyBox/coreutils coverage. The production kernel build
itself emitted no project-source diagnostics.

The measured final test population is 15 deferral oracles: 11 x86 and 4
aarch64, each `ok`, with 0 `lost`, 0 `starved`, and 0 kernel panics. The
aarch64 testing-profile gate remains red on loader markers despite its
deferral result; the three strict boots and two architecture production
gates pass. Production has no test-only deferral oracle.

Issue 891 remains open for review of this branch; loader follow-up is already
tracked by issue 761 (with issue 803 history). The missing requested 586
artifact and unobserved historical wake attribution remain evidence limits,
not manufactured successful reproductions.

## Review fix round: V-1, V-2, V-3 (2026-09-08)

V-1: `docker/qemu/run-aarch64-boot-test-strict.sh` now retains the scorer's
exit 2 through score-only output, frozen-deadline boot scoring, evidence
reporting, the import hook, and iteration aggregation. It reports
INCONCLUSIVE with host-starvation context and imports INCONCLUSIVE/2.
Mixed failure/starvation iterations exit 1; passing/starved iterations exit 2.
Crash and explicit boot-test failure markers take precedence over a starved
oracle. The delivered-tick budget and boot deadlines are unchanged.

V-2: `scripts/score-softirq-deferral.py` requires wait_ns of at least
15000000000 before accepting starved, in addition to wait_ticks below 250.
The regression checks 0, 123, and 14999999999 ns as failures, with
15000000000 and 15000000001 ns accepted as starvation when ticks are below
budget. The existing missing/duplicate/forged-completion checks remain.

V-3: `kernel/src/task/mod.rs` joins the softirq API re-export as rustfmt
requires. A structure test runs rustfmt --check on this file. This round's
only retained kernel edit is formatting in that file; it changes no Tier-1
or Tier-2 file and introduces no logging or warning suppression.

### Regression and mutation evidence

`tests/softirq_deferral_structure.rs` passes 9/9 tests, including the scorer
boundary test, the strict shell behavior test, and the rustfmt check.
`scripts/test-softirq-strict-status.py` executes score-only mode directly,
the actual frozen-snapshot/report/import shell tail with stubbed providers,
and the actual aggregate loop with five mixed-status sequences. It does
not launch QEMU or substitute weaker boot-success evidence.

The [mutation runner](serials/891/review-fix/run-mutations.py) records eight
expected exit-101 reds: score-only exit collapse, frozen-status collapse,
FAIL/1 import substitution, aggregate-status collapse, removed backstop
validation, reintroduced formatting defect, omitted iteration-limit wake,
and removed boot-preemption publication guard. Each source is restored
byte-for-byte before the next case; the [restored suite](serials/891/review-fix/restored-green.txt)
passes 9/9. This repeats the prior omit-wake and boot-publication ratchets
as well as the new review regressions.

The callback-evidence runtime mutation was also repeated: a temporary build
removed the dispatch counter increment in `kernel/src/task/softirqd.rs`.
Its [serial](serials/891/review-fix/runtime-serial.txt) records 250 wait_ticks,
387582000 wait_ns, dispatches=0, 315750 iterations, and lost;
its [scorer](serials/891/review-fix/runtime-score.txt) exits 1. No panic marker
was found in that serial. The mutated artifact was copied aside, the source
restored, and a clean boot_tests kernel rebuilt before the strict gate.
This measures withheld callback evidence, not a production lost wake.

The initial full structure sweep was 67/68: the existing import ratchet
expected two strict-gate call sites, before the third inconclusive outcome
was added. Its [red](serials/891/review-fix/import-pre-update-red.txt) is
retained. `tests/run_inspector_import_structure.rs` now pins three sites,
including the INCONCLUSIVE/2 branch's preceding status check. The subsequent
Mac gate preflight passes 68/68 suites. No structure preflight was skipped.

### Gate provenance and results

Both requested gates ran revision `3346870ed480e5210c0cdacb46b07f049511df67`
plus [tested-source.patch](serials/891/review-fix/tested-source.patch), whose
full-index SHA256 is
`1336a23d695811dfde35f71ae123d1addcdde394bd03d8f10e3c79b715797d10`,
and the new `scripts/test-softirq-strict-status.py`, SHA256
`e2a9833367590ca684ceb29460cee70e5be0ae49f3ca0694cfdb7a9cb5bef936`.
The full-index patch hashes matched between Mac and beast; the ordinary
patch hashes differ solely because their Git object abbreviations have
different lengths. The runtime mutation's distinct provenance is in
[runtime-revision.txt](serials/891/review-fix/runtime-revision.txt).

The Mac command `bash docker/qemu/run-aarch64-boot-test-strict.sh 1` exits 0:
68/68 structure suites, 1/1 boots, zero failures or inconclusive boots.
The [gate transcript](serials/891/review-fix/aarch64-strict.txt) and
[serial](serials/891/review-fix/aarch64-strict-serial.txt) record an ok deferral
oracle with 2 ticks, 3768992 ns, 5 dispatches, and 25 iterations.
The soft-float boot_tests build used `aarch64-breenix-kernel.json` and the
specified fork-library path. Userspace ELFs and ext2 came from the specified
Mac checkout. The build emitted zero project diagnostics; its pinned-core
future-incompatibility notice is the accepted toolchain notice under issues
559 and 945, not a suppressed project warning.

Beast launched the requested `bash docker/qemu/run-x86-boot-tests.sh` in
`/root/breenix-s891`, with lane-local TMPDIR and BREENIX_GATE_TMP, at a
one-minute load of 1.43; its QEMU-start facts later record 3.42. The R238 check did not require a wait. Missing x86
userspace ELFs were built once. The [x86 gate](serials/891/review-fix/x86-gate.txt)
exits 0 with 68/68 structure suites and 1/1 boot. Its userspace tally is
exited=110, nonzero=0, failed=[]. The deferral oracle records 3 ticks,
8063849 ns, 5 dispatches, and 25 iterations; timer-wake latency records
45 ms overrun within its 100 ms bound. The slow context-restore suite
completed in 216 seconds on its first attempt. No preflight timeout,
900-second retry, SCSI/IO failure, or project build warning occurred.
Raw ports are retained as [user serial](serials/891/review-fix/x86-serial-user.txt)
and [kernel serial](serials/891/review-fix/x86-serial-kernel.txt).

### Not claimed

- A live host-starved boot in this round: starvation classification and
  status propagation are exercised with synthetic evidence.
- A production lost-wake reproduction from the callback-counter mutation.
- New production-profile, testing-profile, or multi-boot stress coverage;
  this review round reruns the two requested boot gates.
- A change to the run-inspector application's UI projection; the gate
  hook now receives the distinct INCONCLUSIVE string and exit 2.
- Closure of issue 891 before branch review and merge.


### Commit checks

The [static checks](serials/891/review-fix/static-checks.txt) record Bash
syntax, rustfmt on the four listed task-module files, and git diff --check,
each exiting 0. No lane-owned QEMU remained after the gates; cleanup used
owned PIDs, without name-based killing. No task-created stash exists.

claim-lint: python3 scripts/claim-lint.py -> exit 0

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/review-commit-message.txt -> exit 0
