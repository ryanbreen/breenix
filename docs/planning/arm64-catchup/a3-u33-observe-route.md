# A.3 observation route

This unit implements coordinator ledger line 303 (1)-(2). It runs no boot.
The replacement measurement and acceptance remain separate, tracked in #2872.

## Cause and change

<!-- claim-lint:ok: /tmp/a3-measure-d94db43f/serial.log; cases.rs input_latency; suite.rs upload_diagnostic/evidence_route. -->
The void capture `20261112T081720Z-qemu-97614` reports
`diagnostic-route-absent` for typing, pointer and animation latency. The case
propagated the optional upload error before reporting its latency statistics.
The launcher route lookup legitimately returns absence for a missing route file.

<!-- claim-lint:ok: userspace/programs/src/wm_suite/cases.rs input_observation/input_latency diff. -->
Observation command, trace-read and upload errors now produce an explicit
`VALUE wm-input-observation ... skipped=<error>` record through the existing
diagnostic writer. The upload disposition identifies the client window and
typing, pointer or animation label. With a route, the existing artifact upload
and receipt remain. Missing-route uploads log `skipped=diagnostic-route-absent`.
The latency sample selection, median/max statistics, bounds and final verdict
code are unchanged. No kernel or launcher code changes in this unit.

Skipping preserves the case result; it does not supply the missing animation
observation for a later measurement or establish a latency mechanism.

## Check and builds

<!-- claim-lint:ok: /tmp/a3-u33-red.log; /tmp/a3-u33-green.log; tests/a3_observation_route_test.py and tests/fixtures/a3_observation_route.rs. -->
Check-first commit `15c725c0` executes the production `input_latency`,
`input_observation`, `upload_diagnostic`, `evidence_route` and diagnostic writer
bodies with guest filesystem and completion fixtures. It compiles with
`-Dwarnings`. Before the fix, the passing-latency assertion fails with
`Err("diagnostic-route-absent")`. After the fix it passes, checking passing and
failing latency statistics and verdicts for typing, pointer and animation,
explicit missing-route records, and nonfatal observation command/read errors.

<!-- claim-lint:ok: /tmp/a3-u33-u32-checks.log; /tmp/a3-u33-arm-suite.log; /tmp/a3-u33-x86-suite.log. -->
The requested u32 checks pass, 13/13: animation observation, accounting,
proof gate, GPU lock observation and GPU lock attribution. Scoped release
builds of `suite-window-manager` pass on ARM64 (10.10 s) and x86 (9.84 s,
`framebuffer-v1`), with no warnings. Builds run under `taskpolicy -b` with
`CARGO_BUILD_JOBS=4`. The changed code is confined to the suite binary.

Main `3136b044` was merged first in `a6e6a6fa`; no rebase or force-push.
This unit opens or merges no PR and performs no deployment or hardware access.
