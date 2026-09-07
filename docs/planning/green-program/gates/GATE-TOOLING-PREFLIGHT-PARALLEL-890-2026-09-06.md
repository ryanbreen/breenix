# Structure preflight parallelism — #890 — 2026-09-06

The preflight dispatches sorted suite stems with `xargs -P`, waits for dispatch
completion, then aggregates per-suite status files in the original sorted order.
`BREENIX_STRUCTURE_JOBS=1` serializes compilation and execution. An unset knob
uses `nproc`, then `sysctl -n hw.ncpu`, then 1, capped at 8. Invalid worker counts
are rejected at the environment boundary. Worker paths are exported variables,
not interpolated shell code. Logs retain `<gate_tmp>/breenix_gate_structure_preflight/<stem>.log`.
The existing census, marker format, failure wording, skip branches, four gate
call sites, wiring suite, and census suite are unchanged.

`scripts/run-structure-tests.sh` already uses `BINARY="${OUT_DIR}/${STEM}"`;
different suite stems in a preflight therefore have different output paths.
The runner is unchanged. This is not a compile cache and does not invoke
`cargo test`.

## Baseline profile

One sequential invocation per existing suite, before source caching, on this
arm64 Mac. Baseline source revision: `4f2b32e3`; subsequently fast-forwarded to
`87702857` (documentation and .gitattributes changes, no scanned kernel sources).
The profile captured its list before the new fixture suite was created.
Command for each stem from `/tmp/890-profile/stems.txt`:

```sh
/usr/bin/time -p bash scripts/run-structure-tests.sh "$stem" >"/tmp/890-profile/$stem.log" 2>&1
```

The table is ranked by the `real` line in those logs, also extracted into
`/tmp/890-profile/results.json`. Test counts come from their `test result:` lines.
The 52 baseline invocations exited 0. `uptime` sampled during profiling reported
load averages 4.40/2.03/2.31. The new fixture proof ran briefly alongside this
profile; no kernel build or other gate was launched by this session during it.
No competing rustc process was observed in the profiling agent's process sample.
This is a single observation per suite, not a benchmark distribution.

| Suite stem | Wall seconds | Exit |
| --- | ---: | ---: |
| `context_restore_structure` | 51.77 | 0 |
| `teardown_structure` | 16.14 | 0 |
| `ttbr0_shadow_reconciliation_structure` | 13.75 | 0 |
| `loopback_pump_structure` | 1.47 | 0 |
| `net_lock_structure` | 1.34 | 0 |
| `lockup_capture_guard_structure` | 1.17 | 0 |
| `tty_irq_fg_structure` | 0.77 | 0 |
| `tty_irq_pm_structure` | 0.74 | 0 |
| `gate_capture_drain_structure` | 0.66 | 0 |
| `exec_lock_order_structure` | 0.55 | 0 |
| `strand_handoff_structure` | 0.55 | 0 |
| `critical_path_logging_census_structure` | 0.43 | 0 |
| `serial_line_atomicity_structure` | 0.33 | 0 |
| `terminal_edge_capture_structure` | 0.24 | 0 |
| `syscall_return_register_structure` | 0.23 | 0 |
| `exit_tally_structure` | 0.22 | 0 |
| `block_request_lifetime_structure` | 0.19 | 0 |
| `capture_bxcap_schema_structure` | 0.19 | 0 |
| `entry_point_df_structure` | 0.18 | 0 |
| `gate_boot_facts_pipefail_structure` | 0.18 | 0 |
| `capture_path_lock_free_structure` | 0.16 | 0 |
| `x86_smp_enum_structure` | 0.16 | 0 |
| `mmap_floor_structure` | 0.14 | 0 |
| `qemu_kill_by_name_structure` | 0.14 | 0 |
| `preempt_bracket_structure` | 0.13 | 0 |
| `coreproof_coverage_structure` | 0.12 | 0 |
| `coreproof_mutation_register_structure` | 0.12 | 0 |
| `coreproof_sites_structure` | 0.12 | 0 |
| `dispatch_strand_census_structure` | 0.12 | 0 |
| `ext2_lock_structure` | 0.12 | 0 |
| `fcntl_pm_contention_gate_structure` | 0.12 | 0 |
| `green_program_envelope_structure` | 0.12 | 0 |
| `degenerate_transfer_fd_validation_structure` | 0.11 | 0 |
| `dispatch_fact_census_structure` | 0.11 | 0 |
| `dma_and_log_sink_structure` | 0.11 | 0 |
| `ext2_disk_size_structure` | 0.11 | 0 |
| `fork_lock_order_structure` | 0.11 | 0 |
| `parallels_kill_by_name_structure` | 0.11 | 0 |
| `qemu_host_lock_structure` | 0.11 | 0 |
| `coreproof_component_h_structure` | 0.10 | 0 |
| `dispatch_path_lock_free_structure` | 0.10 | 0 |
| `masked_binary_load_structure` | 0.10 | 0 |
| `poll_tcp_gate_wiring_structure` | 0.10 | 0 |
| `ring_span_report_site_structure` | 0.10 | 0 |
| `signal_eintr_predicate_structure` | 0.10 | 0 |
| `timer_wake_dispatch_structure` | 0.10 | 0 |
| `tty_oracle_structure` | 0.10 | 0 |
| `gate_boot_facts_structure` | 0.09 | 0 |
| `gate_structure_preflight_wiring_structure` | 0.09 | 0 |
| `run_inspector_import_structure` | 0.09 | 0 |
| `trace_ring_depth_structure` | 0.09 | 0 |
| `aarch64_testing_profile_structure` | 0.08 | 0 |

