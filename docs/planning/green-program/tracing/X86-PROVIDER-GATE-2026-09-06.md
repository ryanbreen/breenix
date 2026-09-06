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
`/Users/wrb/fun/code/breenix/target/ext2-aarch64.img` and the worktree's
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
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (/Users/wrb/.rustup/toolchains/nightly-2025-06-24-aarch64-apple-darwin/lib/rustlib/src/rust/library/core)
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
`ssh beast 'sudo -n incus exec breenix-x86 -- bash -s'`, feeding the exact
runner scripts reproduced in the appendix. The lane used `/root/breenix-slot1`,
`TMPDIR=/root/breenix-slot1-tmp`, and
`BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork-real/library`.
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
/root/breenix-slot1-tmp/before-boot/breenix_x86_boot_tests_1/serial_user.txt
/root/breenix-slot1-tmp/main-boot/breenix_x86_boot_tests_1/serial_user.txt
/root/breenix-slot1-tmp/main-prod/breenix_x86_prod_profile/serial_user.txt
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

`ssh beast 'sudo -n incus exec breenix-x86 -- find /root/breenix -type f -name busybox.elf -print'`
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

Restoration was checked in the runner with `cmp`, then independently with:

```bash
cd /root/breenix-slot1
cmp /root/breenix-slot1-evidence/process_task.before-mutation.rs kernel/src/task/process_task.rs
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
scp 'beast:/root/breenix-slot1-evidence/*' ../evidence/
```

It exited 1 (`scp-attempt.txt`):

```text
scp: remote readdir("/root/breenix-slot1-evidence/"): Permission denied
scp: /root/breenix-slot1-evidence/*: No such file or directory
```

The artifacts are inside `breenix-x86`, not the SSH user's host filesystem.
Instead, these commands copied the evidence and raw serials through Incus;
both SSH/tar transfers and both local extractions exited 0:

