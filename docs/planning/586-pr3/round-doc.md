# #586 PR 3 — guest execution budget extensions

Branch: `net/586-pr3-guest-execution-budget`.
Base: `c12ff1409b7e1e1f276ca4c6d5cd97f02973b69b` (PR 2 merge).
Status: implementation and host verification; strict boot and landing pending
resolution of the pinned-toolchain warning tracked in
[issue 945](https://github.com/ryanbreen/breenix/issues/945).

## Policy and integration

The final CAP in the fix-pass request takes precedence over its earlier
counter/tick-ratio proposal and the inherited draft's four 200 ms extensions.
`registry.rs::guest_budget::grant_extension()` accepts context delta, elapsed
counter milliseconds, extensions already spent in milliseconds, and an
`ExtensionPolicy`. Below 30 switches or below 75% of allocated budget it returns
0; from 75% through 90% it returns 50; above 90% it returns 100. Grants are
clipped to remaining capacity. Policy configuration may lower the extension
cap but cannot raise it above 200 ms. The denominator is the initial budget
plus extensions already granted. u128 percentage comparisons avoid overflow.

The kernel initially sleeps for 200 ms. A pending reader observed Ready or
Running may receive a grant, sleep for that grant, and be measured again.
Context delta is measured afresh for each sleep; counter elapsed time is summed
across sleeps. The extension cap is 200 ms and the maximum allocated sleep
budget is 400 ms. This bounds requested sleep, not host scheduling delay: a
host can deschedule the guest beyond a requested wake deadline. Because the
first production observation is after the initial sleep, 100 ms is the usual
eligible production grant; 50 ms behavior is covered independently at the
percentage boundaries.

`extensions` now denotes milliseconds granted, not the inherited draft's
window count. The budget marker prints `extension_cap_ms=200` and
`extension_bound_ms=400`. The executor's Fail marker prints those policy values
for the two loopback receive-wake tests, including setup failures. The final
clock fields describe the last measured window. A remembered starved window
retains `verdict=starved` after an extension; the separate test result still
requires a nonzero reader wake stamp and exactly three bytes received.
No scheduler, syscall, or interrupt implementation files were edited.

## R220 evidence

| Condition | Implementation and evidence |
| --- | --- |
| (a) Grant predicate | Pure production policy in `kernel/src/test_framework/registry.rs`. The structure runner extracts and compiles that module verbatim with `rustc --test -Dwarnings`; boundary and cap assertions are in `tests/fixtures/guest_budget.rs`. |
| (b) Fail bound | `kernel/src/test_framework/executor.rs` emits policy-derived cap and bound in the loopback Fail message. The wiring test checks this source and the registry emitter. This is source verification, not a captured failing boot. |
| (c) Mutation | `always_extend` in the host fixture forces 100 ms. `loopback_guest_budget_always_extend_mutation` and its `all_extend` alias execute four fixture tests with that flag active, require the child run to fail, and check boundary/cap failures explicitly. |
| (d) Recovery | The deterministic fixture uses the production starvation classifier for tick=40, counter=200, ctx=30. With no extensions, the runnable reader's scheduled completion at 240 ms is missed; with policy enabled it receives three bytes within the extra 100 ms. Output records `before verdict=starved extensions=0 FAIL` and `after verdict=starved extensions=100 PASS`. Blocked readers, wrong bytes and permanent non-completion remain failures. This is a scheduling fixture, not live boot evidence. |
| (e) Round doc | This document records implementation, commands, evidence scope, and remaining verification. |

## Commands and results

Commands run with `TMPDIR="$PWD/.tmp"` and
`BREENIX_GATE_TMP="$PWD/.gate-tmp"`. Structure tests use
`scripts/run-structure-tests.sh`; no `cargo test` was used.

- `bash scripts/run-structure-tests.sh`: 103 passed, 0 failed;
  [default suite](serials/structure-default.txt).
- `bash scripts/run-structure-tests.sh loopback_pump_structure`: 128 passed,
  0 failed; [loopback suite](serials/structure-loopback.txt).
- `bash scripts/run-structure-tests.sh loopback_pump_structure loopback_guest_budget`:
  4 parent tests passed. The unmodified child fixture passed 4/4; each forced
  extension child failed 4/4 as expected by the mutation oracle. These are
  executed tests, not a zero-test filtered result;
  [policy and mutation transcript](serials/policy-mutations.txt).
- Structure preflight: 64/64 suites passed on the final source; output is
  [full-suite transcript](serials/structure-suites.txt).
- `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`:
  completed with the existing pinned `core` future-incompatibility warning;
  [build transcript](serials/build.txt). This does not satisfy the repository's
  zero-warning requirement. An explicit exception was requested and is pending.
- `bash docker/qemu/run-aarch64-boot-test-strict.sh 1`: pending the warning
  decision. No strict boot PASS is claimed.
- Claim-lint: tree and prepared commit message pass; see
  [tree output](serials/claim-lint.txt) and
  [message output](serials/commit-message-lint.txt). No commit has been made.

## Scope and deviations

The fix-pass CAP is implemented in place of the earlier ratio-based grant
proposal. Starvation classification still uses the PR 2 ratio and context
thresholds; it is independent of grant eligibility. A window with fewer than
30 switches is ineligible even if classified starved, as required by CAP.

The requested recovery and mutation evidence is a deterministic host fixture.
It does not demonstrate a real host-starved QEMU recovery or a kernel built
with a forced-extension mutation. The inherited optional CPU-contention script
`docker/qemu/run-aarch64-starved-loop.sh` has only received shell syntax
validation in this pass. Historical draft logs under
`docs/planning/green-program/network/serials/586-pr3/` describe a different
implementation and are not current acceptance evidence.

Issue 586 remains open. V-1 is not claimed verified while the strict
boot, warning disposition, commit and push remain pending.
