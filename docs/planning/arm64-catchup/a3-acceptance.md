# A.3 acceptance attempt — 2026-11-11

Status: **not accepted**. Runtime head `5fa5325cd40d5a2c9d16667a4e840f09daf1facd`, based on `78136dfa9616a25292229ad7ffe4b7f14b4f41e3`. PR #2874 remains a draft. The implementation and test commits are pushed under the effort branch and its native-build alias.

## Latest ARM result

Vigil `20261111T134512Z-qemu-29569` finished with exit 1, **80/81** cases passed. [Sanitized producer records](serials/a3-acceptance-5fa5325c.txt) retain the case verdicts, values, memory and completion records, and failure-only startup stamps.

| Group | Passed/total |
|---|---:|
| input-core | 9/9 |
| input-devices | 2/2 |
| startup | 6/6 |
| composition | 6/6 |
| stacking | 5/5 |
| focus | 9/9 |
| pointer | 6/6 |
| geometry | 6/6 |
| lifecycle | 9/9 |
| recovery | 4/5 |
| clipboard | 6/6 |
| liveness | 5/5 |
| performance | 7/7 |

The one failure is `recovery/supervisor-restarts`: the replacement's first client composite arrived at 2.139174 seconds against the unchanged two-second limit. Its trace reaches font planning at 29.063 ms, returns from planning at 1.173553 seconds and font submission at 1.807017 seconds; targets are ready at 1.932132 seconds, startup checks at 2.098373 seconds, and the client composite at 2.139174 seconds. These stamps locate the remaining startup work; they do not establish a further fix. Issue #2872 stays open.

The seven performance cases pass: idle CPU is 0%; resident growth is 4 KiB against 64 KiB. Suite reclamation retains 10 pages against 48. DONE precedes MEMORY PASS, which precedes `SUITE_SEQUENCE READY window-manager`. The host renderer check accepts ANGLE/Metal with 984 records. The run was normal-priority, headless HVF/SPICE GL, with no debugger or QEMU window.

The wrong-pixel and missing-provenance oracle, suppressed-report control, compositor-steal refusal and inject-denied refusal pass their unchanged cases in this capture. These are measured case results, not a full-suite acceptance claim.

## Acceptance contract conflict

The brief requires an ARM full-launch finished-pass Vigil record from the 81 implemented cases. The unchanged [watcher](../../../scripts/watch-qemu.py) calls the unchanged [generic verdict](../../../scripts/suite-verdict.py) without a selection. The latter refuses a full manifest containing empty categories before checking case records. The unchanged [manifest](../../../docs/suites/window-manager.json) contains 12 empty `*-remaining` categories. These two contract files match origin/main.

Scoring the retained latest capture gives:

```text
FAIL: planned categories have no cases: input-core-remaining, input-devices-remaining, startup-remaining, composition-remaining, stacking-remaining, focus-remaining, pointer-remaining, geometry-remaining, lifecycle-remaining, recovery-remaining, clipboard-remaining, liveness-remaining; acceptance is blocked
```

Consequently, 81/81 implemented cases would still fail the requested full-launch record. Existing issue #2784 records the same distinction on native hardware: its 81-case step scorer passes while the generic full-suite record refuses the planned stubs. That issue explicitly preserves this check. The native step scorer already recognizes its exact refusal; the ARM full-launch requirement supplies no equivalent selection.

A coordinator decision about the acceptance scope is recorded as an A.3 ledger blocker. The brief explicitly requires stopping when an expectation is wrong. No manifest, runner, assertion, threshold or verdict was changed to bypass this conflict. The separate supervisor timeout remains an implementation defect, not a request to change its limit.

ARM console and native window-manager, console, GPU, BreenGL and selected UI category boots were not run. No ARM or x86 board cells were changed. No native tools lock remains held.

## Host checks and builds

The original host-first compile blocker was a missing diagnostic macro in the standalone runtime harness. The added sink type-checks diagnostic arguments; the 15 runtime assertions remain intact.

| Required host target | Result |
|---|---:|
| `input_host_channel` | 10/10 |
| `input_host_runtime` | 15/15 |
| `bwm_latency_pacing` | 13/13 |
| `bwm_structure` | 5/5 |

The u2–u5 red-first and regression evidence is retained in the [host-channel](a3-u2-host-channel.md), [pointer-frame](a3-u3-pointer-frame.md), [pacing](a3-u4-pacing.md) and [receipt](a3-u5-input-replay.md) reports. U6 supplied no implementation or new test. Later acceptance repairs have focused red/green host models for their producing defects; the source changes preserve the suite's waits, limits, provenance and fault controls.

ARM64 and x86 release kernel, bwm, suite-window-manager and wm-client_test builds pass. The latest kernel source is `90850206`; bwm is `5fa5325c`; the suite and client were built after the `d0819b7f` diagnostics. Unchanged packages reuse their verified builds. ARM's existing pinned-core future-incompatibility advisory is the G35(2) exception; the builds add no warnings of their own. The ARM kernel FP guard passes. The supported ARM installer installed 216 binaries and the disk was refreshed before the latest boot.

Changed-hunk claim lint at the runtime head is clean across 66 files; 137 findings outside changed hunks remain reported as existing. This report receives its own documentation lint before commit. No checks are rerun merely because a commit is pushed.

## Repairs and remaining evidence

