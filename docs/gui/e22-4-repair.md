# E22 22.4 acceptance repair

The retained head boot `20261108T122858Z-metal` at `10351004` did not pass
22.4: 77/78 selected cases passed. `sessions/start-desktop` exhausted its
300 ms deadline; its observed first composite was 301.746 ms after connect. That observation is
after the deadline and can be affected by collection.
Memory passed. There was no GR refusal. `20261108T120415Z-metal` ran at
`db222c5e`; it is not evidence for the later runtime tree.

The failed head capture attributes 76.26 ms to connection/setup, 12.3 ms to
launch, 81.42 ms to overlapped font/target preparation, 10.58 ms to background,
4.92 ms to initial chrome, 9.84 ms to fill checks, 7.69 ms to another presentation,
61.8 ms to full-frame validation, 22.57 ms to initial client chrome and about
12.3 ms to the first client composite. The trace stores the first occurrence of
each stamp; `frame-pixels-ready` describes the first band, not the end of the
full comparison.

Code reading found these mechanisms:

- `libgfx::device::acquire` spun while its holder could be preempted or waiting
  in a GPU syscall. The new concurrent font worker and target creation contend
  for this lock. Contention now yields through the shared process interface.
- The hidden-cursor path presented the identical bare desktop twice. The fill
  check's presentation now also serves the full-frame check when the cursor is
  absent. Visible cursors still present before comparison. Recovery still
  presents its rebuilt desktop. The full comparison and six fill samples stay.
- Chrome used the per-glyph LCD draw path, and coverage warm-up omitted the
  minimize glyph. Chrome now uses the existing batched LCD renderer; preparation
  includes both button glyphs beside the window titles.
- The case's successful phase line went into its silent output pipe. The
  supervisor now retains desktop/kiosk phase records on successful discard as
  well as failure collection, before removing the trace.
- Forced races validated controls but retained no VALUE witnesses. Their
  independently checked overlap deltas now accumulate into two totals per case,
  respecting the existing 12-record cap. The scorer requires exact totals:
  drawing/readers/resize 1, destroy 23, threads 3, for both stall and release,
  before the corresponding CASE. The negative fixtures in `tests/gt710_ui_step_test.py` refuse missing, nonpositive, duplicate or late totals.
  It also requires the retained desktop phase record to meet U17's 270 ms target.
  The shared 300 ms case limit is unchanged.

## Deterministic checks

Check commits precede implementation. Production-source host checks reproduce
busy waiting without scheduler progress, the redundant hidden-pointer present,
the omitted chrome glyph and per-glyph submissions. The supervisor discard
fixture reproduces absent retained phases. Scorer negatives reproduce acceptance
without witnesses or headroom. The repaired paths pass these checks. The initial
LCD mock needed an explicit unused-method reference before reaching its runtime
assertion; that check-only correction is recorded separately. G2's aggregate
witness correction follows `suite.rs::RECORDS_PER_CASE`, with the existing
missing/nonpositive/duplicate/late refusal checks retained.

The existing fork-ownership check passes, as do the startup preparation,
presentation/mismatch-policy and local-collection ordering checks. The frame
capture harness passes 9/9, including fired controls, coupled pixels/tree/id,
partial refusals and fresh one-shot screen arenas. Unchanged suites are not
rerun on the host for these repairs.

## Pre-boot budget and remaining question

This is a code-derived forecast, not a measured timing result or a worst-case
bound. The phase records on the acceptance boot decide it.

| Critical-path phase | Budget ms | Source/change |
|---|---:|---|
| Connect, fixtures, pins, arena | 80 | Retained 76.26; unchanged |
| Launch and config | 15 | Retained 12.3; unchanged |
| Font coverage plus targets | 65 | Yielding removes holder starvation from the concurrent startup; retained contended interval 81.42 |
| Background | 11 | Retained 10.58; unchanged |
| Bare desktop chrome | 5 | Retained 4.92; unchanged |
| Fill presentation and six checks | 10 | Retained 9.84; unchanged |
| Metadata and warm-up dispatch | 1 | Title and button preparation overlaps the following check |
| Full-frame comparison | 62 | Retained 61.8; duplicate hidden-pointer presentation removed |
| Client chrome | 6 | Batched LCD draws and prepared button coverage replace the retained 22.57 ms path |
| First composite and observation | 14 | Retained about 12.3 |
| **Total** | **269** | **31 ms inside the unchanged 300 ms case deadline** |

