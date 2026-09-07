# 928 service-sequence capture window — 2026-09-07

Scope: host capture budget and a structure ratchet. No kernel source changes.
Base revision: 62924b4739870b344a78cd16cf4099c774ea48e9.
Branch: gates/ss-capture-window-928.

## Bisect evidence

The supplied ATTRIBUTION.md was read first, then each per-SHA gate log and its original serial captures. Its advertised copied serial directories were empty. The original captures identified by each log's Output field have been preserved here under `serials/928-ss-window/bisect/<sha>/`, with the revision prepended to each gate transcript. These are historical supplied runs, not new boots in this lane.

| Revision | max GREEN | cortex-a72 GREEN | Combined GREEN |
| --- | --- | --- | --- |
| 72ad554d | 2/2 | 2/2 | 4/4 |
| fbe82171 | 0/2 | 0/2 | 0/4 |
| 4bec5d74 | 0/2 | 0/2 | 0/4 |
| 1a102dd9 | 0/2 | 0/2 | 0/4 |
| 19427ef4 | 0/2 | first boot red; second serial empty | incomplete; 0/3 observed boots |

For the first four rows, the preserved `bisect/<sha>/gate.log` contains both per-profile and combined GREEN rates. The fifth log ends before a cortex-a72 summary; an empty capture is not a completed boot. The user-reported main-health reading at 19427ef4 is service-sequence 0/50 versus strict 20/20; that larger battery is context, not independently rerun evidence here.

## Mechanism and correction

The first-red revision fbe82171 introduced the C4 worker isolation fixture. At this branch's source revision, `kernel/src/tracing/providers/teardown.rs:8260` defines `exit_kick_worker_window_isolation_test`; lines 8267 and 8269 set its 8000 ms first-progress window and 15000 ms absolute ceiling. Its union control and three frozen-worker scenarios deliberately consume approximately 39 seconds before ordinary services start. The C5 note at `kernel/src/tracing/providers/teardown.rs:8985` identifies this deep consumer of the shared test-phase budget. `kernel/src/test_framework/mod.rs:105` sets that budget to 60000 ms.

The preserved first-red `serials/928-ss-window/bisect/fbe82171/max/serial-1.txt:777` completes the kernel test tally at 117/117, but the 45-second capture ends before the futex verdict and poll-TCP verdict. Later revisions also include C5 fixture time and can end inside the test phase. The defect corrected here is the host capture horizon; completion of kernel tests alone is insufficient evidence for service-sequence success.

| File:line | Before | After |
| --- | --- | --- |
| docker/qemu/run-aarch64-boot-test-strict.sh:933 | `timeout "${BREENIX_STRICT_TIMEOUT_SECONDS:-90}" qemu-system-aarch64` | unchanged, 90 seconds |
| docker/qemu/run-aarch64-service-sequence-gate.sh:20 | `BOOT_TIMEOUT=45` | `BOOT_TIMEOUT=90` |

`docker/qemu/run-aarch64-service-sequence-gate.sh:1167` and line 1213 enforce this budget. Its header now accounts for timed boot fixtures. The script still accepts an explicit `--timeout` override. A recursive search under `docker/qemu/` and `scripts/` found no wrapper or battery invocation pinning 45 seconds for this gate; wrappers fixed: 0. The other scripts with 45-second defaults are separate gates, not callers of this service-sequence script.

## Ratchet and mutation

`tests/gate_structure_preflight_wiring_structure.rs:140` parses the active strict timeout expansion and the service gate's numeric top-level assignment, rejecting missing, duplicate, or nonnumeric defaults. The source test requires service-sequence >= strict. A second test mutates the parsed service value to 45 in memory and requires that ordering to fail.

An actual on-disk mutation changed the service default from 90 to 45 and ran `bash scripts/run-structure-tests.sh gate_structure_preflight_wiring_structure service_sequence_capture_window_covers_strict`: exit 101, 0 passed / 1 failed. The failure identifies 45s as shorter than strict 90s (`serials/928-ss-window/ratchet-red.log`). The mutation was reverted; the entire gate-tooling suite passed 6/6 (`ratchet-restored.log`). The requested bare `bash scripts/run-structure-tests.sh` passed 103/103 (`structure.log`). Each transcript identifies the base revision plus the uncommitted tooling changes used in these initial checks.

## Follow-up

Issue 947: https://github.com/ryanbreen/breenix/issues/947

