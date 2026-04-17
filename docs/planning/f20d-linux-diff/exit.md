# F20d Exit

## Verdict

FAIL: Phase 1, Phase 2, and Phase 3 completed and were pushed. Phase 4 did not produce a fix that passes the required gates.

## Phase Outcomes

- Phase 1 Linux ground truth: completed and pushed in `e9f16a42`.
- Phase 2 Breenix capture: completed and pushed in `c676dcff`.
- Phase 3 divergence table: completed and pushed in `2f12b4d6`.
- Phase 4 fix: blocked. Context-switch-only changes did not pass validation.

## Phase 4 Evidence

Validation attempts are under `logs/breenix-parallels-cpu0/f20d/fix-sweep/` in the local workspace.

Observed outcomes:

- Removing idle-loop timer reprogramming made CPU 0 tick again only while the intrusive `PER_CPU_IDLE_AUDIT` serial path was still present. Example: run2 reported `timer_tick_count=29882`, but `boot_script_completed=0` and `/bin/bsh` failed with `EIO`.
- Removing the intrusive idle serial audit brought back the original CPU 0 failure. Examples:
  - run4: `timer_tick_count=10`, `post_wfi_count=0`, `boot_script_completed=0`
  - run5: `timer_tick_count=9`, `post_wfi_count=0`, `boot_script_completed=0`
  - run6: `timer_tick_count=15`, `post_wfi_count=0`, `boot_script_completed=0`
- The 5-run acceptance sweep was not run to completion because the candidate fix failed the single-run gate.

## Constraint Status

- No PR was opened or merged.
- No prohibited Phase 4 files were edited.
- No polling fallback was added.
- QEMU cleanup was run before handoff.

## PR URL

None.
