# A.3 u9: overlap replacement target allocation with renderer preparation

This unit continues the existing branch and merges `origin/main` at `78136dfa` without conflicts.
It runs no boots and changes no deadline, case, manifest or verdict.

## Capture evidence and its limit

`20261111T132259Z-qemu-76612` at `8dfb9b2c` reports
`recovery/supervisor-restarts PASS`. It contains no `VALUE wm-startup` records.
`wm_suite/desktop.rs::StartupTrace::failure` exports those records only on failure;
`StartupTrace::drop` removes the trace directories. The retained successful boot folder
has no separate startup trace. Thus these captures cannot identify which replacement
phase grew between runs, or establish why its duration grew. The later scene-only
validation change is not evidence of a particular phase regression.

The failed replacement, PID 665 in `20261111T134512Z-qemu-29569` at `5fa5325c`, records:

| Phase | Elapsed from replacement exec |
| --- | ---: |
| Font planning begins | 0.029063 s |
| Claim/scene ready | 0.968124 s |
| Reserved context transferred | 0.968133 s |
| Desktop target ready | 1.065572 s |
| Background target ready | 1.261888 s |
| Scanout A ready | 1.626962 s |
| Scanout B ready | 1.657754 s |
| Scene target ready | 1.714111 s |
| Font planning returns | 1.173553 s |
| Font submission returns | 1.807017 s |
| Fonts ready | 1.809238 s |
| Cleared targets and painted background ready | 1.932132 s |
| Bare desktop checks ready | 2.098373 s |
| First client composite | 2.139174 s |

Five large target allocations occupy 745,978 us after context transfer. The old
startup path does not begin them until `session::claim` returns. That claim waits
for the reserved renderer catalogue in `kernel/src/gpu/mod.rs::prepare_window_renderer`.
Target resource creation itself does not require a compositor claim or a context.
`CreationGuard` serializes process resource/context creation, and `create_device_context`
attaches existing live process resources. Font preparation also creates resources,
so its planning/submission can contend with the late target allocations. That
contention is a code-derived explanation to check, not a measured comparison with
the successful boot.

## Change and deterministic check

`b27c7dc4` commits the red check before the implementation. Its extracted production
opening methods fail with **0 targets instead of 5** at the modeled claim completion.
The check requires five targets before the claim join, no context transfer or clears
before that join, one completed clear submission afterward, and the normal recovery
clear path. It also binds the production startup call to claim-worker dispatch,
target allocation and claim join in that order.

The implementation starts the claim on a scoped worker. `Compositor::open_during`
allocates the desktop, background, two scanouts and scene target before joining that
worker. It then takes the reserved context and performs the existing clears. A partially
allocated compositor drops its resources without destroying context handle `0`.
Background pixels, full startup comparisons, title coverage, recovery bounds and
client composition remain in place. The steady-state loop, scene-only validation
eligibility, input deferral, clock clipping, pointer routing and pacing are unchanged
by this unit.

This removes the serial dependency for the recorded 746 ms allocation interval.
The wall-time saving remains unmeasured: kernel creation serialization, font
contention and renderer work can overlap differently. The acceptance target remains
first client receipt at or below 1.6 s, against the unchanged 2 s deadline. The next
acceptance unit must measure that target from its single final-head boot; this unit
does not claim a timing pass.

## Validation

- `python3 -m unittest discover -s tests -p 'bwm_startup*test.py'`: 14/14 pass.
- `tests/bwm_startup_structure.rs`, compiled with `rustc --test -Dwarnings`: 1/1 pass.
- Scoped `bwm` release builds: ARM64 and x86 pass, with no compiler warnings.
  Both use the pinned Rust library, `CARGO_BUILD_JOBS=4` and `taskpolicy -b`.
- The font-overlap fixture understands the new opening API. The recovery fixture
  exports its included public helper module to avoid an unrelated dead-code error.
  Their assertions are unchanged.
- `git diff --check`: exit 0.
- `claim-lint: scripts/claim-lint.py --base HEAD`: exit 0 before this report.
- `claim-lint: scripts/claim-lint.py --files tests/bwm_startup_target_overlap_test.py`: exit 0.
- `claim-lint: scripts/claim-lint.py --commit-msg /tmp/a3-u9-test-message.txt`: exit 0.
- `claim-lint: scripts/claim-lint.py --commit-msg /tmp/a3-u9-merge-message.txt`: exit 0.
- `claim-lint: scripts/claim-lint.py`: exit 0 after this report's wording corrections.
- `claim-lint: scripts/claim-lint.py --commit-msg /tmp/a3-u9-fix-message.txt`: exit 0.

No acceptance, board update, PR creation, PR merge, deployment or hardware access is
performed by this unit. Issue #2872 remains open for final-head acceptance.