The exact issue title is recorded on GitHub as requested. It asks for the approximately 39-second per-boot fixture cost to fall below 10 seconds. At 50 boots, 50 × 39 = 1950 seconds = 32.5 minutes (about 32 minutes) per reading. The C5 budget note is `docs/planning/green-program/tracing/522-C5-2026-09-07.md:455–457`. The scaled-budget precedent is `kernel/src/tracing/providers/teardown.rs:9167–9195`, whose fixture preserves deadline ordering with smaller constants.

## Claim lint

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/code-commit.txt -> exit 0

These initial checks cover the scripts/ratchet change. The first draft lint returned exit 1 for an unquantified wrapper statement; changing it to the measured count made the next tree lint exit 0. Final checks and gate results are recorded below after execution.

## Not claimed

- The approximately 39-second isolation fixture cost itself is not fixed here.
- No kernel behavior, production timeout, oracle predicate, or failure bucket was changed.
- This is not a fresh 50-boot battery or a population reliability estimate.
- The incomplete fifth bisect run is not represented as a completed four-boot sample.

## Build environment

The userspace ELFs and ext2 image were copied from `/Users/wrb/fun/code/breenix` as requested. TMPDIR and BREENIX_GATE_TMP point to this worktree's `.tmp` and `.gate-tmp`. BREENIX_RUST_FORK_LIBRARY remains `/Users/wrb/fun/code/breenix-parallels/rust-fork/library`.

Both the installed nightly library and the supplied fork initially built successfully but emitted Cargo's soft-float/NEON future-compatibility summary for core (`build.log`, `build-fork.log`). To meet the zero-warning requirement without suppressing a diagnostic or modifying shared files, a private copy of the fork library and its `libs/libc` path dependency was made under `.tmp`. The build-only patch in `serials/928-ss-window/build-std-softfloat.patch` excludes NEON intrinsic modules, reexports, and their portable-SIMD conversion/import consumers on aarch64 softfloat; it leaves other ABIs intact. The final kernel build uses `__CARGO_TESTS_ONLY_SRC_ROOT=$PWD/.tmp/rust-fork/library`. This changes the build's standard-library source input, not repository kernel source. The first local-copy attempt needed the fork's relative libc dependency copied too; that path error was corrected. An intermediate build then identified portable-SIMD consumers of those unavailable intrinsic types; those consumers received matching ABI guards before the final clean build.

No acceptance of the existing warning was assumed. The lane-local source correction resolves it instead. Shared toolchain and fork files remain untouched.

The final lane-local build returned exit 0 with zero warnings/errors (`serials/928-ss-window/build-local.log`). The code commit is 1b09a97d66319a3aff6dede99ea9af5f5641b115. Its fresh build is `head-build.log`, also exit 0 with zero warnings/errors. `inputs.sha256` records the kernel, disk, and copied ELFs. The full host sweep passed 64/64 structure suites, 993/993 tests (`all-structure.log`); the required bare command and gate-tooling suite were also rerun at the code commit (`head-structure.log`, `head-gate-tooling.log`).

The first commit unexpectedly invoked the preconfigured shared Beads post-commit hook, which reported exporting issues to `/Users/wrb/fun/code/breenix/.beads/issues.jsonl`. No other worktree was manually modified. Subsequent commits disable that hook per command with `git -c core.hooksPath=/dev/null commit`; GitHub issue 947 is the work-tracking record for this task.

To reproduce the lane-local library preparation from the supplied fork, run from this worktree (the patch is only applied to the private copy):

```bash
mkdir -p .tmp/rust-fork .tmp/libs
cp -R /Users/wrb/fun/code/breenix-parallels/rust-fork/library .tmp/rust-fork/library
cp -R /Users/wrb/fun/code/breenix-parallels/libs/libc .tmp/libs/libc
patch -d .tmp/rust-fork/library -p1 < docs/planning/green-program/gates/serials/928-ss-window/build-std-softfloat.patch
export __CARGO_TESTS_ONLY_SRC_ROOT="$PWD/.tmp/rust-fork/library"
```

Then use the build command recorded in `serials/928-ss-window/head-build.log`. This preparation assumes the destination copy does not already exist.

## Service-sequence results at the code HEAD

Revision 1b09a97d66319a3aff6dede99ea9af5f5641b115 ran `bash docker/qemu/run-aarch64-service-sequence-gate.sh --boots 2` with the default profile selection, both. The command exited 0. The capture default was 90 seconds; no timeout override or structure-skip environment variable was used. The host lock serialized this lane with the other battery; lock waiting occurred before each boot's capture clock.