The hardware question is whether removal of busy-wait starvation and redundant
presentation/draw work realizes these phase budgets on the shared GPU path.
Prediction: first desktop composite at or below 270 ms, retained success phases,
78/78 selected cases, exact overlap totals and compare-cost at or below 50 ms.
A miss is a failure to report from that capture, not permission for a sample boot.

Acceptance also owes console alone (88/88 within 330 s handoff-to-done),
window-manager (81/81), production BreenGL (401/401) and the default desktop.
No kernel or driver code changed, so GPU, files-io and directories are outside
this repair's reach. ARM uses the same renderer, sessions and yielding interface;
its runtime and canonical-scene pin remain unmeasured in #2770. The step's
unchanged ARM kernel build evidence remains in the u1 report.

## Build and lint results before acceptance

Seven affected userspace binaries built in release mode for x86 and ARM64 with
`CARGO_BUILD_JOBS=4` under `taskpolicy -b`. Both final builds exit 0, with
0 compiler warnings and 0 errors. After the transport aggregation and supervisor
change, those affected binaries were rebuilt once; unchanged code is not
retested after push. Claim-lint on changed prose and `git diff --check` exit 0.
The standalone libgfx host check refreshed its lockfile to include the existing
libbreenix input-abi dependency.

## Retained console acceptance and historical coordinator hold

Runtime head `616ab6ab` has one new acceptance capture,
`20261108T130827Z-metal`: console 88/88, memory PASS, no GR refusal, driver exit
0, complete capture, power-off confirmed. Build and tools identities both equal
`616ab6ab0d6d369e1bb8c0987f231f919c9a5649`. Handoff 85.2 s and final marker
380.7 s give **295.5 s**, inside the unchanged 330 s bound. The host production
build exits 0; its builder rejects compiler warnings. The broad GT scorer
returns 1: 28/31 rows pass; `core/row` and `core/dispatch` report missing core
bulk anchors, and `core/producer` reports missing named production and diagnostic
companions. That historical proof set is not supplied by this console capture.
No replacement core or GPU boot was run.

Before another boot, coordinator ledger line 328 (8:07 AM ET) requires a
stage-by-stage comparison of the two previous UI phase lines, attributing the
roughly 55 ms variance before a new prediction. The authoritative full records
contradict its premise that both phase lines exist:

| Retained file | Bytes | UI_STARTUP / UITEST_STARTUP occurrences |
|---|---:|---:|
| 120415 serial.txt | 381480 | 0 / 0 |
| 120415 serial.raw | 383939 | 0 / 0 |
| 122858 serial.txt | 383375 | 1 / 0 |
| 122858 serial.raw | 385844 | 1 / 0 |

120415's text SHA-256 is
`efe5bd5691b1adee7531b8e519beaaf818d98d80c406b6bafbf36f1d83368ec0`;
its raw SHA-256 is
`9096917753eec51aeee608e8d27df00008b153a8124be66380b9a8691925798b`.
The source explanation is the old supervisor's collection-only diagnostic
branch, reproduced and repaired in this change. A passing case's stdout line
was not retained. Independent `bwm-check-us` records give fill checks
11.649 to 9.820 ms and full comparison 64.420 to 61.769 ms: those costs fell.
The second-present acceptance to first-client presentation interval grew from
82.999310 to 96.700858 ms. The absent clean phase record prevents splitting the
remaining change among setup, font planning, targets and scheduling.

A blocker with these facts is recorded in the coordinator ledger. UI,
window-manager, BreenGL and default-desktop acceptance remain unrun at this
runtime head; the 269 ms forecast and the step remain **unproven**. No further
boot follows the new prerequisite without its disposition. #2791 remains open.

