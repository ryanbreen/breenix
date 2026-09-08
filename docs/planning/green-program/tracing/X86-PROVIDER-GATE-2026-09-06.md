# X86 tracing provider gate — round record, 2026-09-06

Date: 2026-09-06. Branch: `tracing/x86-provider-gate`.

Starting commit, re-derived in this worktree with `git merge-base HEAD origin/main`:

```text
5bfc7077af7dfde2e0aa81189088cc838584dea3
```

The beast runs used WIP commit `e3aa05972d3aad6cdfa617c8522fb29e431d12ef`.
The final source commit is `e6115a4922136c196bfc89179d31069b27ca1a60`.
`git diff --exit-code e3aa05972d3aad6cdfa617c8522fb29e431d12ef HEAD`
after the source commit exited 0: it replaces the WIP without changing its tree.
No PR was opened and no merge was performed by this task.

## Changes

| File | Change |
| --- | --- |
| `kernel/src/test_framework/registry.rs` | Add the direct x86 `boot_tests` wrapper around the existing provider; restrict this provider's registry entry to `Arch::Aarch64`. |
| `kernel/src/main.rs` | Call that wrapper after `run_x86_ring_span_gate`, before the nearest subsequent interrupt-disable call, under the same shipping `boot_tests` cfg. |
| `docker/qemu/run-x86-boot-tests.sh` | Require one START, one PASS and no FAIL for this provider in `serial_user.txt`, using a bare awk assertion between ring-span and wake-latency scoring. |
| `tests/tracing_provider_gate_structure.rs` | Add 17 tests covering wrapper/result dependence, exact cfg and call ordering, registry architecture, shell wiring, in-memory regressions, and execution of the extracted shell block against synthetic serial shapes. |

The registry restriction prevents a future `x86_staged_registry` x86 build
from dispatching the provider a second time in addition to the direct call.
Aarch64's registry match is unchanged: both its former `Arch::Any` and its
new `Arch::Aarch64` match that target. No other TestDef architecture changed.
The staged x86 executor remains off. The provider implementation is unchanged.
`process_task.rs` was mutated only in the dedicated beast clone for the
negative control below, then restored. No Tier-1 file was edited.

## Evidence location and local structure checks

Full command output and raw serial files are in the session evidence directory
outside the repository (not committed):

```text
/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/slot1/evidence
```

The initial session's existing outputs were retained and inspected; the four
source/test edits were not re-applied. `git status --short --branch` showed
three modified tracked files and the new untracked structure suite;
`git diff --stat` showed 65 insertions and 1 deletion across the three tracked
files. The WIP commit included the new 285-line test file as well.

Commands from Step 3, with saved output files:

```bash
bash scripts/run-structure-tests.sh tracing_provider_gate_structure
for suite in teardown_structure trace_ring_depth_structure ring_span_report_site_structure gate_structure_preflight_wiring_structure gate_capture_drain_structure; do
    bash scripts/run-structure-tests.sh "$suite"
done
(cd tests && find . -maxdepth 1 -type f -name '*_structure.rs' -print | sed 's|^\./||; s/\.rs$//' | sort) \
  | while read -r stem; do
      echo "=== $stem ===";
      bash scripts/run-structure-tests.sh "$stem" || echo "RED: $stem";
    done
```

`tracing-provider-structure.txt` records:

```text
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`selected-structure.txt` records these five suite results in command order:

```text
test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 17.51s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.53s
```

The complete discovery-loop output is `all-structure.txt`; its parsed tally,
rechecked during this continuation, is:

```text
52 discovered suites passed; 0 RED; 790 tests passed; 0 tests failed
```

No pre-existing red was observed in that loop. The ARM64 gate and each of
the four beast gate invocations also ran the structure preflight without a
skip flag, reporting:

```text
[GATE_PREFLIGHT:structure_suites=52/52:critical_path_lines=260:pinned=120]
```

The critical-path line count is informational output of the existing
preflight; it is not a new tracing-provider assertion.

## ARM64 build, baseline notice check, and strict regression

The existing primary-checkout image was available at
`<local-checkout>` and the worktree's
`target/ext2-aarch64.img` already symlinked to it. No ARM64 userspace image
was rebuilt.

The same build command was run against the unmodified `origin/main` tree
at the starting SHA, using a temporary stash of the four edits:

```bash
git stash push -u -m slot1-notice-baseline
git diff origin/main --exit-code
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
cargo report future-incompatibilities --id 1
cmp ../evidence/aarch64-future-incompatibilities.txt ../evidence/aarch64-origin-main-future-incompatibilities.txt
git stash pop
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

`git diff origin/main --exit-code` exited 0 before the baseline build.
`aarch64-origin-main-build.txt` contains:

```text
    Finished `release` profile [optimized] target(s) in 8.32s
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (<local-checkout>)
note: to see what the problems were, use the option `--future-incompat-report`, or run `cargo report future-incompatibilities --id 1`
baseline build exit: 0
```

The detailed baseline report in
`aarch64-origin-main-future-incompatibilities.txt` is byte-identical to
`aarch64-future-incompatibilities.txt` (`cmp` exit 0). Matching notice text:

```text
warning: enabling the `neon` target feature on the current target is unsound due to ABI issues
```

Its source locations are in the pinned nightly's
`core/src/../../stdarch/crates/core_arch/src/arm_shared/neon/generated.rs`,
including line 422. The baseline build had one future-incompatibility summary
notice and no other warning/error output. The restored branch build also
exited 0 (`aarch64-restored-build.txt`, finished in 8.19s), with the same notice.