| Profile | Boot 1 | Boot 2 | GREEN |
| --- | --- | --- | --- |
| max | GREEN at 69 s | GREEN at 69 s | 2/2 |
| cortex-a72 | GREEN at 69 s | GREEN at 69 s | 2/2 |
| Combined | | | 4/4 |

`serials/928-ss-window/service-sequence.log` records the complete scorer output and revision. `serials/928-ss-window/oracle-census.txt` names the file and line for the full futex-handoff verdict, poll-TCP PASS, BLOCK_EINTR PASS, poll-timeout report, armed context oracle, census-widen PASS, TTY completion, quiesce refusal/walk and boot-script completion in each of 4/4 boots, alongside the 118/118 test tally. The kernel tally is 118/118 at this later code HEAD; the first-red bisect tally was 117/117. The unchanged gate additionally enforces its existing negative-marker and census predicates. Both per-profile census.tsv files, QMP verdicts and four raw serials are preserved next to the transcript. `SHA256SUMS` covers the evidence files; local attributes preserve raw console bytes.

These four passing captures discriminate the capture-window correction at this code revision. They do not establish a 50-boot rate or a particular reliability bound. The following documentation commit changes only documentation/evidence; the tested scripts, ratchet and kernel sources remain the code HEAD above.

## Final claim lint and handoff

Initial documentation draft (wrapper count phrasing corrected afterward):

claim-lint: python3 scripts/claim-lint.py -> exit 1

Before the documentation commit:

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/docs-commit.txt -> exit 0

Claim lint validates prose shape, not oracle execution; the gate and serial evidence supply the execution results. Files with evidence-only extensions outside claim-lint's supported set are reported as skipped, not represented as linted source. Kernel source diff against the base is empty. The gate's PID-scoped cleanup terminated its four QEMU children; a final process inspection found no QEMU using this lane's kernel path. Other lanes' processes were left running.

Additional not-claimed: the standard-library build-only patch is not an upstream toolchain fix or a shared-toolchain modification.


## Landing

The supplied PROSE findings list was empty (0 findings). The round document was checked explicitly:

claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/gates/928-SS-CAPTURE-WINDOW-2026-09-07.md -> exit 0

After `git fetch origin`, `git merge --no-commit --no-ff origin/main` found origin/main 62924b4739870b344a78cd16cf4099c774ea48e9 already contained in HEAD. There was no merge conflict or new local merge commit. The integrated tip tested was c98d7cc77e2298f46fbc796d3d74cd7575a18d1e. The PR is to use a merge commit on GitHub.

The userspace ELFs and ext2 image were copied again from the requested main worktree. The fresh soft-float build used the previously documented lane-local library patch and returned exit 0 without compiler warnings or errors (`serials/928-ss-window/landing-build.log`). Input hashes are in `serials/928-ss-window/landing-inputs.sha256`.

| Command | Result at c98d7cc77e2298f46fbc796d3d74cd7575a18d1e | Transcript |
| --- | --- | --- |
| `bash scripts/run-structure-tests.sh` | exit 0; 103/103 tests | `serials/928-ss-window/landing-structure.log` |
| `bash docker/qemu/run-aarch64-boot-test-strict.sh 1` | exit 0; 1/1 boots; fixed cortex-a72 profile; preflight 64/64 suites, 993/993 tests | `serials/928-ss-window/landing-strict1.log` |
| `bash docker/qemu/run-aarch64-service-sequence-gate.sh --boots 2` | exit 0; max 2/2 GREEN, cortex-a72 2/2 GREEN; combined 4/4 GREEN | `serials/928-ss-window/landing-serviceSeq2x2.log` |

The four service-sequence captures reached GREEN at 69 seconds each. The default 90-second capture windows were used. No structure-skip variable was set. Raw strict and service-sequence captures plus preflight transcripts are in `serials/928-ss-window/landing-captures/`; `serials/928-ss-window/landing-SHA256SUMS` records their hashes and the landing transcript hashes. Gate transcripts identify the revision they ran at. The following landing commit adds documentation and evidence; it does not change tested code.

Landing commit and planned GitHub merge message checks:

claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/landing-commit.txt -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/merge-commit.txt -> exit 0

Not claimed: a fresh 50-boot battery, a population reliability bound, a kernel fix, reduced fixture runtime, an upstream standard-library fix, or strict-gate coverage of max (the strict script fixes cortex-a72). Fixture-runtime follow-up remains issue 947. The branch's kernel commit log relative to origin/main is empty.

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --files .tmp/pr-body.md -> exit 0