## Bounded source caching

Source inspection:
- `tests/context_restore_structure.rs`: 97 tests; five `rust_sources_below` call sites (three kernel, two aarch64; one inside a validator invoked repeatedly) plus four `text_sources_below` call sites of the assembly root; 79 `repo_text` call sites. Cached both Rust-tree roots and assembly-text tree with `OnceLock`, returning shared references. Mutation construction explicitly clones where needed. Single-file reads and parsers unchanged.
- `tests/teardown_structure.rs`: 92 tests; 26 `rust_sources_below` call sites (24 kernel root, two syscall root); 98 `repo_text` call sites. Cached those two Rust-tree snapshots with `OnceLock`; mutation tests clone the syscall snapshot before adding synthetic sources. Single-file reads and parsers unchanged.
- `tests/ttbr0_shadow_reconciliation_structure.rs`: 32 tests; nine `rust_sources_below` call sites, 9 of 9 at the kernel root; 13 `repo_text` call sites. Cached the kernel-tree snapshot with `OnceLock`, returning a shared reference. Single-file reads and parsers unchanged.

These bounded fixes remove actual repeated filesystem traversals, but the CPU/system timing split does not establish those reads as the dominant cost. Context and teardown repeatedly compute code masks and item censuses, while ttbr0 repeatedly scans function bodies and instruction-install inventories. Those parsers are untouched per scope. No parsing-cost reduction is claimed.

Background load: `uptime` during baseline context reported 4.40/2.03/2.31, then during fixed context 2.78/2.30/2.36; `ps -axo pid,etime,%cpu,command` sample showed context at ~499% CPU and no competing rustc process. The fixture-suite proof's own `finished in 1.19s` line (`/tmp/890-proof/fixture-fixed.log`) shows it briefly overlapped baseline plus editing, but no kernel builds. No claim of isolated/idle-host measurement.

One initial post-edit context compile failed with E0308/E0277 due to owned String/PathBuf call sites receiving cached references. Fixed by source.clone() and comparing borrowed PathBuf; failure log retained at `/tmp/890-profile/context_restore_structure.compile-failure.log`. No successful command was repeated to improve timing.