The captured invalid-resource reset was caused by cleanup unreferencing internal context resources whose device creation had not completed. `226bc978` publishes device allocation only on the matching acknowledgment and preserves context lifetime through pending synchronous work. Retained restart adoption then passes. The implementation also repairs missing completion drains, ARM serial line replay ordering and caller identity lookup, reusable capture storage, idle validation interruption, startup font/background overlap and reserved renderer preparation. The focused source models retain their original pixel and resource-lifetime assertions.

The last two full captures show separate outcomes. At `8dfb9b2c`, [Vigil 20261111T132259Z-qemu-76612](serials/a3-acceptance-8dfb9b2c.txt) passed recovery 5/5 but failed idle CPU above 2%, for 80/81 overall. A new actual main-call/helper test then reproduced scene-only admission moving an eligible 20 ms check to 800 ms. `5fa5325c` retains observed-cost deferral for real input and preserves scene-only first-frame eligibility; its two activity models pass and idle CPU returns to 0%. The latest capture still exposes the supervisor startup miss; no pass is inferred from the earlier capture.

Bounded CPU phase counters use the existing suite-owned trace page and export deltas only on an idle-CPU failure. Startup and readback diagnostics retain failing observations without printing on successful wait polls. They remain available for a code-first continuation.

Two unrelated host fixture failures were recorded separately as #2789 and #2876 after comparison with origin/main. They were not weakened or chased in this change.

## Retained ARM capture history

This table preserves prior failures and selected diagnostics; it does not combine results from different heads into acceptance. Exit 1 remains a failure. A selected diagnostic is not full-suite or board evidence.

| Vigil boot | Head | Scope | Passed/total | Exit |
|---|---|---|---:|---:|
| `20261111T054423Z-qemu-34249` | `46c2abd3` | full | 77/81 | 1 |
| `20261111T065657Z-qemu-97132` | `09d8846d` | full | 77/81 | 1 |
| `20261111T071913Z-qemu-52040` | `47d581a8` | full | 79/81 | 1 |
| `20261111T073308Z-qemu-79759` | `f06e3705` | full | 79/81 | 1 |
| `20261111T074925Z-qemu-22275` | `47beead0` | full | 77/81 | 1 |
| `20261111T080136Z-qemu-47569` | `ec776ca0` | full | 79/81 | 1 |
| `20261111T081601Z-qemu-a3-recovery` | `93b294f4` | launch, no case verdict | -/- | - |
| `20261111T081707Z-qemu-a3-recovery` | `93b294f4` | launch, no case verdict | -/- | 1 |
| `20261111T082030Z-qemu-a3-recovery` | `f3c90449` | selected | 0/1 | 1 |
| `20261111T083128Z-qemu-a3-recovery` | `77b3d014` | selected | 0/1 | 1 |
| `20261111T083348Z-qemu-a3-recovery` | `77b3d014` | selected | 0/1 | 1 |
| `20261111T084203Z-qemu-29873` | `d8c21fda` | full | 79/81 | 1 |
| `20261111T085633Z-qemu-a3-recovery` | `df4d2d1d` | launch, no case verdict | -/- | 1 |
| `20261111T090051Z-qemu-a3-recovery` | `c71a0144` | selected | 0/1 | 1 |
| `20261111T091646Z-qemu-4047` | `737c0717` | full | 79/81 | 1 |
| `20261111T093101Z-qemu-31845` | `49c298c8` | full | 79/81 | 1 |
| `20261111T095252Z-qemu-69448` | `c3bcb8e6` | full | 79/81 | 1 |
| `20261111T100510Z-qemu-a3-startup` | `7de65d2f` | selected | 1/1 | 0 |
| `20261111T100557Z-qemu-3669` | `7de65d2f` | full | 78/81 | 1 |
| `20261111T101807Z-qemu-28726` | `c81ab151` | full | 76/81 | 1 |
| `20261111T103525Z-qemu-60402` | `4951dcdb` | full | 78/81 | 1 |
| `20261111T104731Z-qemu-91576` | `93fbd489` | full | 77/81 | 1 |
| `20261111T111827Z-qemu-a3-recovery-copy` | `46c379bd` | launch, no case verdict | -/- | 130 |
| `20261111T111844Z-qemu-a3-recovery-copy` | `46c379bd` | selected | 3/5 | 1 |
| `20261111T112523Z-qemu-99765` | `a1556eff` | full | 78/81 | 1 |
| `20261111T113614Z-qemu-a3-reset` | `41af6189` | selected | 1/1 | 0 |
| `20261111T113747Z-qemu-a3-reset-cat` | `41af6189` | selected | 3/5 | 1 |
| `20261111T114527Z-qemu-34390` | `226bc978` | full | 79/81 | 1 |
| `20261111T120252Z-qemu-a3-phases` | `075e6d12` | selected | 0/1 | 1 |
| `20261111T120852Z-qemu-a3-phase-env` | `3c6aad97` | selected | 0/1 | 1 |
| `20261111T121843Z-qemu-5644` | `ed618ce3` | full | 78/81 | 1 |
| `20261111T124428Z-qemu-52293` | `fce39d83` | full | 79/81 | 1 |
| `20261111T132259Z-qemu-76612` | `8dfb9b2c` | full | 80/81 | 1 |
| `20261111T134512Z-qemu-29569` | `5fa5325c` | full | 80/81 | 1 |