This exact NEON-ABI notice is a known toolchain notice, not a kernel-code
warning and not new: the unmodified baseline reproduces it. Following the
precedent in [CRITICAL-PATH-DEBT-PR1-2026-09-06.md](../gates/CRITICAL-PATH-DEBT-PR1-2026-09-06.md)
(the later gate-results table's identical ARM64 build command), it is
pre-existing/orthogonal and is not fixed, suppressed, or worked around by this
task. This is an exit-0 build with a notice, not a claim of a clean build.

The first strict invocation was stopped during preflight (exit 143) after
observing another worktree using the default `/tmp/breenix_aarch64_strict_*`
paths. That invocation completed 0 boots. It was restarted with isolated
output/disk paths and the existing shared host lock still enabled:

```bash
mkdir -p ../evidence/aarch64-tmp
BREENIX_GATE_TMP="$PWD/../evidence/aarch64-tmp" \
  bash -o pipefail -c 'bash docker/qemu/run-aarch64-boot-test-strict.sh 3 2>&1 | tee ../evidence/aarch64-strict-3.txt'
```

`aarch64-strict-3.txt`, exit 0:

```text
  [OK] Boot 1: SUCCESS
  [OK] Boot 2: SUCCESS
  [OK] Boot 3: SUCCESS
Total iterations: 3
Successes: 3
Failures: 0
Success rate: 100%
Duration: 72s
PASS: 3/3 boots succeeded
```

## Beast positive gates

Commands were executed with
`ssh beast 'sudo -n incus exec <x86-build-environment> -- bash -s'`, feeding the exact
runner scripts reproduced in the appendix. The lane used `<isolated-checkout-221>`,
`TMPDIR=<isolated-checkout-222>`, and
`BREENIX_RUST_FORK_LIBRARY=<rust-fork-checkout>/library`.
Setup cloned the pushed WIP branch, linked the existing Rust fork, copied
fonts and the existing userspace ELF files. The required ELF glob was nonempty;
no alternative ELF staging location was needed.

The WIP was committed and pushed with:

```bash
git add kernel/src/main.rs kernel/src/test_framework/registry.rs docker/qemu/run-x86-boot-tests.sh tests/tracing_provider_gate_structure.rs
git commit -m 'wip: x86 tracing provider gate (evidence in progress)' \
  -m 'Co-Authored-By: Ryan Breen <ryan.breen@gmail.com>
Co-Authored-By: Claude Code <noreply@anthropic.com>'
git push -u origin tracing/x86-provider-gate
```

Output included:

```text
[tracing/x86-provider-gate e3aa0597] wip: x86 tracing provider gate (evidence in progress)
 * [new branch]        tracing/x86-provider-gate -> tracing/x86-provider-gate
```

The directory called `before-boot` was **the branch's own gate, full clone,
run once**. It was not an unmodified-main boot, and supplies no unmodified-baseline observation. `beast-branch-once.txt` / `before-boot.txt`, exit 0:

```text
x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0
x86 frame-custody gate run 1: PASS
2
```

The final `2` is the requested grep count for the provider marker family.

The prescribed second boot-tests run (`x86-boot-tests.txt`), exit 0:

```text
x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0
x86 frame-custody gate run 1: PASS
```

The production run (`x86-prod.txt`), exit 0:

```text
PASS: x86 production profile reached steady state with the teardown census at rest
  test-only marker '[TEST:': 0
```

The explicit serial checks (`beast-marker-check.txt`), exit 0:

```text
boot-tests serial: started=1 passed=1 failed=0
production serial tracing-provider marker count: 0
```

Serial paths in the container:

```text
<isolated-checkout-222>/before-boot/breenix_x86_boot_tests_1/serial_user.txt
<isolated-checkout-222>/main-boot/breenix_x86_boot_tests_1/serial_user.txt
<isolated-checkout-222>/main-prod/breenix_x86_prod_profile/serial_user.txt
```

The x86 compile stages produced no lines matching `^warning|^error` in the
four gate logs. This does not describe the entire packaging output as clean:
the unmodified disk-packaging script attempted its optional BusyBox build and
reported the following on these runs:

```text
  busybox.elf not found, attempting to build...
Error: x86_64-linux-musl-gcc not found in PATH
  WARNING: BusyBox build failed (see build-busybox.sh for prerequisites)
  WARNING: busybox.elf not found, skipping coreutils
```

`ssh beast 'sudo -n incus exec <x86-build-environment> -- find <canonical-checkout> -type f -name busybox.elf -print'`
exited 0 with no output. No BusyBox compiler was installed and no userspace
source was changed. The gate scripts continued with the supplied ELF payload;
the positive verdicts above do not claim BusyBox coverage.

## Runtime mutation, earlier failure, and restoration

The mandatory on-disk mutation removed only this unique fragment from
`kernel/src/task/process_task.rs` in the beast clone:

```rust
        crate::trace_count!(
            crate::tracing::providers::teardown::DEFERRED_FAULT_RING_DROPPED
        );
```

`runtime-mutation.diff` records the actual diff. The runner backed up the file,
installed an EXIT restoration trap, rebuilt and rebooted via the full gate,
then checked the provider markers and restored the file. The positive build's
allocation-guard ELF hash was
`df1e09446571c6d41a7e295eb3d9dadf8a4e832a5f69e3f0498feddf943efb87`;
the mutation build's was
`445257dc365641733b4ba8a9811942c6387a4a201a665766b1027d0cf883c9a8`.

`beast-mutation.txt` contains:

```text
[TEST:process:deferred_fault_ring_overflow_injection:START]
[TEST:process:deferred_fault_ring_overflow_injection:FAIL:deferred fault ring overflow was not counted and drained]
x86 frame-custody gate run 1: FAIL (set -e abort at docker/qemu/run-x86-boot-tests.sh:1060, exit 1)
  failing command: test "$passed" = true
mutation gate exit: 1
mutation serial: started=1 passed=0 failed=1
mutation reverted and confirmed identical to backup
```

**The new awk assertion was not reached by this full mutation gate.** After
the provider FAIL, the kernel panicked at `kernel/src/task/softirq_tests.rs:228:5`:

```text
ksoftirqd should have processed deferred softirqs (tid=Some(2))
```

The poll loop stopped on the crash marker and the earlier `passed` assertion
failed. This matches the existing signature in [issue #891](https://github.com/ryanbreen/breenix/issues/891),
read using `gh issue list --state open --search 'ksoftirqd' --limit 30 --json number,title,url`
and `gh issue view 891 --json number,title,body,url` (`issue-891.json`). This
round does not establish why that panic occurred or attribute it to the
counter mutation. No softirq fix or test weakening was attempted. The run was
not retried to conceal this outcome. The runtime provider result is negative;
the full-script failure cannot be credited to the new assertion.

As supplemental evidence, the literal block between `# (5a) tracing framework:`
and `# (6) #766` was extracted from the actual local gate script into
`extracted-provider-gate.sh`. It was run with `bash -c`, `set -e`, and
`OUTPUT_DIR` pointing to each copied **actual** serial directory. It was not
hand-transcribed or supplied synthetic input. `actual-serial-oracle.txt`:

```text
actual before-boot serial, extracted provider assertion exit: 0
actual main-boot serial, extracted provider assertion exit: 0
actual mutation serial, extracted provider assertion exit: 1
```

This establishes rejection of the real mutation serial by the new assertion
in isolation; it does not replace the full gate's earlier-failure record.
The synthetic archived-baseline/malformed-shape rejection remains separately
covered by the 17-test structure suite.

**Residual gap, recorded for a future round:** the new awk assertion has
still not been exercised end-to-end inside one unbroken full
`run-x86-boot-tests.sh` execution that reaches it via the mutated provider
path, because the softirq panic (#891) aborts the script before that point
on this mutation boot. A future round should re-run this mutation gate once
#891 no longer intervenes on this boot (either fixed, or by chance not
firing) to obtain a direct full-gate red at the new assertion itself; until
then, the mutation-rejection evidence for this specific assertion is the
extracted-shell-block replay above against the real captured serial, not a
direct full-gate result.

Restoration was checked in the runner with `cmp`, then independently with:

```bash
cd <isolated-checkout-221>
cmp <isolated-checkout-223>/process_task.before-mutation.rs kernel/src/task/process_task.rs
git diff --exit-code -- kernel/src/task/process_task.rs
```

Both exited 0. The copied backup also matched the unchanged local file with
`cmp ../evidence/process_task.before-mutation.rs kernel/src/task/process_task.rs`.
A `/proc/*/cmdline` check identified no QEMU process with this lane's paths:

```text
remote process_task.rs restored: cmp=0, git diff=0
lane-owned QEMU PIDs: []
```

## Evidence transfer deviation

The specified command was attempted:

```bash
scp 'beast:<isolated-checkout-223>/*' ../evidence/
```

It exited 1 (`scp-attempt.txt`):

```text
scp: remote readdir("<isolated-checkout-223>/"): Permission denied
scp: <isolated-checkout-223>/*: No such file or directory
```

The artifacts are inside `<x86-build-environment>`, not the SSH user's host filesystem.
Instead, these commands copied the evidence and raw serials through remote-environment;
both SSH/tar transfers and both local extractions exited 0:

```bash
ssh beast 'sudo -n incus exec <x86-build-environment> -- tar -C <isolated-checkout-223> -cf - .' > ../evidence/beast-evidence.tar
tar -xf ../evidence/beast-evidence.tar -C ../evidence
ssh beast 'sudo -n incus exec <x86-build-environment> -- tar -C <isolated-checkout-222> -cf - before-boot/breenix_x86_boot_tests_1/serial_user.txt before-boot/breenix_x86_boot_tests_1/serial_kernel.txt main-boot/breenix_x86_boot_tests_1/serial_user.txt main-boot/breenix_x86_boot_tests_1/serial_kernel.txt mutation/breenix_x86_boot_tests_1/serial_user.txt mutation/breenix_x86_boot_tests_1/serial_kernel.txt main-prod' > ../evidence/beast-serials.tar
mkdir -p ../evidence/beast-serials
tar -xf ../evidence/beast-serials.tar -C ../evidence/beast-serials
```

## Issue-scope question for review

Does this direct x86 route exhaust the tracing row's own functional-test
evidence obligation, so that the remaining open issues (#533, #680, #681)
belong only to the boot/scheduler rows from here on — or does this PR supply
evidence only, with no colour change, because those issues are still
tracing-layer issues under the maintained rubric?

This task does not answer that question itself. It is for whoever reviews
this PR, including assessment of the mutation gate's earlier-failure limitation.
No issue was closed or reclassified by this task. The separately observed
softirq panic already has issue #891.

## Review in-layer ruling

The review pass answered the issue-scope question above and recorded the
following ruling (`tracingBlocking: true`), reproduced verbatim:

> **Reasoning:** Two separate questions, answered separately. (1) Are
> #533/#680/#681 themselves tracing-layer defects that would keep Tracing x86
> at UNVERIFIED? No. Row 20's path is kernel/src/tracing/ (19 files) +
> kernel/src/fs/procfs/trace.rs (confirmed from atlas-data.json). #533 lives
> in kernel/src/test_framework/ (registry executor + kernel_main_continue
> orchestration) -- row 21/row 2. #680 is a kthread_join hang in
> kernel/src/task/kthread.rs -- row 6. #681 is sys_exit's premature
> terminal-verdict publication in kernel/src/syscall/handlers.rs -- row
> 8/21. None touches kernel/src/tracing/. They were chip-tagged to row 20
> only because, until this PR, they collectively blocked the only avenue
> (the off-by-default x86_staged_registry feature) through which the
> tracing row's one functional test could run on x86 at all -- and this PR
> supplies an independent, already-precedented route around all three (the
> same direct-call shape as the existing run_x86_ring_span_gate), landing in
> the shipping boot_tests profile with a real gate assertion. So the
> tracing row's own stated functional obligation ("the one registered
> test... is undispatched") is now met independent of whether #533/#680/#681
> are ever fixed. They should be removed from row 20's chip list on both x86
> and blended views (and, since #680/#681 currently chip ONLY row 20, added
> to their correct homes -- rows 6 and 8/21 respectively -- as a
> housekeeping item, not part of this PR's scope). (2) Given that, can the
> atlas honestly declare Tracing x86/blended HIGH with "no open in-layer
> issues" today? No -- issue #855, filed 2026-09-06 independent of this
> branch, is a genuine open defect in the tracing framework's own ring
> buffer (kernel/src/tracing/core.rs, buffer.rs -- squarely inside row 20's
> path), describing real retained-history being 15-30ms rather than seconds
> under realistic event density. It has nothing to do with #533/#680/#681
> or with this PR's scope, and it is not fixed or excluded by this PR. A
> live post-merge issue sweep run today therefore does NOT come back clean
> for the tracing row, even after correctly excluding #533/#680/#681.
>
> **Allowed prose (MAY say):** the shipping x86 boot_tests profile now
> directly executes the deferred-fault-ring overflow provider test in a
> shippable configuration with a gate that requires actual PASS and rejects
> missing/failed/duplicate results (subject to the narrow F2/F3
> gate-strictness caveats above); #533/#680/#681 are
> registry-dispatch/scheduler/syscall-layer issues, not tracing-layer
> issues, and do not block this row (with citations to the actual
> files/rows they belong to); full x86_staged_registry execution remains
> separately tracked (rows 2/21), not asserted as working here.
>
> **Allowed prose (MAY NOT say):** "the post-merge issue sweep finds no
> open tracing-layer defects" or otherwise declare a clean zero-issue HIGH
> without addressing or explicitly carrying #855 as a remaining open
> in-layer issue against this row; that #680 or #681 are resolved/closed
> (they are re-scoped away from tracing, not fixed); that full
> staged-registry execution, RELIABILITY, or a live (non-synthetic)
> fault-path result is established by this PR (the round doc's own "Not
> claimed" section already correctly disclaims these).

This ruling supersedes the "no post-merge issue sweep was performed"
statement that previously closed this section: the review pass did run a
live post-merge sweep for this row (`gh issue view 855`, confirmed OPEN,
filed 2026-09-06T02:44:13Z, independent of this branch) and found it not
clean. #533/#680/#681 are removed from row 20's chip list per the ruling
above, but #855 is carried forward as the row's one remaining open
in-layer issue.

## Proposed atlas replacement prose (NOT applied)

The following was originally drafted as verbatim proposed text pending the
review question above. Per the ruling recorded just above, its original
"no open tracing-layer defects"/"no open in-layer issues" clauses were false
against a live sweep (issue #855 is open and in-layer) and have been
corrected below to carry #855 forward instead of claiming a clean sweep;
this corrected text remains proposed, not applied, by this document:

> Tracing × x86 — complete/HIGH (PROPOSED): "HIGH: the shipping x86 boot_tests profile directly executes the deferred-fault-ring overflow provider test, its gate requires the actual PASS and rejects missing or failed results, and the sampling/report-site mutations and production regression gate pass; #533/#680/#681 are registry-dispatch/scheduler/syscall-layer issues (not tracing-layer) and do not block this row; issue #855 (trace-ring retention window, in-layer) remains open and is carried forward rather than dropped; full staged-registry execution remains tracked in the boot/scheduler rows."
>
> Tracing × blended — complete/HIGH (PROPOSED): "HIGH follows the existing aarch64 tracing evidence and the newly gated x86 provider execution in the shipping profile; issue #855 (trace-ring retention window) remains open and in-layer and is carried forward rather than dropped; neither seconds of retained history nor full staged-registry execution is asserted."

This document does not itself declare either cell HIGH. No atlas file in
this repository was edited by this task. No atlas file exists here; the atlas
lives outside this repository. A live post-merge issue sweep for this row
WAS performed by the review pass (see the ruling above) and returned one
open in-layer issue, #855, rather than the clean sweep the original draft
above assumed.

## Not claimed

- Full x86 registry execution.
- The #567/#680 scheduler repair.
- The #681 fix: its decisive logic is `sys_exit`'s premature terminal-verdict
  publication in `kernel/src/syscall/handlers.rs` (the syscall exit-verdict
  path), not the scheduler.
- A live process fault: the injection uses a private fixture with synthetic TIDs, not a real fault path.
- RELIABILITY.
- Any atlas colour change; the review ruling above says what MAY be said, but no atlas file was edited by this task.
- That the full mutation gate reached the new assertion; it stopped earlier on the softirq panic.

## Runner command appendix

The following scripts were supplied on stdin to:

```bash
ssh beast 'sudo -n incus exec <x86-build-environment> -- bash -s'
```

Local wrappers used `bash -o pipefail` and `tee` to retain SSH output.
The setup, branch-once, paired positive gates, marker-check, and mutation
runner wrappers exited 0. The mutation runner's exit 0 means its negative
expectations and restoration checks passed; the nested gate itself exited 1.

### `beast-setup.sh`

```bash
set -euo pipefail
export PATH=<cargo-home>/bin:$PATH
rm -rf <isolated-checkout-221>
git clone -b tracing/x86-provider-gate --single-branch https://github.com/ryanbreen/breenix.git <isolated-checkout-221>
cd <isolated-checkout-221>
ln -s <rust-fork-checkout> rust-fork
cp -a <canonical-checkout>/fonts/. fonts/
mkdir -p userspace/programs
cp <canonical-checkout>/userspace/programs/*.elf userspace/programs/ 2>/dev/null || true
if ! compgen -G 'userspace/programs/*.elf' >/dev/null; then
    find <canonical-checkout> -type f -name '*.elf' -print
    exit 1
fi
mkdir -p <isolated-checkout-222> <isolated-checkout-223>
```

### `beast-branch-once.sh`

```bash
set -euo pipefail
export PATH=<cargo-home>/bin:$PATH
cd <isolated-checkout-221>
export TMPDIR=<isolated-checkout-222>
export BREENIX_RUST_FORK_LIBRARY=<rust-fork-checkout>/library
mkdir -p "$TMPDIR/before-boot"
BREENIX_GATE_TMP="$TMPDIR/before-boot" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee <isolated-checkout-223>/before-boot.txt
serial=$(ls "$TMPDIR"/before-boot/breenix_x86_boot_tests_*/serial_user.txt | head -1)
grep -c 'TEST:process:deferred_fault_ring_overflow_injection' "$serial" || true
```

### `beast-gates.sh`

```bash
set -euo pipefail
export PATH=<cargo-home>/bin:$PATH
cd <isolated-checkout-221>
export TMPDIR=<isolated-checkout-222>
export BREENIX_RUST_FORK_LIBRARY=<rust-fork-checkout>/library
mkdir -p "$TMPDIR/main-boot" "$TMPDIR/main-prod"
BREENIX_GATE_TMP="$TMPDIR/main-boot" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee <isolated-checkout-223>/x86-boot-tests.txt
BREENIX_GATE_TMP="$TMPDIR/main-prod" bash docker/qemu/run-x86-prod-profile-boot-test.sh 2>&1 | tee <isolated-checkout-223>/x86-prod.txt
```

### `beast-marker-check.sh`

```bash
set -euo pipefail
boot_serial=$(ls <isolated-checkout-222>/main-boot/breenix_x86_boot_tests_*/serial_user.txt | head -1)
awk '
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:START]") { started++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:PASS]") { passed++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:FAIL:") { failed++ }
  END { printf "boot-tests serial: started=%d passed=%d failed=%d\n", started+0, passed+0, failed+0; exit !(started==1 && passed==1 && failed==0) }
' "$boot_serial"
prod_serial=$(ls <isolated-checkout-222>/main-prod/*/serial_user.txt 2>/dev/null | head -1)
if [ -z "$prod_serial" ]; then
  echo "no production serial_user.txt found -- report this plainly, do not guess a path"
else
  n=$(grep -c 'TEST:process:deferred_fault_ring_overflow_injection' "$prod_serial" || true)
  echo "production serial tracing-provider marker count: ${n:-0}"
  test "${n:-0}" -eq 0
fi
```

### `beast-mutation.sh`

```bash
set -euo pipefail
export PATH=<cargo-home>/bin:$PATH
cd <isolated-checkout-221>
export TMPDIR=<isolated-checkout-222>
export BREENIX_RUST_FORK_LIBRARY=<rust-fork-checkout>/library
backup=<isolated-checkout-223>/process_task.before-mutation.rs
cp kernel/src/task/process_task.rs "$backup"
trap 'cp "$backup" kernel/src/task/process_task.rs' EXIT
python3 - <<'MUTATE'
from pathlib import Path
p = Path('kernel/src/task/process_task.rs')
s = p.read_text()
fragment = '''        crate::trace_count!(
            crate::tracing::providers::teardown::DEFERRED_FAULT_RING_DROPPED
        );'''
assert s.count(fragment) == 1, 'mutation target must be unique in process_task.rs'
p.write_text(s.replace(fragment, '', 1))
MUTATE
mkdir -p "$TMPDIR/mutation"
set +e
BREENIX_GATE_TMP="$TMPDIR/mutation" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee <isolated-checkout-223>/provider-mutation.txt
mutation_exit=${PIPESTATUS[0]}
set -e
echo "mutation gate exit: $mutation_exit"
test "$mutation_exit" -ne 0
serial=$(ls "$TMPDIR"/mutation/breenix_x86_boot_tests_*/serial_user.txt | head -1)
awk '
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:START]") { started++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:PASS]") { passed++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:FAIL:") { failed++ }
  END { printf "mutation serial: started=%d passed=%d failed=%d\n", started+0, passed+0, failed+0; exit !(started==1 && passed==0 && failed==1) }
' "$serial"
cp "$backup" kernel/src/task/process_task.rs
cmp "$backup" kernel/src/task/process_task.rs
trap - EXIT
echo "mutation reverted and confirmed identical to backup"
```

## Claim-lint and source commit record

Draft checks returned exit 1 for three wording findings, then for two hits
in the paragraph describing those findings. That prose was revised to state
the observed counts and baseline limitation directly. Final checks:

```bash
python3 scripts/claim-lint.py
python3 scripts/claim-lint.py --commit-msg /tmp/slot1-commit-msg.txt
python3 scripts/claim-lint.py --files docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md
```

Observed exit codes: tree 0, source commit message 0, doc file 0. Output:

```text
claim-lint: clean (5 file(s) checked, changed hunks vs 5bfc7077af7d).
claim-lint: 164 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
claim-lint: clean commit message (../../../../../../../../tmp/slot1-commit-msg.txt).
claim-lint: clean (1 file(s) checked, whole files).
```

The source commit was created after the real gate results, replacing the WIP:

```bash
git reset --soft 5bfc7077af7dfde2e0aa81189088cc838584dea3
git add kernel/src/main.rs kernel/src/test_framework/registry.rs docker/qemu/run-x86-boot-tests.sh tests/tracing_provider_gate_structure.rs
git diff --cached --check
git commit -F /tmp/slot1-commit-msg.txt
git diff --exit-code e3aa05972d3aad6cdfa617c8522fb29e431d12ef HEAD
```

```text
[tracing/x86-provider-gate e6115a49] test(tracing): gate the x86 provider oracle in the shipping boot profile
 4 files changed, 350 insertions(+), 1 deletion(-)
```

The documentation is committed separately. Replacing a remotely published WIP
requires a non-fast-forward branch update; the final push uses an explicit
lease on `e3aa05972d3aad6cdfa617c8522fb29e431d12ef` to avoid overwriting any
unexpected remote work.

## Astra fix pass — F3 closed, 2026-09-06

Review-pass finding F3, quoted verbatim from
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/slot1/REVIEW.md`:

```text
F3 (minor, code). tests/tracing_provider_gate_structure.rs:103-186.
Claim: The 17-test structure suite pins the presence of the gate's marker
strings and its bare-assertion shape (no `if`), but does not pin the awk
predicate's own comparison operators, so a regression in the comparison
logic itself would not be caught by this suite even though the real script
currently behaves correctly.
Evidence: in-memory mutating the extracted awk block's `started == 1` to
`started >= 1` and replaying all 5 existing run_oracle fixtures
(baseline/positive/duplicate-pass/missing-start/fail-alongside-pass) against
both the original and mutated predicate gave identical (correct) results
under the mutation — none of the fixtures exercises a duplicate-START
serial. Separately confirmed the actual current (unmutated) script correctly
rejects a duplicate-START serial today (exit 1). No live defect: only a
coverage gap in the test suite.
```

The fifth `assert!` in `assert_gate` pins the exact predicate substring.
The two new `should_panic` tests exercise the started and failed operator
mutations in memory. Actual diff:

```bash
git diff -- tests/tracing_provider_gate_structure.rs
```

```diff
diff --git a/tests/tracing_provider_gate_structure.rs b/tests/tracing_provider_gate_structure.rs
index 3a9190dc..27853cb6 100644
--- a/tests/tracing_provider_gate_structure.rs
+++ b/tests/tracing_provider_gate_structure.rs
@@ -126,6 +126,10 @@ fn assert_gate(source: &str) {
                 .ends_with("' \"$OUTPUT_DIR/serial_user.txt\""),
         "gate assertions must be bare"
     );
+    assert!(
+        code.contains("END { exit !(started == 1 && passed == 1 && failed == 0) }"),
+        "gate predicate must require exactly one start, one pass, zero fail"
+    );
 }

 #[test]
@@ -227,6 +231,30 @@ fn wrapping_gate_in_if_would_be_caught() {
     assert_gate(&source.replace(block, &format!("if true; then\n{block}\nfi\n")));
 }

+#[test]
+#[should_panic(
+    expected = "gate predicate must require exactly one start, one pass, zero fail"
+)]
+fn weakening_started_equality_would_be_caught() {
+    let source = read(SCRIPT);
+    assert_gate(&source);
+    let block = gate_block(&source);
+    let mutated = block.replace("started == 1", "started >= 1");
+    assert_gate(&source.replace(block, &mutated));
+}
+
+#[test]
+#[should_panic(
+    expected = "gate predicate must require exactly one start, one pass, zero fail"
+)]
+fn weakening_failed_equality_would_be_caught() {
+    let source = read(SCRIPT);
+    assert_gate(&source);
+    let block = gate_block(&source);
+    let mutated = block.replace("failed == 0", "failed <= 1");
+    assert_gate(&source.replace(block, &mutated));
+}
+
 fn unique_temp_dir(tag: &str) -> PathBuf {
     let path = std::env::temp_dir().join(format!(
         "tracing-provider-{tag}-{}-{}",
```

The recorded baseline above is 17 tests; this pass runs 19 (17 → 19).

```bash
bash scripts/run-structure-tests.sh tracing_provider_gate_structure
```

```text
== compiling tracing_provider_gate_structure ==
== running tracing_provider_gate_structure  ==

running 19 tests
test gate_requires_provider_markers_from_com1_with_bare_assertions ... ok
test removing_provider_gate_block_would_be_caught - should panic ... ok
test weakening_started_equality_would_be_caught - should panic ... ok
test printing_pass_unconditionally_would_be_caught - should panic ... ok
test wrapper_uses_shipping_cfg_and_result_dependent_markers ... ok
test weakening_failed_equality_would_be_caught - should panic ... ok
test wrapping_gate_in_if_would_be_caught - should panic ... ok
test removing_direct_call_would_be_caught - should panic ... ok
test direct_call_is_after_ring_span_before_next_disable_with_shipping_cfg ... ok
test registry_dispatches_provider_only_on_aarch64 ... ok
test moving_direct_call_past_nearest_disable_would_be_caught - should panic ... ok
test restoring_arch_any_duplicate_registration_would_be_caught - should panic ... ok
test gating_direct_call_on_staged_executor_would_be_caught - should panic ... ok
test gating_wrapper_on_staged_executor_would_be_caught - should panic ... ok
test exactly_one_start_and_pass_is_accepted ... ok
test archived_baseline_without_provider_markers_is_rejected ... ok
test fail_alongside_pass_is_rejected ... ok
test duplicate_pass_is_rejected ... ok
test missing_start_is_rejected ... ok

test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

```

Full discovery command (exit 0):

```bash
(cd tests && find . -maxdepth 1 -type f -name '*_structure.rs' -print | sed 's|^\./||; s/\.rs$//' | sort) \
  | while read -r stem; do
      echo "=== $stem ===";
      bash scripts/run-structure-tests.sh "$stem" || echo "RED: $stem";
    done
```

Full stdout and stderr are saved at
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/slot1/evidence/fixpass-all-structure.txt`.
The following are the actual suite headers and result lines, in output order:

```text
=== aarch64_testing_profile_structure ===
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== block_request_lifetime_structure ===
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
=== capture_bxcap_schema_structure ===
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== capture_path_lock_free_structure ===
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
=== context_restore_structure ===
test result: ok. 97 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 55.59s
=== coreproof_component_h_structure ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== coreproof_coverage_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== coreproof_mutation_register_structure ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
=== coreproof_sites_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== critical_path_logging_census_structure ===
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
=== degenerate_transfer_fd_validation_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== dispatch_fact_census_structure ===
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== dispatch_path_lock_free_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== dispatch_strand_census_structure ===
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== dma_and_log_sink_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== entry_point_df_structure ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
=== exec_lock_order_structure ===
test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
=== exit_tally_structure ===
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
=== ext2_disk_size_structure ===
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== ext2_lock_structure ===
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== fcntl_pm_contention_gate_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== fork_lock_order_structure ===
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== gate_boot_facts_pipefail_structure ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
=== gate_boot_facts_structure ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== gate_capture_drain_structure ===
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.51s
=== gate_structure_preflight_wiring_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== green_program_envelope_structure ===
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== lockup_capture_guard_structure ===
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s
=== loopback_pump_structure ===
test result: ok. 104 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.28s
=== masked_binary_load_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== mmap_floor_structure ===
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
=== net_lock_structure ===
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.20s
=== parallels_kill_by_name_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== poll_tcp_gate_wiring_structure ===
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== preempt_bracket_structure ===
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== qemu_host_lock_structure ===
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== qemu_kill_by_name_structure ===
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
=== ring_span_report_site_structure ===
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== serial_line_atomicity_structure ===
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
=== signal_eintr_predicate_structure ===
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== strand_handoff_structure ===
test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.27s
=== syscall_return_register_structure ===
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
=== teardown_structure ===
test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.57s
=== terminal_edge_capture_structure ===
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
=== timer_wake_dispatch_structure ===
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== trace_ring_depth_structure ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== tracing_provider_gate_structure ===
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== ttbr0_shadow_reconciliation_structure ===
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 13.71s
=== tty_irq_fg_structure ===
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.80s
=== tty_irq_pm_structure ===
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.79s
=== tty_oracle_structure ===
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== x86_smp_enum_structure ===
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

Parsed tally:

```text
52 discovered suites passed; 0 RED; 792 tests passed; 0 tests failed
```

Compared with the recorded baseline of 52 suites and 790 tests, the two
new tests account for the increase to 792; the suite count stays 52.

Scope evidence before this append:

```bash
git status --short --branch
git diff --stat
```

```text
## tracing/x86-provider-gate...origin/tracing/x86-provider-gate
 M tests/tracing_provider_gate_structure.rs
 tests/tracing_provider_gate_structure.rs | 28 ++++++++++++++++++++++++++++
 1 file changed, 28 insertions(+)
```

No files under `kernel/src/`, `kernel/Cargo.toml`, or
`docker/qemu/run-x86-boot-tests.sh` changed in this pass. Therefore the
aarch64 kernel was not rebuilt, the aarch64 strict boot gate was not re-run,
and the beast x86 gates were not re-run. This closes only the F3 test-suite
coverage gap; it does not fix a kernel or script defect.

## Not claimed

- This addendum does not claim a live defect existed in `docker/qemu/run-x86-boot-tests.sh`; F3 reports a correctly behaving script and a coverage gap.
- This addendum does not claim to close F1, F2, F4, or F5; those are out of scope.
- This addendum does not claim any kernel or aarch64/x86 gate was re-run.

### Claim-lint invocation record for this addendum

The first tree check returned exit 1 on the final clause in the
Not claimed list. The sentence now attributes the correctly behaving script
and coverage gap to F3. The initial output was:

```text
docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md:833: [universal-claim] - This addendum does not claim a live defect existed in `docker/qemu/run-x86-boot-tests.sh`; none did.
    -> unquantified absolute ('none') with no N-of-M count, resolving evidence-log citation, or claim-lint:ok in this paragraph

claim-lint: 1 finding(s) across 5 file(s) [changed hunks vs 5bfc7077af7d]. Discharge a legitimate claim with a same-paragraph `claim-lint:ok: <citation>` annotation naming an N-of-M count, a resolving path, an issue, or a review. See docs/planning/green-program/claim-linting.md.
claim-lint: 164 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```

```text
claim-lint: python3 scripts/claim-lint.py -> exit 1 (initial wording)
claim-lint: python3 scripts/claim-lint.py -> exit 0 (revised wording)
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/slot1-fixpass-commit-msg.txt -> exit 0
```

Final invocation output:

```text
claim-lint: clean (5 file(s) checked, changed hunks vs 5bfc7077af7d).
claim-lint: 164 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
claim-lint: clean commit message (../../../../../../../../tmp/slot1-fixpass-commit-msg.txt).
```

The evidence-record prose also triggered a tree check (exit 1), repeated
once to capture its output; both invocations used `python3 scripts/claim-lint.py`.
That prose was revised to identify the clause without repeating its wording.

```text
docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md:839: [universal-claim] The first tree check returned exit 1 on the phrase "none did" in the Not claimed list. The sentence now attributes the correctly behaving script and coverage gap to F3. The initial output was:
    -> unquantified absolute ('none') with no N-of-M count, resolving evidence-log citation, or claim-lint:ok in this paragraph

claim-lint: 1 finding(s) across 5 file(s) [changed hunks vs 5bfc7077af7d]. Discharge a legitimate claim with a same-paragraph `claim-lint:ok: <citation>` annotation naming an N-of-M count, a resolving path, an issue, or a review. See docs/planning/green-program/claim-linting.md.
claim-lint: 164 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```

`git diff --check` initially returned exit 2 for the two blank context lines
in the quoted diff (lines 611 and 616, "trailing whitespace"). Their single
context spaces were removed from this Markdown quotation.

## Prose fix pass — F1, F4, F5 closed, 2026-09-06

This pass closes review-pass findings F1, F4, and F5 (each quoted verbatim
in the task that produced this pass; F2 and F3 are unaffected — F3 was
already closed by the astra fix pass above, F2 remains open and out of
scope here).

- **F1 (major, prose)**: the PROPOSED (not-applied) atlas replacement text
  claimed the post-merge issue sweep finds no open tracing-layer defects.
  Issue #855, filed 2026-09-06T02:44:13Z independent of this branch and
  confirmed OPEN via `gh issue view 855`, is a genuine open in-layer defect
  in `kernel/src/tracing/core.rs` + `buffer.rs` (row 20's own path per
  `atlas-data.json`), unrelated to #533/#680/#681 and unaddressed by this
  PR. Closed by adding a "Review in-layer ruling" section recording the
  review pass's ruling verbatim, and by rewriting the PROPOSED atlas text
  to carry #855 forward instead of claiming a clean sweep.
- **F4 (minor, prose)**: the "Not claimed" section's "#567/#680/#681
  scheduler repair" bullet mischaracterized #681, whose decisive logic is
  `sys_exit`'s premature terminal-verdict publication in
  `kernel/src/syscall/handlers.rs` (the syscall exit-verdict path), not the
  scheduler. Closed by splitting the bullet into a scheduler item
  (#567/#680) and a separate syscall-exit-verdict-path item (#681).
- **F5 (minor, prose)**: the residual evidentiary gap — the new awk
  assertion has not yet been exercised inside one unbroken full
  `run-x86-boot-tests.sh` mutation run because #891's softirq panic aborts
  the script first — was already honestly disclosed in the round doc, but
  carried no explicit forward action for a future round. Closed by adding
  a "Residual gap, recorded for a future round" note in the "Runtime
  mutation" section naming the concrete follow-up (re-run the mutation gate
  once #891 no longer intervenes on that boot).

No file under `kernel/src/`, `kernel/Cargo.toml`,
`docker/qemu/run-x86-boot-tests.sh`, or `tests/` changed in this pass — the
commit for this pass touches only this round doc (a `git diff --name-only`
before committing shows a single path,
`docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md`; the
exact insertion count is not quoted here because this section's own text
contributes to it).

Because no kernel or script source changed, the aarch64 kernel was not
rebuilt and neither the aarch64 strict boot gate nor the beast x86 gate was
re-run for this specific pass. Per the task's own instructions, this
document's separate "R16" landing step (merge main, structure suites, one
boot per arch, PR, merge) follows this pass and is recorded in the
"Landing" section below.

### Claim-lint invocation record for this pass

```text
claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md -> exit 1 (initial wording: "never"/"zero" universal-claim hits on the new F5 note and the corrected atlas-prose paragraph)
claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/tracing/X86-PROVIDER-GATE-2026-09-06.md -> exit 0 (revised wording)
claim-lint: python3 scripts/claim-lint.py -> exit 0 (5 file(s) checked, changed hunks vs 5bfc7077af7d)
```

## Landing

`git fetch origin && git merge origin/main --no-edit` merged cleanly as
commit `b0019cfb7a65206537862d10fd463a0b860ec8b5` (origin/main was at
`5f68fedd3b3056fce3eec4be2792e8f54a310a60`, 31 commits ahead of this
branch's merge-base `5bfc7077af7dfde2e0aa81189088cc838584dea3`). No conflict
of any kind occurred — `git status --short` was empty immediately after the
merge — so the "STOP on any kernel/ conflict" condition did not apply.
`git diff --stat 5bfc7077..HEAD -- kernel/` shows main's own changes to
`kernel/src/main.rs`, `kernel/src/main_aarch64.rs`,
`kernel/src/task/scheduler.rs`, and `kernel/src/test_framework/registry.rs`
merged in alongside this branch's own edits with no textual conflict.

`bash scripts/run-structure-tests.sh` (default: `teardown_structure`, whole
file) exited 0: `test result: ok. 92 passed; 0 failed; 0 ignored; 0
measured; 0 filtered out`.

`bash docker/qemu/run-aarch64-boot-test-strict.sh 1`: `[GATE_PREFLIGHT:
structure_suites=53/53:critical_path_lines=260:pinned=120]`, then
`[OK] Boot 1: SUCCESS`, `PASS: 1/1 boots succeeded`.

`bash docker/qemu/run-x86-boot-tests.sh 1` on beast, in
`<isolated-checkout-221>`: **first attempt's structure-suite preflight came
back `GATE_PREFLIGHT: FAIL (2 of 53 ... red: coreproof_component_h_structure
coreproof_coverage_structure)`.** Both suites pass cleanly in this worktree
(`5/5` and `4/4` respectively, same commit `b0019cfb`), and neither suite's
own log file existed under the shared `/tmp/breenix_gate_structure_preflight`
afterward, even though 51 other suites' logs did. At the time, another lane
(clone `<isolated-checkout-132>`, confirmed via `ps`) was concurrently running the
same gate script on the same beast container, and both
`docker/qemu/lib/gate-structure-preflight.sh` (its `rm -rf "$log_dir"` +
shared log directory) and `scripts/run-structure-tests.sh` (its shared
`${TMPDIR:-/tmp}/breenix-structure-tests/<stem>` compile-output path) default
to the bare, non-run-scoped `/tmp` unless the caller sets `BREENIX_GATE_TMP`
and `TMPDIR`. This reads as a cross-lane race on those two shared paths, not
a source defect newly introduced by this branch or by origin/main's merged
commits — it is a preexisting gap in this gate's own temp-path scoping, not
touched by this branch's diff, so it is not fixed here; a session
encountering it again should isolate `BREENIX_GATE_TMP`/`TMPDIR` per run, as
done below. **Second attempt, with `TMPDIR=<host-artifact-dir-224>
BREENIX_GATE_TMP=<host-artifact-dir-224>`** (a run-scoped directory, avoiding
the shared-`/tmp` race): `[GATE_PREFLIGHT:structure_suites=53/53:
critical_path_lines=260:pinned=120]`, kernel + userspace + ext2 build
succeeded, queued on the shared `x86-qemu.lock` behind other lanes'
concurrent boots (normal per this task's own instructions), then booted and
scored `x86 frame-custody gate run 1: PASS`. The provider gate's own lines
in `<host-artifact-dir-224>/breenix_x86_boot_tests_1/serial_user.txt` (copied
locally to
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/slot1/landing-evidence/x86-serial-user.txt`,
lines 119-120):

```text
[TEST:process:deferred_fault_ring_overflow_injection:START]
[TEST:process:deferred_fault_ring_overflow_injection:PASS]
```

No leftover `qemu-system-x86_64` process for `breenix-slot1` remained after
the run (`ps aux | grep qemu-system-x86_64 | grep breenix-slot1` came back
empty). Full stdout is saved locally at
`.../scratchpad/slot1/landing-evidence/x86-gate-stdout.log` and the full
serial capture at `.../landing-evidence/x86-serial-user.txt`.

### Claim-lint invocation record for landing

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0 (5 file(s) checked, changed hunks vs 5bfc7077af7d)
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/slot1-landing-commit-msg.txt -> exit 0
```