Post-cache validation, once successfully per modified suite using the same `/usr/bin/time -p bash scripts/run-structure-tests.sh <stem>` command, stdout/stderr captured to `/tmp/890-profile/<stem>.after.log`. 3 of 3 exit 0, no compile warning/error diagnostics.

| Suite | Before real | After real | Before user/sys | After user/sys | Result |
|---|---:|---:|---|---|---|
| `context_restore_structure` | 51.77 | 52.74 | 253.77/0.46 | 258.92/0.40 | 97 passed, 0 failed |
| `teardown_structure` | 16.14 | 16.18 | 38.29/0.50 | 38.48/0.26 | 92 passed, 0 failed |
| `ttbr0_shadow_reconciliation_structure` | 13.75 | 13.87 | 18.96/0.55 | 19.06/0.45 | 32 passed, 0 failed |

Comparison proof: Python selected the `running N tests` line, sorted per-test `test NAME ... ok` result lines, and the final `test result:` counts with only `; finished in ...` removed; exact UTF-8 byte equality holds for each before/after pair saved as `/tmp/890-profile/<stem>.{before,after}.canonical.txt`. All original 97/92/32 names and result counts remain identical. Raw stdout is not byte-identical because the Rust harness schedules concurrently and prints measured durations; no literal raw-byte identity claim is made.

The measured successful post-cache runs show no wall-time improvement for the three dominant suites. Cache benefit is limited to eliminating repeated source-tree reads; performance remains dominated elsewhere. CPU-heavy parsing/census optimization is follow-up work outside this bounded fix, and should be tracked in #890 without closing it.

## Behavioral fixture mutation proof

```sh
bash scripts/run-structure-tests.sh structure_preflight_parallel_structure > /tmp/890-parallel-suite.log 2>&1
```

The initial standalone command exited 0 (`/tmp/890-parallel-suite.log`).
The first full sequential preflight then exposed a fixture-directory race:
two concurrent test threads can obtain the same `SystemTime` timestamp. They
shared a directory and removed each other's logs. The fixture now adds a
process-local atomic counter to the PID/timestamp name. The corrected suite
was run with the same command, logging to `/tmp/890-proof/fixture-fixed.log`: 
exit 0, 2 passed, 0 failed, no compiler warnings. The fixture preflights use `/bin/bash` with `set -euo pipefail` and
copied real runners. Three synthetic suites run green, one assertion is made
red, then restored to green under both unset jobs and jobs=1. Assertions check
exact score lines, failure wording, individual logs, and sorted serial event
order. A separate jobs=2 rendezvous proves overlapping suite bodies and a
maximum of two active bodies. Temporary fixture roots (including spaces and a
quote) and their private `TMPDIR` artifacts are removed by `Drop`.

`/bin/bash --version` reports 3.2.57(1)-release (arm64-apple-darwin25).
`nproc` reports 18 on this host, so the unset preflight selects 8 workers.
The test does not depend on the caller setting a job count.

## Standalone preflight measurements

The command pair uses the same final source caches, with the sequential knob
as the regression escape hatch. The pair is not an uncached-source comparison.
`BREENIX_GATE_SKIP_STRUCTURE` is unset for these runs.

```sh
BREENIX_STRUCTURE_JOBS=1 /usr/bin/time -p /bin/bash -c 'source docker/qemu/lib/gate-structure-preflight.sh; gate_structure_preflight "$PWD" /tmp/gsp-check'
env -u BREENIX_STRUCTURE_JOBS /usr/bin/time -p /bin/bash -c 'source docker/qemu/lib/gate-structure-preflight.sh; gate_structure_preflight "$PWD" /tmp/gsp-check'
```

Development measurements are retained, not discarded: the first sequential
command exited 1, scoring 52/53 in 98.25s (`/tmp/890-proof/sequential.log`) due
to the fixture race above. The already-started parallel command exited 0,
scoring 53/53 in 57.60s (`/tmp/890-proof/parallel.log`). After the fixture fix,
the pair was rerun once to validate the final code, not to select a faster run.

