# A.3 u23: preserve the missing command identity

Input: `a46f94c7`, continuing `arm-catchup/a-3`. Coordinator ARM ledger line
239 governs this unit. The full plan, its A.3 step, the specified U rulings,
ledger lines 167, 182, 185, 198, 208, 216, 221, 233–240, the mechanism report
and u20 observation report were read before implementation.

## Decision

[u22's result](a3-mechanism.md#a3-u22-sample-join--coordinator-line-239-retained-capture-only)
is **CONFIRMED BUT CONTEXT UNMAPPED**, at aggregate replacement-sample
resolution. In particular, window 65's four preceding submissions lack their
exact originating guest stamps. Later context usage does not recover those
origins. That missing identity blocks choosing a client-specific repair.
This unit implements line 239's preservation alternative. It does not claim
to fix GL/BQL contention or attribute the five failed cases to that mechanism.

## Preservation and boundaries

The active GPU journal keeps its existing 2048-slot recent ring. A separate
case archive keeps the **first 32768** command/doorbell rows in a selected
case, without replacing those rows on saturation. The archive uses the same
versioned atomic slot implementation and original global sequence numbers.
It retains PID/TID, context, command type, header/payload sizes, queue/wire
fences, submission and completion boundaries, and doorbell entry/return.

The existing root-only, test-control-gated `WM_STALL budget` path now delivers
boundaries to this archive on ARM64. `parent-fork` arms it just before the
existing CASE START output and fork. `parent-outcome` freezes at the parent
execution-end boundary before export. For an outer watchdog, the existing
`watchdog-before-kill` stamp freezes before the pre-kill dump, so the child's
remaining GPU traffic and the dump itself cannot evict the evidence. Only the
recorded parent TID can freeze its window. Child diagnostic/report stamps do
not freeze it.

Selected cases are the five recovery cases plus stacking/click-raises,
stacking/all-minimized, focus/pair-click and focus/keys-to-focused. The next
selected CASE START reuses the archive. Sequence bounds exclude older rows
and late completions from a prior case. A completion after the frozen end
remains pending (`fence_return_ns=0` or `return_ns=0`) in that case's snapshot.

The archive is allocated once at the first selected parent boundary. Its
32768 slots use 3407872 bytes (12 u64 words plus one u64 version per slot),
from the existing fixed kernel heap; it adds no corresponding static image
reservation. GPU send/reap and doorbell recording use numeric atomic slots.
The first-boundary allocation occurs before the case child starts. The
instrumentation's runtime cost still needs measurement.

Suite export defers archived rows until freeze and emits them once for that
window. The recent ring continues to export as before. This avoids reprinting
the larger frozen archive on later cases' diagnostic reads. The in-memory
snapshot remains available until the next selected start. Stored rows retain
the existing `gpu-core-command`/`gpu-core-doorbell` labels and append
`case=<name> window_begin_ns=<ns>`, compatible with the existing analyzer's
field parsing. The new header is:

```text
VALUE wm-gpu-core-case-window case=<name> owner_tid=<tid> begin_ns=<ns> end_ns=<ns> capacity=<n> retained=<n> overflow=<n> lost=<n> missing=<n> first_seq=<n> last_seq=<n>
```

Overflow reports commands beyond capacity. Lost counts failed publication
attempts; missing counts unobserved slots within the bounded sequence range.
These counters expose incomplete evidence; they do not turn it into complete
causal attribution. The archive is a bounded prefix of the case, not a promise
of unlimited capture history. Frame-floor remains outside this unit's selected
preservation cases.

## Check and reach

Check-first commit `424ee615` adds `tests/a3_gpu_case_preserve_test.py`.
After correcting an unused fixture import, its production-slice Rust check
compiles with `-Dwarnings` and fails on the input journal with **evicted
context creator**. The journal's real modulo wrap has discarded the creator
and shader submit before export. Boundary calls are inert only for that old
implementation, which lacks the boundary API; submission, eviction and export
remain its production code. The second check fails on the missing ARM dispatch.

After implementation, both checks pass. The retention check also covers
pending completion at watchdog freeze, parent identity, reuse, visible
saturation, and export once after freeze. After adding the export behavior,
the affected retention and recent-ring checks were rerun; after the final
header/identity adjustment, only the retention check was rerun. The unchanged
boundary integration and budget-writer results stand. Two existing host
fixtures gained bindings for `Once` and the ARM boundary hook; their existing
request, doorbell and whole-write assertions remain in place.

| Changed file | Does the R240 run the change? |
| --- | --- |
| `kernel/src/window/gpu_core_trace.rs` | No: module is ARM64-only. |
| `kernel/src/fs/devfs/mod.rs` | The file is shared; the added call is under `cfg(target_arch = "aarch64")` and excluded on x86. |
| `tests/a3_gpu_case_preserve_test.py` | Host check only. |
| `tests/a3_gpu_lock_attribution_test.py` | Host fixture binding and an additional export assertion. |
| `tests/a3_irq_observe_test.py` | Host fixture binding only. |
| `docs/planning/arm64-catchup/a3-u23-preserve.md` | Documentation only. |

No suite assertion, deadline, threshold, renderer check or launcher changes.
No Tier-1 or Tier-2 file changes. Existing issues #2872 and #2890 remain open.

## Verification and handoff

Host results:

- New preservation/integration check: 2/2 pass; final affected retention
  invocation: 1/1 pass (`/tmp/a3-u23-final-retention.log`).
- Existing active-request/recent-ring check: 1/1 pass
  (`/tmp/a3-u23-existing-ring.log`).
- Existing whole-write budget transport check: 1/1 pass
  (`/tmp/a3-u23-existing-budget.log`).

Release builds pass: production ARM64 kernel in 1m12s and scoped x86
`qemu-uefi`/kernel with `testing,external_test_bins` in 1m40s. The x86 build
has no warnings. ARM has only the existing pinned-core future-compatibility
advisory documented in u20/u21, with no new warnings. The ARM FP/SIMD guard
passes: 0 forbidden and 32 allowlisted instructions. Logs are
`/tmp/a3-u23-{arm,x86}-build.log` and `/tmp/a3-u23-arm-fp-guard.log`.
Both architectures use `CARGO_BUILD_JOBS=4` and `taskpolicy -b`. An earlier
ARM build passed before the export adjustment; the final code state was built
again because that adjustment changed code. Userspace is unchanged.

Claim-lint invocations, in order:

- `python3 scripts/claim-lint.py --files tests/a3_gpu_case_preserve_test.py`
  → exit 0 before the check commit.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u23-check-msg`
  → exit 0 before the check commit.
- `python3 scripts/claim-lint.py --base 424ee615`
  → exit 1 for an absolute comment, then exit 0 after correcting its wording;
  the implementation/export invocation also exits 0. The first two final-report
  invocations against `a46f94c7` exit 1 for a universal quantifier in the decision
  paragraph and then this invocation log, corrected in this report. Subsequent
  final-report invocations exit 0 (six files checked).
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u23-implementation-msg`
  → exit 0 before the implementation commit.

`git diff --check` exits 0. Commit/push identities and the last report-lint
invocation are recorded in the ARM ledger. No tests are repeated after push.

The next authorized boot is a **measurement**, not acceptance: preserve and
join the exact guest origins of the replacement-window commands to the host
trace and samples, distinguishing GL/BQL waits from other gaps. Verify the
frozen window's coverage and loss counters before attributing a command.
The earlier host sample coverage limits for failed click waits and frame-floor
remain as u22 reported; this guest preservation change does not supply missing
host stacks. This unit performs no boot, deployment, PR operation or merge.
