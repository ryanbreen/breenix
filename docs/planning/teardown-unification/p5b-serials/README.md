# P5b preserved serials

Every red run this phase produced, and the exhibits its acceptance rests on. Preserved rather than
described, per the campaign's standing rule.

## Attributable non-green — open #589

| File | Gate | Signature |
|---|---|---|
| `589-fulltest-phase1c-20260817-230637.txt` | `run-aarch64-full-test.sh --boot-tests-only` | Phase 1c 30s timeout |
| `589-fulltest-phase1c-20260817-235600.txt` | same | same |
| `589-servicesequence-soak-max.txt` | `run-aarch64-service-sequence-gate.sh` | bucketed `589` by the gate's own classifier |

All match **#589** field-exactly: `CLONEVM_EXEC_TEST: live sibling refused exec` is the last CLONEVM
line, the sibling's `[syscall] exit(0) … name=thread-N` never appears, and heartbeats keep their ~1s
cadence to the timeout. The parent is spinning in `wait_for_zero_u32` on the pre-exec
`clear_child_tid` handshake while the sibling is never scheduled.

**Attribution measurement (interleaved A/B, this Mac, unloaded).** Prebuilt kernel + ext2 image pairs
were swapped in and out so nothing but the code differed, alternating arms across six rounds:

| arm | PASS | Phase 1c red |
|---|---|---|
| `main` @ `4f64be15` | 5/6 | 1 |
| P5b branch | 4/6 | 2 |

`main` reds at the same signature, so this is #589 and not a P5b regression. The 60-boot
service-sequence soak on the branch put it at 10/60 (17%), matching `main`'s 1/6 in the A/B.

## Round-2 blocker exhibits

| File | Gate run | Bucket | What it proves |
|---|---|---|---|
| `dataabort-schedule-from-kernel-cortexa72-boot55.txt` | 200-boot service-sequence run, `cortex-a72` boot 55/100 | `DATA_ABORT` (#596; formerly misattributed to `P5B`) | The boot reached all three service markers and then took an EL1 data abort at `schedule_from_kernel`; a crash must be classified before any late P5b marker check. The fault lands entirely in context-switch code byte-identical to `main`. |
| `serial-interleave-max-boot33.txt` | same service-sequence run, `max` boot 33/100 | `P5B` false red (B2) | The unlocked periodic timer writer byte-interleaved with the pinned quiesce walk, producing `[INIT_GROUP_WALK...[timer] cp0:reu0 tficusks=ed1=4:verdict=P0000`; deleting that IRQ-path writer removes a permanent false-red source. |

Both files are byte-for-byte copies of the serials preserved from the run.

## Mutation exhibits (each turned `run-aarch64-boot-test-strict.sh` red)

| File | Mutation | Observed |
|---|---|---|
| `mutation-M1-baseline-refusal-deleted.txt` | refusal deleted — this is `main`'s behaviour, i.e. the anti-vacuity baseline | `probe1=191:probe2=192` (clones succeeded), `[INIT_GROUP_CHILD_RAN]` ×2, no walk |
| `mutation-M2-guard-inverted.txt` | guard inverted to `designated_init().is_none()` | oracle `none_refusals=3:init_refused=0:alias_refused=0`, probes succeeded |
| `mutation-M3-pid-not-group.txt` | compare init's **pid** instead of its **effective TGID** | oracle `alias_refused=0:alias_pid_refused=1` |
| `mutation-M4-refusal-cfg-out-aarch64.txt` | refusal `#[cfg]`'d to x86 only | probes succeeded and the child ran while the oracle stayed green — the runtime vehicle, not the oracle, is what catches it |
| `mutation-M5-walk-skips-init-row.txt` | walk stops counting the init row | `init_tgid_rows=0 … verdict=FAIL` on both walks |

## Acceptance exhibit

`green-quiesce-walk-max.txt` — one of the 50 GREEN boots of the 60-boot service-sequence soak,
carrying `[INIT_GROUP_WALK:aarch64:rows=11:init_tgid_rows=1:foreign_tgid_rows=0:refused=4:verdict=PASS]`
with init's full service set live. All 50 carried it; 0 carried a `verdict=FAIL` walk or
`[INIT_GROUP_CHILD_RAN]`.