Final pair, from `/tmp/890-proof/sequential-final.log`,
`/tmp/890-proof/parallel-final.log`, and `/tmp/890-proof/preflight-final-results.json`:

| Host | Jobs | Wall seconds | Exit | Suites |
| --- | --- | ---: | ---: | --- |
| Mac arm64, system Bash 3.2.57 | 1 | 99.00 | 0 | 53/53 |
| Mac arm64, system Bash 3.2.57 | unset (8 workers) | 58.33 | 0 | 53/53 |
| beast (Incus container `breenix-x86`, GNU Bash 5.2.21) | 1 | 337.488 | 0 | 53/53 |
| beast (Incus container `breenix-x86`, GNU Bash 5.2.21) | unset (8 workers) | 197.000 | 0 | 53/53 |

The final Mac pair saved 40.67s (41.1% of the sequential wall time), computed
from the two `real` lines above. These are single runs, not an expected speedup
under arbitrary load. `uptime` immediately before the final sequential and
parallel commands recorded 3.83/3.98/3.17 and 3.63/4.04/3.29 respectively in
`/tmp/890-proof/preflight-final-results.json`. No concurrent gate or kernel
build was launched by this session during these two measurements. The
profile's after-fix suite runs had already finished.

Both final logs contain exactly:

```text
[GATE_PREFLIGHT:structure_suites=53/53:critical_path_lines=260:pinned=120]
```

Per-suite logs were copied before the next invocation wiped the gate directory,
to `/tmp/890-proof/sequential-final-logs` and
`/tmp/890-proof/parallel-final-logs`. The unchanged wiring and census suites
passed as part of each full run.

## Beast measurement (driving agent, after the Codex round above)

