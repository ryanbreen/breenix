# A.3 u3: absolute pointer frames

`Stream::read` now retains a device's absolute frame through `SYN_REPORT`,
including when the reader splits its axes across calls. `positions` assigns
one final position to the first supported axis record. The other records
still reach `begin` and `finish` with their original serials. `SYN_DROPPED`
releases the pending records for accounting without routing their motion.
Relative-only frames keep the existing batch coalescing; single-axis absolute
frames retain the other committed coordinate.

The first axis remains the motion source after the whole frame is available.
This keeps a button following those axes at the completed position, as the
existing `wm_suite::cases::virtual_pointer` case requires. Frames are retained
per device so another device's SYN cannot release them. This follows the frame
contract in `docs/gui/input.md`; no driver or input ABI change was needed.

The test-only commit is `ae177d4b`. Run the check from the repository root:

```sh
rustc --edition=2021 -Dwarnings --test tests/bwm_pointer_frame.rs -o /tmp/bwm-pointer-frame
/tmp/bwm-pointer-frame --nocapture
```

It compiles the production Stream, router, normalization and source-accounting
functions with guest I/O replaced. On the original implementation it failed:
`[(80, 50), (80, 70)]` versus the required `[(80, 70)]`. With the fix it passed
(1 test). The same check covers whole frames, both read split points,
single-axis and relative frames, dropped and interleaved device frames, a
button in the axis frame, and one terminal source disposition per record.
The existing `tests/bwm_pointer_route.rs` passed its 18 tests; its implementation
was unchanged. WM suite cases, runner and thresholds are unchanged.

No boot was run in this unit. ARM and x86 acceptance remains with the acceptance
unit. The four A.3 baseline failures are not shown to be cleared by this change.

Scoped release builds of `bwm` passed for `aarch64-breenix.json` and
`x86_64-breenix.json`, with no compiler warnings. The x86 libc link dependency
also built without warnings. Mac builds used `taskpolicy -b` and
`CARGO_BUILD_JOBS=4`. Neither kernel nor Kepler assets changed.

Claim lint: `python3 scripts/claim-lint.py --base e34abebb` exited 0.
Commit-message lint for the check and implementation messages exited 0;
the check message was linted before its test-only amendment.
