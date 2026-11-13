# A.4 unit 10: checkpoint phase records

This unit implements ARM ledger line 390 items 1 and 2 for #2915. No boot
belongs to this unit; acceptance remains a later unit.

The [u7 case records](serials/a4-u7-ui-testing.txt) report focus PASS in
18813 ms (line 116), over the unchanged `1da26f19` runtime. The 20000 ms
deadline leaves 1187 ms, or 5.935% headroom. It already missed the required
25% headroom before the snapshot-reacquisition change. This observation
does not identify the expensive phase.

`wm_scenario` enables reporting for its focus or stack execution, without
a runtime flag. Checkpoints are numbered in call order. The grammar is:

```text
VALUE wm-phase <focus|stack> <checkpoint> <phase> <milliseconds> <reacquires>
```

Times have microsecond precision, rounded upward. The reporter uses the
guest device `/dev/screen-evidence` when a phase finishes, before the next
phase starts.
It does not wait for an error return, teardown or case verdict. Interrupted
phases have no completion record. A checkpoint total includes nested work
and reporting; nested intervals must not be added to it again.

The outer phases are park, settle, desktop-tree, arena creation, client
capture, tree read/hash/parse/check, screen capture, front validation,
screen arena read/hash/copy, screen comparison and the whole checkpoint.
The shared capture code splits client capture into arena mapping, device
copy and publication (tree/frame hashes and metadata). Screen capture
reports arena mapping, device readback and publication hash separately.
Repeated client phases follow window order; repeated arena phases precede
the corresponding client or screen. Reacquisition emits its updated count
and starts another snapshot under the existing deadline.

Output is bounded to 8192 bytes per scenario, including an explicit `limit`
record if the report budget is exhausted. There is no retry cap or success
fallback. The 20000 ms case deadline, two-second checkpoint deadline and
generation, composite, tree and screen predicates are unchanged. Only the
focus and stack scenarios activate reporting; other capture callers do
not open the evidence device or read the phase clock.

## Checks

The check-only commit precedes the implementation. On `a5c9a0fa`,
`python3 tests/wm_phase_records_test.py` compiled the production checkpoint
with `-Dwarnings`, stopped it at screen capture and failed because no park
completion record existed. The check also requires tree/client completion
records before that stop, later checkpoint completion and bounded output.

The check-only commit is `f08c4f8c`. The following scoped checks passed:

```sh
python3 tests/wm_phase_records_test.py
python3 tests/wm_snapshot_test.py
python3 tests/uitest_capture_busy_test.py CaptureBusy.test_capture_waits_for_completion_without_resetting_deadline
python3 tests/ui_app_control_test.py Controls.test_tree_capture_avoids_pixels_but_frame_capture_keeps_them
```

The snapshot check retains its reacquisition and permanently stale-composite
assertions. Existing capture fixtures import the reporter module without
changing assertions. The tree/frame check needed its documented host-library
prerequisite; that build completed with the `breenix` target cfg registered
and `-Dwarnings`.

Scoped `suite-ui-testing` release builds passed on ARM64 and x86, with no
warnings or errors. Builds used `CARGO_BUILD_JOBS=4` and background policy,
the verified Rust fork, and the existing libc archives. No kernel source
changed. The resulting release binary hashes are:

| Target | SHA-256 |
|---|---|
| ARM64 | `2baf45777125d51eabdd717eda0b7450ba068819dfc7d3e987537f45cba857ca` |
| x86 | `14bef5fb479fe4cf53a4d4c961f4ee7d68376741c41e8ad8012a918c77d88483` |

The separate `ui_capture_test.py --test ui_captures` fixture fails to compile
with ten missing-interface errors. A clean export of origin/main `5ee353ad`
reproduced the same ten errors; #2916 tracks that existing fixture problem.
No assertion was loosened or removed. Its repair is outside this unit.

The final lint invocations and push identity are recorded in the effort
ledger. No acceptance result is claimed here.
