# F32k Validation Exit

Branch: `f32k-ttwu-input-cursor`  
HEAD: `8cb108a9 feat(gui): F32k migrate op 19 ReadWindowInput to waitqueue`  
Run artifacts: `.factory-runs/F32k-validate-20260419-162008/`

## Verdict

**FAIL. Do not merge.**

The A+B branch passed clean build preflight and the wait-stress gate, and the
first normal Parallels boot passed. The second required normal boot failed the
CPU0 tick evidence gate: it reached bsshd, bounce, sustained compositor frames,
strict render PASS, no AHCI timeout, and visible cursor pixels, but the serial
log contains no CPU0 tick-count evidence beyond timer configuration.

Because the required gate is **5/5** normal boots with CPU0 `tick_count > 1000`,
validation stopped after `normal-2`. No PR was opened and nothing was merged.

## Sweep Table

| Gate | Command / artifact | Result | Evidence |
| --- | --- | --- | --- |
| Branch preflight | `git status --short --branch`; `git log --oneline -6` | PASS | Branch was `f32k-ttwu-input-cursor` at `8cb108a9`, containing `91aa574d` + `8cb108a9` on top of `df914fbe` (`origin/main`). |
| x86_64 build | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS | `build-x86_64.log` finished release build with no `warning` / `error` lines. |
| aarch64 build | loader + kernel + `userspace/programs/build.sh --arch aarch64` | PASS | `build-aarch64.log` finished loader/kernel/userspace with no `warning` / `error` lines. |
| wait-stress first run | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | INCONCLUSIVE | No `WAIT_STRESS_STALL`; `wait_stress exited pid=2 code=0`; final PASS line was corrupted by interleaved TTBR diagnostics, so this was not counted as the gate pass. |
| wait-stress rerun | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | PASS | `wait-stress-rerun.serial.log:421` contains `WAIT_STRESS_PASS`; no `WAIT_STRESS_STALL`; strict render `VERDICT=PASS`. |
| normal run 1 | `./run.sh --parallels --test 120` | PASS | `normal-1.audit.txt`: bsshd=true, bounce=true, `max_cpu0_ticks=80000`, `max_frame=19500`, `fps=243.75`, no AHCI timeout, cursor visible; `normal-1.render.txt`: `VERDICT=PASS`. |
| normal run 2 | `./run.sh --parallels --test 120` | **FAIL** | `normal-2.audit.txt`: bsshd=true, bounce=true, `max_cpu0_ticks=0`, `max_frame=20000`, no AHCI timeout, cursor visible, `RESULT=FAIL`; `normal-2.render.txt`: `VERDICT=PASS`. Raw serial has frames through `Frame #20000` but no `[timer] cpu0 ticks=...` or `tick_count` evidence. |
| normal runs 3-5 | Not run | SKIPPED | Stopped after normal run 2 per failure policy. |
| input delivery smoke | Not run | SKIPPED | Stopped after normal run 2 per failure policy. |
| PR / merge | Not run | SKIPPED | No PR opened; no merge attempted. |

## Failure Evidence

`normal-2.serial.log` contains:

- `[init] bsshd started (PID 4)`
- `[init] bounce started (PID 5)`
- `[bounce] Window mode: id=1 400x300`
- `[virgl-composite] Frame #20000: 1280x960 -> 1280x960 display`
- no `AHCI TIMEOUT`

But searching `normal-2.serial.log` for `tick`, `tick_count`, `cpu0`, or
`[timer]` only finds timer setup and no CPU0 progress line:

- `[timer] Timer configured for ~1000 Hz (24000 ticks per interrupt)`
- `[timer] Using virtual timer (PPI 27)`

The required CPU0 evidence gate was therefore not met.

## Cursor Evidence

The existing strict render tooling does not include a cursor-specific verdict,
so the run used a screenshot mask audit based on BWM's aarch64 software cursor:
the initial pointer is at `(0,0)` and the 16x16 arrow mask should contain
53 white body pixels and 48 black outline pixels. Both normal runs matched this:

- `normal-1.audit.txt`: `cursor_white=53`, `cursor_black=48`, `cursor_visible=True`
- `normal-2.audit.txt`: `cursor_white=53`, `cursor_black=48`, `cursor_visible=True`

## Cleanup

Temporary Parallels VMs created by the completed runs were stopped and deleted
by the run wrapper after artifacts were copied.