The subsequent host source-fixture correction replaces only literal
`Some(first_pointer)` inventory tokens with the branch's cursor-selection
expression. Its same ordered operations pass 1/1 under `-Dwarnings`. This and
the report do not change guest code; they do not justify repeating the console
boot. PR #2801 remains open and unmerged.

Claim-lint results: changed prose, this report, the runtime commit message and
the final source-fixture message exit 0. Auditing the earlier pushed check
messages finds wording-only exit-1 findings in `113f5dba` (glyph-coverage wording),
`40117ef7` and `8d457998` (zero-valued negative fixtures). Their executable
checks passed the documented red/green sequence. History was not rewritten to
change those messages.


## Continuation after the coordinator released the hold

Coordinator ledger line 335 (2026-11-08 08:37 ET) accepts the missing historical
phase-line explanation and authorizes one UI acceptance attempt at the existing runtime,
then window-manager, production BreenGL and the default desktop. It explicitly
preserves console `130827` and separates the broad core evidence gaps from
22.4. Those gaps are tracked in #2802. The hold described above is historical.

The authorized UI proof `20261108T134205Z-metal` at `4b8a514b` fails:
42/78 selected cases pass, DONE is 42 passed, 164 failed, 0 skipped, total 206.
Memory passes with 12 retained pages; clocks are full and no GR refusal appears.
The host production builder exits 0 under its zero-warning gate. Build and tools
identities match. The driver closes the capture with exit 1 and powers off.
The canonical step scorer refuses the 36 selected failures.

Desktop's VALUE is 269,836,671 ns, and kiosk's is 146,183,534 ns. Both pass
the shared 300 ms case check. Neither has a retained startup phase line, so the
step scorer's independent 270 ms first-composite requirement is still unproven.

Reading the supervisor and its callers identifies two defects introduced by
the previous retention repair:

- `RunnerSession::start` and the inner executor do not supply collection names.
  The new unconditional environment lookup on discard makes those supervisors
  return an error instead of completing their existing `done` handshake. The
  failures include ordinary frame sessions and clean inner-scenario controls.
- `Session::teardown` requests explicit teardown before the case returns. The
  supervisor removes its environment and private root at that point. Opening
  the trace only when the runner later sends discard cannot retain a successful
  startup record.

Check-first commit `bc80c10e` extends this branch's retention fixture under G2.
It executes the production explicit-teardown and collector branches together,
with a real temporary root whose removal the check verifies. The unchanged
source compiles with `-Dwarnings`, then fails at both mechanisms: unnamed discard
returns `environment variable not found`, and desktop/kiosk explicit teardown
loses its phase line. The earlier direct success/failure checks remain.
Initial fixture compilation found an unused type alias and an unread initial
state; those mock issues were corrected before recording the runtime red.

The repair makes identity optional only for diagnostic formatting; the existing
collection branch still requires names. It snapshots the named startup line
after explicit teardown and before removing the root, then emits that saved
line through the supervisor transcript at collector completion. The other path
opens the still-live trace then, as before. It preserves one emitted line per
startup and the existing cleanup/handshake order. The two host checks pass.
No renderer, kernel, case, threshold, or selection changes in this continuation.
The prior yielding/chrome fixes and their evidence remain unchanged.

Scoped release builds of `suite-ui-testing` and `ui-runner-fixture_test` pass
for x86 and ARM64 with `CARGO_BUILD_JOBS=4` under `taskpolicy -b`, with 0
compiler warnings and errors. Logs are `/tmp/e22-proof/build-x86.log` and
`build-arm.log`. Kernel source is unchanged; the earlier ARM kernel evidence
stands. ARM runtime and its canonical-scene pin remain #2770.

Claim-lint: the corrected #2802 issue body, the lifecycle check and its commit
message, changed prose and the follow-up report message exit 0. The first #2802
body invocation flagged wording in an instruction; the posted body was corrected.
The initial report lint finding was corrected. The pushed repair message in
`5a7ff2a6` retains a wording-only lint finding about compiler warning counts;
the logs above give the numeric counts. History was not rewritten.
`git diff --check` exits 0 after removing a trailing blank in the check commit.