Codex was scoped to the Mac worktree and had no SSH access, so the beast pair
above was run separately by the driving agent, from a fresh clone at
`/root/breenix-890p` inside beast's `breenix-x86` Incus container
(`ssh beast` then `sudo -n incus exec breenix-x86 -- bash -lc '<cmd>'`), on the
same commit this branch pushed, `cb1fd3a02ee7d2441fe91b6e7e15553c56771d42`
(`git log -1 --format=%H` inside the clone). `rust-fork` was symlinked to
`/root/breenix/rust-fork-real` per this round's own dispatching instructions;
the structure suites do not need it (`scripts/run-structure-tests.sh` is
`rustc --test`, no crate deps), so it plays no role in the numbers below.
`/bin/bash --version` on this container reports GNU Bash 5.2.21(1)-release
(x86_64-pc-linux-gnu) -- unlike the Mac's system Bash 3.2.57, so this pair does
not exercise the bash-3.2 compatibility concern; it exists to compare wall time
on the host that actually gates x86 merges. `nproc` reports 8, so the unset
knob selects 8 workers here too (the same cap as the Mac, coincidentally equal
to this host's full core count).

`/usr/bin/time` is not installed in this container, so timing used the bash
`time` keyword with `TIMEFORMAT=%R` (wall-clock seconds only; no user/sys
split is available for the beast rows, unlike the Mac's `/usr/bin/time -p`
three-line output):

```sh
BREENIX_STRUCTURE_JOBS=1 bash -c 'export TIMEFORMAT=%R; source docker/qemu/lib/gate-structure-preflight.sh; time gate_structure_preflight "$PWD" /tmp/gsp-check-beast'
env -u BREENIX_STRUCTURE_JOBS bash -c 'export TIMEFORMAT=%R; source docker/qemu/lib/gate-structure-preflight.sh; time gate_structure_preflight "$PWD" /tmp/gsp-check-beast-par'
```

The two runs used distinct gate-tmp directories (`/tmp/gsp-check-beast` and
`/tmp/gsp-check-beast-par`) and ran one after the other, not concurrently, so
neither could interfere with the other's log directory the way the Mac's
strict-gate attempt collided with a concurrent lane earlier in this note.
`uptime` immediately before each command: `load average: 0.07, 0.33, 1.80`
(sequential) and `load average: 0.11, 1.23, 1.89` (parallel) -- both runs on an
otherwise-idle container, no other build or gate observed running against it.
Full command output: `/tmp/890-beast-sequential.log` and
`/tmp/890-beast-parallel.log` inside the container. Both logs contain exactly
`[GATE_PREFLIGHT:structure_suites=53/53:critical_path_lines=260:pinned=120]`.

The beast pair saved 140.488s, 41.6% of the sequential wall time (337.488s to
197.000s) -- close to the Mac's 41.1%, on a host with 8 real cores rather than
8 of 18. These are single runs on both hosts, not a statistical sample; no
claim is made about the speedup holding under different load or a different
worker count.

## Strict ARM64 gate and build prerequisites

This worktree initially had no kernel or rootfs artifacts. Built its own
`boot_tests` kernel with:

```sh
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Exit 0 (`/tmp/890-proof/arm-build.log`). The build is **not warning-free**:
Cargo reports the pinned nightly's `core` NEON future-incompatibility warning.
`cargo report future-incompatibilities --id 1` exited 0 and saved the details
to `/tmp/890-proof/arm-future-incompat.log`. This is the existing open #559,
confirmed with `gh issue view 559`; changing the toolchain is outside this
round's scope, and no warning was suppressed. Modified host structure-suite
compiles themselves emitted no warnings.

The existing `/Users/wrb/fun/code/breenix/target/ext2-aarch64.img` and
`userspace/programs/aarch64/simple_exit.elf` from that worktree were copied into
this worktree at the same relative paths as prerequisites. Userspace/rootfs
were not independently rebuilt for this round.

The requested command was attempted:

```sh
bash docker/qemu/run-aarch64-boot-test-strict.sh 1
```

Exit 1, before boot (`/tmp/890-proof/strict.log`):

```text
[GATE_PREFLIGHT:structure_suites=0/0:critical_path_lines=260:pinned=120]
GATE_PREFLIGHT: FAIL (found 0 tests/*_structure.rs files under /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p890/wt/tests -- discovery itself is broken, not just red)
```

The log also reports the preflight's `stems` file missing. A concurrent gate
in the driving session's `scratchpad/battery/wt-45daec35` worktree was observed
with `ps -p 35782 -o pid,lstart,etime,command`, started at 21:16:40 local time,
after the final standalone pair. Both gates used the default shared `/tmp`
log namespace, which a preflight wipes at entry. This is evidence of concurrent
caller interference, not missing repository test files. The existing #825
per-lane `BREENIX_GATE_TMP` mechanism is the supported isolation for concurrent
gates. The unchanged runner's stem-only binary namespace also requires a
private `TMPDIR` when separate worktrees run the same stems concurrently;
cross-invocation binary isolation was not independently mutation-tested here.

Retried the failed gate using those existing environment settings:

```sh
BREENIX_GATE_TMP=/tmp/890-proof/strict-tmp TMPDIR=/tmp/890-proof/runner-tmp bash docker/qemu/run-aarch64-boot-test-strict.sh 1
```

The isolated gate exited 0 (`/tmp/890-proof/strict-isolated.log`):

```text
[GATE_PREFLIGHT:structure_suites=53/53:critical_path_lines=260:pinned=120]
PASS: 1/1 boots succeeded
```

Its `GATE_BOOT_FACTS` receipt records `qemu_at_start=0`, `load_at_start=9.80`,
`load_at_end=9.44`, `guest_uptime_ms=10277`, and `ended_by=scored_pass`.
The concurrent battery described above had finished by the post-gate process
check. The gate's binary guards also passed (no forbidden FP/SIMD memory
instructions, no allocator reachable from the lockup dump), as recorded in
`/tmp/890-proof/strict-isolated.log`. QEMU cleanup afterward found no residual
QEMU processes.

## Not claimed and remaining work

- Beast CPU-time breakdown: not available (`/usr/bin/time` is not installed in the `breenix-x86` container), so only wall-clock (`time`'s `%R`) is reported for the beast rows, unlike the Mac's user/sys split.
- Repeated or statistically sampled beast measurements: one sequential run and one parallel run, back to back, not concurrent, not repeated.
- A measured wall-time improvement from source caching: 0 of 3 post-cache runs were faster than their baselines.
- Parser/masking/census CPU optimization: unchanged; follow-up stays in #890.
- Literal byte equality of raw concurrent test stdout: only normalized names/results/counts match, as documented above.
- An idle host, repeated-sample statistical confidence, or performance on other core counts.
- x86 boot or either production-profile boot gate execution in this round.
- Fresh userspace/rootfs builds or absence of the pinned toolchain warning tracked in #559.
- Safe concurrent preflights sharing a gate temp directory or runner TMPDIR; use the existing per-lane environment settings. The cross-worktree runner namespace remains a follow-up item for #890, not a new compile-cache attempt.

#890 remains open for the parser/census CPU-cost work and the cross-worktree runner-namespace follow-up; the beast measurement this note originally deferred is now included above. No PR was opened or merged, and no main-branch change was made.

## Pre-commit quality and claim lint

The required x86 build command, with the documented fork-library path override
for this fresh worktree, exited 0:

```sh
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix/rust-fork/library cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

`/tmp/890-proof/x86-build.log` contains no `warning:` or `error:` diagnostics.
The structure-suite compiler logs in `/tmp/890-proof/sequential-final-logs`,
`/tmp/890-proof/parallel-final-logs`, and `/tmp/890-profile/*.after.log` likewise
contain none. The ARM64 toolchain warning is separately disclosed above.
`git diff --check` and `/bin/bash -n docker/qemu/lib/gate-structure-preflight.sh`
exited 0.

Initial prose lint findings (unquantified wording and a heading lacking its
mutation context) were corrected before committing. Final invocations:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/890-proof/commit-message.txt -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/890-proof/commit-message-f8d2b7d4.txt -> exit 0
```

## 2026-09-06 Astra fix pass: P-1, P-2, P-12a mutation evidence

The helper in docker/qemu/lib/gate-structure-preflight.sh now allocates a
private mktemp directory per invocation and exports it as TMPDIR for runner
binaries (P-1). It dispatches each suite through timeout with a default
300-second budget (P-2). The shared lexical validator accepts 1..999999 for
jobs and timeout, rejecting longer digit strings before xargs (P-12a).

tests/structure_preflight_parallel_structure.rs adds three behavioral tests.
The concurrent test spawns two preflights before waiting, with distinct
fixture roots, a shared fresh gate_tmp and inherited TMPDIR, and matching
alpha stems. Each compiled suite checks its compile-time root against its
runtime root and rendezvous with its peer to compare binary namespaces.
The harness also checks two private directories and their logs/binaries.
Existing repeated-run log reads discover exactly one newly created directory
per call and retain the exact failure-message path assertion.

The following filtered runner executions temporarily reverted only the
relevant production fix, then restored the helper and reran the same test.
P-1 restored the old directory block and removed the TMPDIR export; its
observed symptom was 0/1, not the earlier round's 0/0. P-2 removed only the
timeout wrapper. P-12a restored the previous jobs case validation.
The P-2 reproduction used Python Popen(start_new_session=True) and
communicate(timeout=12); on expiry os.killpg targeted that reproduction's
own process group with SIGKILL. This is an external watchdog failure,
not a completed assertion from the hung test. The fixed run completed
with the test's under-20-second, non-success-status, and 0/1 assertions.

Raw command output is retained locally under /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/890-fix-proof.xsnxxkqo; output follows.

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure concurrent_invocations
MUTATION: concurrent_invocations
RESULT: exit 101; elapsed 10.25s
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure concurrent_invocations ==

running 1 test

thread 'concurrent_invocations_isolate_logs_and_same_stem_binaries' panicked at /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p890/wt/tests/structure_preflight_parallel_structure.rs:239:9:
Output { status: ExitStatus(unix_wait_status(256)), stdout: "[GATE_PREFLIGHT:structure_suites=0/1:critical_path_lines=0:pinned=0]\n", stderr: "GATE_PREFLIGHT: FAIL (1 of 1 structure suite(s) red: alpha_structure -- per-suite logs under /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/gsp fixture ' 98241 1788747458425986000 2/breenix_gate_structure_preflight)\n" }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test concurrent_invocations_isolate_logs_and_same_stem_binaries ... FAILED

failures:

failures:
    concurrent_invocations_isolate_logs_and_same_stem_binaries

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 10.12s

```

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure concurrent_invocations
RESTORED: exit 0
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure concurrent_invocations ==

running 1 test
test concurrent_invocations_isolate_logs_and_same_stem_binaries ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.11s

```

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure hung_suite
MUTATION: hung_suite
RESULT: external watchdog expired at 12s; killed this reproduction process group; elapsed 12.00s
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure hung_suite ==

running 1 test
```

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure hung_suite
RESTORED: exit 0
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure hung_suite ==

running 1 test
test hung_suite_returns_red_within_budget ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 2.02s

```

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure oversized_jobs
MUTATION: oversized_jobs
RESULT: exit 101; elapsed 0.14s
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure oversized_jobs ==

running 1 test

thread 'oversized_jobs_are_configuration_errors' panicked at /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p890/wt/tests/structure_preflight_parallel_structure.rs:306:9:
xargs: -P 2147483648: too large
GATE_PREFLIGHT: FAIL (1 of 1 structure suite(s) red: alpha_structure -- per-suite logs under /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/gsp fixture ' 3652 1788747483065240000 0/gate/breenix_gate_structure_preflight.dW2AeF)

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test oversized_jobs_are_configuration_errors ... FAILED

failures:

failures:
    oversized_jobs_are_configuration_errors

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.01s

```

```text
COMMAND: bash scripts/run-structure-tests.sh structure_preflight_parallel_structure oversized_jobs
RESTORED: exit 0
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure oversized_jobs ==

running 1 test
test oversized_jobs_are_configuration_errors ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.01s

```

Direct /bin/bash validation of both oversized inputs, with the helper sourced
and its return status captured immediately, printed:

    value=2147483648 length=10 status=1
    value=999999999999999999999999999999999999 length=36 status=1

Neither invocation emitted an arithmetic diagnostic. The fixed oversized-jobs
test runs both inputs and checks configuration-error diagnostics without
xargs errors or red-suite attribution.

Final whole-suite runner output after restoring the fixes and rustfmt:

```text
bash scripts/run-structure-tests.sh structure_preflight_parallel_structure -> exit 0
== compiling structure_preflight_parallel_structure ==
== running structure_preflight_parallel_structure  ==

running 5 tests
test oversized_jobs_are_configuration_errors ... ok
test concurrent_invocations_isolate_logs_and_same_stem_binaries ... ok
test two_jobs_overlap_and_never_exceed_the_bound ... ok
test mutation_is_green_red_green_with_default_and_one_job ... ok
test hung_suite_returns_red_within_budget ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

```

```text
bash scripts/run-structure-tests.sh gate_structure_preflight_wiring_structure -> exit 0
== compiling gate_structure_preflight_wiring_structure ==
== running gate_structure_preflight_wiring_structure  ==

running 4 tests
test missing_wiring_validator_rejects_a_gate_with_neither ... ok
test shared_lib_defines_the_preflight_function_and_its_marker_line ... ok
test every_target_gate_calls_the_structure_preflight ... ok
test missing_wiring_validator_rejects_a_gate_with_the_call_site_removed ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

These compiler outputs contain no warning/error diagnostics. git diff --check
and /bin/bash -n docker/qemu/lib/gate-structure-preflight.sh each exited 0.

Not claimed:

- Kernel builds, QEMU boots, beast execution, or a full preflight of the tree.
- Independent verification of orphan cleanup after the production timeout.
- Repeated race statistics or reproduction of the historical 0/0 symptom.
- Isolation for standalone run-structure-tests.sh callers outside this helper.

This section supersedes the earlier shared-preflight-namespace deferral for
these tested helper invocations; parser/census CPU-cost work remains in #890.
No issue closure or PR merge is part of this fix pass.

The first tree claim-lint invocation exited 1 on the header's numeric phrase
describing the prohibited initial digit. The comment now states “the first digit in 1..9” to describe
the same validation without that lint ambiguity.

The next tree lint invocation exited 1 on the quoted numeric wording in this
note; that quotation was rephrased. The first commit-message lint exited 1
on an issue-keyword adjacency; the title was changed to put #890 last.

Final claim-lint invocations after those corrections:

    python3 scripts/claim-lint.py -> exit 0
    python3 scripts/claim-lint.py --commit-msg /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/890-fix-proof.xsnxxkqo/commit-message.txt -> exit 0

Tree output: claim-lint: clean (6 file(s) checked, changed hunks vs 87702857c979).
The tool separately reports 177 pre-existing findings outside changed hunks;
this run does not certify the whole-file backlog.

## Landing

`git fetch origin` then `git merge origin/main` from branch tip `30bfaa08`
(after the P-10a/P-11 prose-fix commit `f4a9ceb2`) produced merge commit
`5db824c2` on top of `origin/main`'s `45daec35` (PR #907, x86 provider-gate
tracing docs). Git reported no conflicts (`Merge made by the 'ort'
strategy.`); `git status --short` was empty afterward.

`bash scripts/run-structure-tests.sh` (no stem argument, so its documented
default `teardown_structure`) at `5db824c2`: exit 0, 92 passed, 0 failed,
`finished in 15.41s`, `real 16.44` (`/tmp/890-proof/landing-structure-tests.log`).

`cargo build --release --features boot_tests --target aarch64-breenix-kernel.json
-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel
--bin kernel-aarch64` at `5db824c2`: exit 0, no warnings from this repository's
own crates; the only warning line is the pre-existing toolchain
future-incompat notice for `core` already disclosed above
(`/tmp/890-proof/landing-aarch64-build.log`).

One `bash docker/qemu/run-aarch64-boot-test-strict.sh 1` at `5db824c2`, run
under `bash -x` with `PS4='+PS4TS $(date "+%s.%N") '` solely to timestamp the
production `gate_structure_preflight` call/return pair inside this real
invocation (not a separate standalone re-derivation): exit 0, `PASS: 1/1
boots succeeded`, `[OK] Boot 1: SUCCESS`
(`/tmp/890-proof/landing-strict-boot-xtrace.log`). GATE_PREFLIGHT line, unset
`BREENIX_STRUCTURE_JOBS` (default parallel path, this Mac's `nproc`-derived
worker count capped at 8, per the "Behavioral fixture mutation proof"
section above):

```text
[GATE_PREFLIGHT:structure_suites=54/54:critical_path_lines=260:pinned=120]
```

54/54 (up from the 53/53 measured earlier in this note) because the merged
`origin/main` PR #907 added `tests/tracing_provider_gate_structure.rs`. The
preflight's own wall-clock, from the xtrace timestamp on the
`gate_structure_preflight` function's entry line (`1788748162.756631000`) to
its `return 0` line (`1788748225.180168000`): **62.42s**
(`/tmp/890-proof/landing-strict-boot-xtrace.log`).

Claim-lint on the tree and on this Landing commit's own message, at HEAD
after this section was written:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/890-proof/commit-message-landing.txt -> exit 0
```
