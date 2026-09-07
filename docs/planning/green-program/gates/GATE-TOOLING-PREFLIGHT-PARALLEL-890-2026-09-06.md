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

Background load: `uptime` during baseline context reported 4.40/2.03/2.31, then during fixed context 2.78/2.30/2.36; `ps -axo pid,etime,%cpu,command` sample showed context at ~499% CPU and no competing rustc process. Main agent reports a ~1.2s fixture-suite proof briefly overlapped baseline plus editing, but no kernel builds. No claim of isolated/idle-host measurement.

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
| beast | 1 | not measured | — | — |
| beast | unset | not measured | — | — |

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

Beast was deliberately not contacted: this agent is scoped to the Mac worktree
and has no SSH access. The driving agent must run the same command pair on
beast and fold the actual outputs and host load into this note. No estimate
or extrapolation substitutes for that measurement.

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

- Beast timings or Linux execution: not attempted, explicitly assigned to the driving agent.
- A measured wall-time improvement from source caching: 0 of 3 post-cache runs were faster than their baselines.
- Parser/masking/census CPU optimization: unchanged; follow-up stays in #890.
- Literal byte equality of raw concurrent test stdout: only normalized names/results/counts match, as documented above.
- An idle host, repeated-sample statistical confidence, or performance on other core counts.
- x86 boot or either production-profile boot gate execution in this round.
- Fresh userspace/rootfs builds or absence of the pinned toolchain warning tracked in #559.
- Safe concurrent preflights sharing a gate temp directory or runner TMPDIR; use the existing per-lane environment settings. The cross-worktree runner namespace remains a follow-up item for #890, not a new compile-cache attempt.

#890 remains open for the beast measurements and further CPU-cost work. No PR
was opened or merged, and no main-branch change was made.

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
```
