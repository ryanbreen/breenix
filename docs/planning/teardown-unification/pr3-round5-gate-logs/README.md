# T3-G PR3 round-5 gate logs

Captured on the shipped round-5 tree of `fix/producer-corruption-family`. Ruling **R49** closed the
round-4 audit on four micro-items and set this round's gate list: **boot-tests + structural suites +
strict 1×20, and explicitly NO service-sequence re-run**. These are its outputs, kept here so the
acceptance table in PR #642 can be checked against something durable.

R49's reasoning for the short list is recorded rather than assumed: the round-5 diff is one Rust
reason code on a path that cannot be reached on a booted kernel, one service-sequence classifier
bucket that changes attribution and not any FAIL condition, and two doc corrections. The numbering
follows round 4's (`01`, `02`, `03`, `04`, `05`); `06`/`07`+ (x86 and the ratchet mutation runs) are
round-3/round-4 artifacts and were not in R49's list, so those copies stand.

| file | what it is |
|---|---|
| `01-aarch64-prod-build.txt` | aarch64 **production**-profile build (no features), `aarch64-breenix-kernel.json` (the soft-float kernel target — never the NEON one, per the #528 re-arm rule), after a forced `cargo clean -p kernel --release`. `EXIT=0`. Zero PROJECT warnings: the only `warning:` line is the pre-existing toolchain `core` future-incompatibility note, which is not project code. |
| `01-aarch64-boot-tests-profile-build.txt` | the same forced-clean build in the `--features boot_tests` profile — the kernel every gate below boots. `EXIT=0`, same single toolchain note, no project warnings. |
| `02-aarch64-boot-tests.txt` | `run-aarch64-full-test.sh --boot-tests-only --rebuild`. **PASSED, 107/107**, including **Phase 5** (`run-aarch64-refusal-drain-gate.sh`): `ARM64 REFUSAL DRAIN GATE: PASSED`, leg G `:PASS]`, leg H `:PASS]`, `[BOOT_TESTS:PASS]`, `[BLOCK_EINTR_ORACLE:PASS:…]`. Leg H is the live evidence that round 5 did not disturb the drain's decision: `refusals=1:victim_present=1:victim_terminated=0:still_current=1:ptr_nulled=0:PASS`. |
| `03-structural-suites.txt` | the twelve structural suites on the final tree: **306 passed, 0 failed** — the same tally as round 4, with the round-4 departure and foreign-drain ratchets included. |
| `04-641-classifier-controls.txt` | the #641 bucket's controls (below). |
| `05-strict20-summary.txt` | strict boot gate, 1×20: **20/20, 100%** (208 s), including its `kernel_no_neon_guard` preflight — 0 FP/SIMD in kernel `.text`. |

## The #641 bucket is an attribution, not a tolerance

Round 4's service-sequence red was `[DATA_ABORT] FAR=0x2 ESR=0x96000005 DFSC=0x5 from_el0=0` with an
ELR in kernel text — field for field the filed, open #641 — and round 4 deliberately did NOT create a
bucket for it, because adding one in the round that produced the red is the goalpost move the
campaign law forbids. **R49 pre-adjudicated #641 by that field signature and authorized the bucket in
round 5.** What it does and does not do:

* `count_641` is in `run_profile`'s per-profile FAIL condition, alongside #576 and #626. A boot
  carrying this signature failed the profile before the bucket existed (as `UNATTRIBUTED`) and fails
  it now. **The set of runs this gate passes is unchanged.** No tolerance was created, no other
  bucket's behaviour changed, and #635's authorized non-failing attribution is untouched.
* **Round 4's service-sequence run is not re-scored and not re-run.** It shipped FAILED at 49/50 and
  is still reported that way in `../pr3-round4-gate-logs/`. R49's adjudication is a ruling about
  landing this PR, not a change to what the gate said.

### Controls (`04-641-classifier-controls.txt`)

The key is `FAR=0x2` + `ESR=0x96000005` (DFSC is that ESR's low six bits, so a field-exact ESR match
is a DFSC match by construction) + `from_el0=0`, which the arm's own guard already requires, + a
single kernel-text ELR across the WHOLE record set. The ELR is checked separately rather than folded
into the `far esr` signature that #612 and #622 are filed against, so those two are not redefined.

| serial | bucket |
|---|---|
| round-4 `cortex-a72` boot 21 (real) | `641` |
| round-1 clean `cortex-a72` boot 97 (real, the serial #641 was filed from) | `641` |
| round-4 serial with `ELR` rewritten to `0x2` | `UNATTRIBUTED` |
| round-4 serial with its two records disagreeing about `ELR` | `UNATTRIBUTED` |
| round-4 serial with `FAR` rewritten to `0x3` | `UNATTRIBUTED` |
| round-4 serial rewritten to #612's filed `FAR`/`ESR` | `612` |
| the other three preserved battery serials | `635`, `UNATTRIBUTED` (#613 disagreeing records), `BOOT_TEST_FAIL` (#555) — all unchanged |

Both real occurrences are now recorded on #641 itself, with the second one's `[FATAL_REGS]` anomaly
disclosed there: its `spsr` holds a kernel VA rather than a PSTATE word, which is the #639 dump
class showing up on a #641-signature fault. The classifier keys on FAR/ESR/DFSC/from_el0/ELR and
cannot see that, which is exactly why it is written down on the issue.

## The third refusal reason has no runtime leg, and that is stated rather than papered over

`RESUME_PC_DRAIN_SCHEDULER_UNAVAILABLE` / verdict `REFUSED_SCHEDULER_UNAVAILABLE` fires when
`with_scheduler`'s closure never runs, so a claimed refusal record is dropped without a decision.
On a booted kernel the scheduler exists long before any dispatch can refuse, so **no leg can fire
this arm** — no leg is claimed for it, and R49 required no service-sequence re-run on that basis. Its
evidence is: the structural suites stay green (the departure ratchet still dominates every terminate,
`cpu_state` rewrite and pointer-null with the departure test), leg H still refuses and leaves its
victim alive on the round-5 tree, and the counter is published in the same family as the other two
reason codes for GDB and oracle inspection.
