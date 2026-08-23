# T3-G PR3 round-4 gate logs

Captured on the shipped round-4 tree of `fix/producer-corruption-family`. Ruling R48 set the round-4
gate list; these are its outputs, kept here so the acceptance table in PR #642 can be checked against
something durable rather than a scratch directory.

Round 3's log set skipped `02` — the boot-tests gate had no captured log. R48 item P5 required that
gap closed, so the numbering here is complete: `01`, `02`, `04`, `05` are the gates, and `07`–`10`
are the ratchet mutation runs and the structural-suite tally. `03` and `06` (the x86 build and x86
boot tests on beast) are round-3 artifacts and were not in R48's round-4 list; round 3's copies
stand.

| file | what it is |
|---|---|
| `01-aarch64-prod-build.txt` | aarch64 production build, `aarch64-breenix-kernel.json` (the soft-float kernel target — never the NEON one, per the #528 re-arm rule), after a forced `cargo clean -p kernel --release`. `EXIT=0`. Zero PROJECT warnings: the only `warning:` line is the pre-existing toolchain `core` future-incompatibility note, which is not project code. |
| `02-aarch64-boot-tests.txt` | `run-aarch64-full-test.sh --boot-tests-only --rebuild`. **PASSED, 107/107.** This is the log R47 asked for and round 3 did not capture. It now also carries **Phase 5**, the new `run-aarch64-refusal-drain-gate.sh` sub-gate, which is how legs G and H became gate evidence instead of something a human remembers to run: `ARM64 REFUSAL DRAIN GATE: PASSED`, both legs `:PASS]`, `[BOOT_TESTS:PASS]`, `[BLOCK_EINTR_ORACLE:PASS:…]`. |
| `04-ss25-summary.txt` | service-sequence gate, 25 boots × 2 profiles. `max` **PASSED 25/25**. `cortex-a72` **FAILED 24/25** on one red. Total **49/50 GREEN**, `575=0 576=0 626=0 635=0 DATA_ABORT=0 CLONE_EXEC=0 STRAND=0 BOOT_TEST_FAIL=0 596=0 612=0 609=0 P5B=0`, `Resume PC refused: 0/50`, `RET dispatch refused: 0/50`. The red is attributed below. |
| `05-strict20-summary.txt` | strict boot gate, 1×20, including its `kernel_no_neon_guard` preflight. |
| `07-ratchet-M6-red.txt` | R48 item P1, mutation 1. The foreign-drain ratchet under **M6** (the CPU comparison replaced by `if false`): **1 failed** — `expected exactly one \`if\` comparing cpu_id against drain_cpu; found 0`. Under the round-3 ratchet this mutation stayed green; that is the vacuity R48 named. |
| `08-ratchet-M10-red.txt` | R48 item P1, mutation 2. The same ratchet under **M10** (the foreign early-out block deleted outright, restoring the pre-round-3 all-CPU drain): **1 failed**, same assertion. |
| `09-departure-ratchet-M9-red.txt` | The new departure ratchet under **M9** (`let departed = !on_victim_stack` → `let departed = true`): **1 failed** — `set_terminated is not dominated by the departure test`. |
| `10-structural-suites.txt` | the twelve structural suites on the final tree: **306 passed, 0 failed**. |

## The one red, attributed by field signature

`cortex-a72` boot 21:

```
[DATA_ABORT] FAR=0x2 ELR=0xffff000040846998 ESR=0x96000005 DFSC=0x5 TTBR0=0x1000044137000 from_el0=0
```

Field for field, that is **#641** — filed from the round-1 battery with exactly
`[DATA_ABORT] FAR=0x2 ESR=0x96000005`, open. When this run was scored the gate's classifier had no
#641 bucket, so it scored it `UNATTRIBUTED`, which is gate-failing; the gate therefore reports FAILED
and that verdict is reported here as measured. No tolerance was added and no bucket was created for
it in round 4: adding one after seeing the red is exactly the goalpost-move the campaign law forbids.

> **Round-5 addendum (ruling R49).** R49 pre-adjudicated #641 by its field signature and authorized a
> field-keyed bucket for it in the NEXT round; it exists now, in
> `run-aarch64-service-sequence-gate.sh`, and `count_641` is in the same per-profile FAIL condition as
> #576 and #626 — an attribution, never a tolerance. **This run is not re-scored.** It measured
> FAILED at 49/50 and is reported as FAILED at 49/50 here, permanently; the bucket only changes what a
> FUTURE occurrence is called. Controls for the key are in `../pr3-round5-gate-logs/`.

The red is not on any path this round changed, and that is checkable rather than asserted:

* the whole 50-boot run reports `Resume PC refused: 0 marker line(s) across 0/50 boot(s)` and
  `RET dispatch refused: 0 marker line(s) across 0/50 boot(s)`, so no refusal record was ever
  published;
* boot 21's serial contains **zero** `[RESUME_PC_CUSTODY:` lines and **zero** `[RESUME_PC_REFUSED:`
  lines, so `drain_asm_resume_pc_refusals` never entered a record body at all. Every line round 4
  changed lives inside that body.

Rate, stated plainly rather than argued: round 1 saw this signature once in 330 boots; round 4 saw
it once in 50. Both are single occurrences and neither pins a rate.

The failing serial is preserved at
`docs/planning/teardown-unification/pr3-round4-serials/ss25-cortex-a72-boot21-641-far0x2-esr0x96000005.txt`.