```bash
ssh beast 'sudo -n incus exec breenix-x86 -- tar -C /root/breenix-slot1-evidence -cf - .' > ../evidence/beast-evidence.tar
tar -xf ../evidence/beast-evidence.tar -C ../evidence
ssh beast 'sudo -n incus exec breenix-x86 -- tar -C /root/breenix-slot1-tmp -cf - before-boot/breenix_x86_boot_tests_1/serial_user.txt before-boot/breenix_x86_boot_tests_1/serial_kernel.txt main-boot/breenix_x86_boot_tests_1/serial_user.txt main-boot/breenix_x86_boot_tests_1/serial_kernel.txt mutation/breenix_x86_boot_tests_1/serial_user.txt mutation/breenix_x86_boot_tests_1/serial_kernel.txt main-prod' > ../evidence/beast-serials.tar
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

## Proposed atlas replacement prose (NOT applied)

The following is verbatim proposed text, pending the review question above.
It is not a claim this document makes about the current atlas or an issue sweep:

> Tracing × x86 — complete/HIGH (PROPOSED): "HIGH: the shipping x86 boot_tests profile directly executes the deferred-fault-ring overflow provider test, its gate requires the actual PASS and rejects missing or failed results, the sampling/report-site mutations and production regression gate pass, and the post-merge issue sweep finds no open tracing-layer defects; full staged-registry execution remains tracked in the boot/scheduler rows."
>
> Tracing × blended — complete/HIGH (PROPOSED): "HIGH follows the existing aarch64 tracing evidence and the newly gated x86 provider execution in the shipping profile, with no open in-layer issues in the post-merge sweep; neither seconds of retained history nor full staged-registry execution is asserted."

This document does not itself declare either cell HIGH. No atlas file in
this repository was edited by this task. No atlas file exists here; the atlas
lives outside this repository. No post-merge issue sweep was performed.

## Not claimed

- Full x86 registry execution.
- The #567/#680/#681 scheduler repair.
- A live process fault: the injection uses a private fixture with synthetic TIDs, not a real fault path.
- RELIABILITY.
- Any atlas colour change; that is left to the review question above.
- That the full mutation gate reached the new assertion; it stopped earlier on the softirq panic.

## Runner command appendix

The following scripts were supplied on stdin to:

```bash
ssh beast 'sudo -n incus exec breenix-x86 -- bash -s'
```

Local wrappers used `bash -o pipefail` and `tee` to retain SSH output.
The setup, branch-once, paired positive gates, marker-check, and mutation
runner wrappers exited 0. The mutation runner's exit 0 means its negative
expectations and restoration checks passed; the nested gate itself exited 1.

### `beast-setup.sh`

```bash
set -euo pipefail
export PATH=/root/.cargo/bin:$PATH
rm -rf /root/breenix-slot1
git clone -b tracing/x86-provider-gate --single-branch https://github.com/ryanbreen/breenix.git /root/breenix-slot1
cd /root/breenix-slot1
ln -s /root/breenix/rust-fork-real rust-fork
cp -a /root/breenix/fonts/. fonts/
mkdir -p userspace/programs
cp /root/breenix/userspace/programs/*.elf userspace/programs/ 2>/dev/null || true
if ! compgen -G 'userspace/programs/*.elf' >/dev/null; then
    find /root/breenix -type f -name '*.elf' -print
    exit 1
fi
mkdir -p /root/breenix-slot1-tmp /root/breenix-slot1-evidence
```

### `beast-branch-once.sh`

```bash
set -euo pipefail
export PATH=/root/.cargo/bin:$PATH
cd /root/breenix-slot1
export TMPDIR=/root/breenix-slot1-tmp
export BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork-real/library
mkdir -p "$TMPDIR/before-boot"
BREENIX_GATE_TMP="$TMPDIR/before-boot" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee /root/breenix-slot1-evidence/before-boot.txt
serial=$(ls "$TMPDIR"/before-boot/breenix_x86_boot_tests_*/serial_user.txt | head -1)
grep -c 'TEST:process:deferred_fault_ring_overflow_injection' "$serial" || true
```

### `beast-gates.sh`

```bash
set -euo pipefail
export PATH=/root/.cargo/bin:$PATH
cd /root/breenix-slot1
export TMPDIR=/root/breenix-slot1-tmp
export BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork-real/library
mkdir -p "$TMPDIR/main-boot" "$TMPDIR/main-prod"
BREENIX_GATE_TMP="$TMPDIR/main-boot" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee /root/breenix-slot1-evidence/x86-boot-tests.txt
BREENIX_GATE_TMP="$TMPDIR/main-prod" bash docker/qemu/run-x86-prod-profile-boot-test.sh 2>&1 | tee /root/breenix-slot1-evidence/x86-prod.txt
```

### `beast-marker-check.sh`

```bash
set -euo pipefail
boot_serial=$(ls /root/breenix-slot1-tmp/main-boot/breenix_x86_boot_tests_*/serial_user.txt | head -1)
awk '
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:START]") { started++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:PASS]") { passed++ }
  index($0,"[TEST:process:deferred_fault_ring_overflow_injection:FAIL:") { failed++ }
  END { printf "boot-tests serial: started=%d passed=%d failed=%d\n", started+0, passed+0, failed+0; exit !(started==1 && passed==1 && failed==0) }
' "$boot_serial"
prod_serial=$(ls /root/breenix-slot1-tmp/main-prod/*/serial_user.txt 2>/dev/null | head -1)
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
export PATH=/root/.cargo/bin:$PATH
cd /root/breenix-slot1
export TMPDIR=/root/breenix-slot1-tmp
export BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork-real/library
backup=/root/breenix-slot1-evidence/process_task.before-mutation.rs
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
BREENIX_GATE_TMP="$TMPDIR/mutation" bash docker/qemu/run-x86-boot-tests.sh 1 2>&1 | tee /root/breenix-slot1-evidence/provider-mutation.txt
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
