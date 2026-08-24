# Gate baseline — `main` @ `f96ea36c`

**Tree measured:** `main` @ `f96ea36c7c9d4477127f8dab2b584cca5dc7fa97` (merge of PR #648), clean
working tree, no code changes made for any leg.
**Measured:** 2026-08-23 evening into 2026-08-24 early morning ET.
**Purpose:** discharge exit items **E1** (measure the full gate matrix once and write the numbers
down) and **E2** (one 200-boot service-sequence battery) from
`GREENGATE-EXIT-ASSESSMENT` — the T3-G green-gate exit assessment, §5.

**This document records measurements and states what they support. It actions no adjudication.**
No issue is closed, no close-retake is filed, no rate is edited in any issue by this document.
Those are separate adjudications; §4 says only what the data supports and what it does not.

---

## 1. Per-gate verdict and rate

| # | Gate | Command | Verdict | Rate | Reds |
|---|---|---|---|---|---|
| G1 | aarch64 kernel-merge (strict) | `docker/qemu/run-aarch64-boot-test-strict.sh 20` | **GREEN** | 20/20 (100%) | none |
| G2 | aarch64 full-test / refusal-drain | `docker/qemu/run-aarch64-full-test.sh --boot-tests-only --rebuild` | **RED** | Phase 1 107/107 PASS, Phase 1b PASS, **Phase 1c FAIL**, Phase 5 PASS | 1 → #610 |
| G3 | aarch64 production-profile | `docker/qemu/run-aarch64-prod-profile-boot-test.sh` | **GREEN** | 1/1 boot | none |
| G4 | beast x86 boot-parallel | `docker/qemu/run-boot-parallel.sh 5` × 4 batches | **RED (partial)** | 18/20 (90%) | 2 → #608, #636 |
| G5 | aarch64 service-sequence (E2 battery) | `docker/qemu/run-aarch64-service-sequence-gate.sh --profile both --boots 100 --rebuild` | **GREEN** | **200/200 (100%)**, every gate-failing bucket 0 | none |

The aarch64 starved gate (`run-aarch64-boot-test-native.sh` under host hogs) was **not** in the E1
list and was not run. It remains the unmeasured gate at this SHA; #555 / #536 / #586 / #623 / #624 /
#638 are its residents and this baseline says nothing about them.

### 1.1 G5 — the 200-boot battery in detail

100 boots per profile, `boot_tests` feature, target `aarch64-breenix-kernel.json` (soft-float),
`scripts/check-kernel-no-neon.sh` clean (0 FP/SIMD load/stores in kernel `.text`) before the run.
Single sequential QEMU throughout, ~59 min wall clock, one clean pass with **no re-runs for any
reason**.

| Bucket | max | cortex-a72 | total |
|---|---|---|---|
| 575 / 576 / 626 / 635 / 641 | 0 | 0 | 0 |
| DATA_ABORT / CLONE_EXEC / STRAND | 0 | 0 | 0 |
| BOOT_TEST_FAIL / 596 / 612 / 609 / P5B | 0 | 0 | 0 |
| RESUME_PC_REFUSED / PERCPU_STACK_ALIEN / CPU_IDENTITY_SPLIT / RET_STAGE_REFUSED | 0 | 0 | 0 |
| **UNATTRIBUTED** | **0** | **0** | **0** |
| **GREEN** | **100/100** | **100/100** | **200/200** |

Reported-not-gated counters (informational by ruling; they did not affect any verdict):

| Counter | max | cortex-a72 | total |
|---|---|---|---|
| `CTX596` ELR divergence | 25 lines | 36 lines across 30/100 boots | 61 lines across 53/200 boots |
| `RET_DISPATCH_REFUSED` | 0 | 0 | 0 across 0/200 boots |
| Saved-LR non-PC words (`LR_NONTEXT`) | 452 lines | 476 lines | 928 lines across **200/200 boots** |

---

## 2. Every red, and what it attributes to

| Red | Where | Observed signature | Attributed to | Basis |
|---|---|---|---|---|
| R1 | G2 Phase 1c (`clonevm_exec_test`) | `CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed` (causal, program line 278), then `ERROR parent wait was not woken by sibling` (line 407, what the gate quotes) | **#610** | Exact match to the filed TOCTOU in the *test program* — the sibling asserts `futex_wake == 1` before the parent is guaranteed blocked. The parent's line-407 message is the downstream consequence, not a second defect. Ruled out #589 (closed; its shape is a 30 s timeout with no ERROR line) and #608 (x86-only, different symptom). |
| R2 | G4 beast batch 2, test 2 | `USERSPACE TEST COMPLETE was absent; boot did not finish`, with `sys_read: fd=140737454784552, buf_ptr=0x1, count=0` repeating **153,107 times** | **#608** | The garbage-argument `sys_read` spin of the second-stage `CLONE_VM` child, named line-for-line in #608's filing. Deliberately **not** attributed to #630 despite the matching outer wording: #630's filing preserves no serial and names no stuck-syscall signature, so the specific positively-matching issue takes the attribution. |
| R3 | G4 beast batch 3, test 1 | `TEST_TALLY: exited=19 nonzero=2 failed=[loopback_wake_test_child:15,loopback_wake_test:1]`; `LOOPBACK_WAKE_TEST: eof wait_ms=9757 bytes=0`; `[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]` | **#636** | Fail tag, ~9.5 s EOF wait, zero bytes and tally shape all reproduce #636's filed signature verbatim. |

**Unattributed reds: none.** E1's blocking condition — *any red that cannot be attributed to a filed
issue blocks exit, because an unattributed red is an unknown defect rather than a known one* — is
**not triggered** at this SHA. No new issue needed to be filed by this measurement.

Serials preserved in-repo (the scratchpad copies are ephemeral):

- `docs/planning/t3g-prb/serials/e1-aarch64-fulltest-phase1c-610-f96ea36c.txt` (full G2 serial, R1)
- `docs/planning/t3g-prb/serials/e1-x86-batch2-sysread-spin-608-f96ea36c-excerpt.txt` (bounded
  excerpt of R2's 13 MB kernel serial: the spin onset and the tail; the 153k repeats are elided and
  the count is recorded in the header line)
- `docs/planning/t3g-prb/serials/e1-x86-batch3-loopback-eof-636-f96ea36c.txt` (full userspace serial, R3)

G5 produced no specimens, so there is nothing to preserve from it.

---

## 3. The E2 pre-registered reading, applied

E2 pre-registered two branches:

> **(a)** all gate-failing buckets zero — accept the #635 specimen-family closure, give #606 a
> close-retake, restate #641 at its measured ceiling, carry #576/#626 forward with the residual
> written out explicitly; or **(b)** the run produces specimens, in which case the arc has a live
> target and the exit recommendation is void.

**The data selects branch (a).** Every gate-failing bucket read 0 across 200 boots and
`UNATTRIBUTED` read 0, so no specimen exists to select branch (b). Branch (a) is selected on the
observation, not on a judgement call.

### 3.1 The arithmetic, per defect

`P(0 occurrences in N boots | rate p) = (1-p)^N`. `N₉₅ = ln(0.05)/ln(1-p)` is the sample size at
which a clean run becomes surprising (5%) if the defect were unchanged. `N` below is the number of
boots in this battery that could have sampled the defect — 200 for profile-agnostic signatures, 100
for signatures filed on one CPU model only.

| # | Filed rate | Applicable N | P(0 \| unchanged) | N₉₅ | Reading |
|---|---|---|---|---|---|
| **#606** | 8% (4/50) | 200 | **5.7 × 10⁻⁸** | 36 | **Refuted.** The filed rate cannot survive this run. |
| **#635** (bucket) | 1.5% (3/200) | 200 | **0.049** | 198 | First sample that is surprising under "unchanged". Supports the specimen-family closure — with §3.2's caveat. |
| #640 | 2% (1/50) | 200 | 0.018 | 148 | Surprising under "unchanged", but #640 has never been the arc's target and rests on one observation. |
| #626 | 4% (1/25, max only) | 100 max | 0.017 | 73 max boots | Surprising under the **point estimate**, silent at the low end of its own n=1 interval: `P(0 \| 0.1%, 100) = 0.90`. Not a close. |
| #625 | 4% (1/25, a72 only) | 100 a72 | 0.017 | 73 a72 boots | Same shape as #626. |
| **#576** | 1.25% (~1/80) | 200 | **0.081** | 238 | Consistent with unchanged. **Not a close.** Residual carried forward verbatim. |
| **#641** | 1.0% (1/100) | 200 | **0.134** | 298 | Consistent with unchanged. Restate at ceiling, do not close. |
| #644 | ~2% (1/50, a72) | 100 a72 | 0.133 | 148 a72 boots | Uninformative. #644 is unfixed and will red roughly 2% of future a72 batteries. |
| #622 | ~1% | 200 | 0.134 | 298 | Uninformative. |
| #613 | 0.5% | 200 | 0.367 | 598 | Uninformative. |
| #612 | 0.25% | 200 | 0.606 | 1197 | Uninformative. |

For #641 specifically, "measured ceiling" in numbers: pooling its one filed occurrence with this
clean 200 gives 1 event in ~300 boots (point estimate 0.33%); the rule of three on this battery alone
puts a 95% upper bound of ~3/200 = **1.5%** on its current rate. Neither number is zero and neither
is a close.

### 3.2 What branch (a) does **not** license

- **The #635 bucket is field-keyed, and the live #635 face is not in it.** The bucket keys on
  `ESR = 0x8600000e` with `FAR == ELR` at a kernel address. The live face documented during PR-A —
  a corrupted whole context / live `x30` restored by an ordinary `ret` — is invisible to it and
  lives inside the **census-only** `LR_NONTEXT` signal, which fired **928 lines across 200/200
  boots** in this very battery. A zero in the bucket is evidence about the specimen family; it is
  not evidence about the live face, whose producing write is still unpinned. Any closure taken on
  this data must be scoped to the specimen family and say so.
- **#605 is invisible to this gate** (counter only, no oracle, no bucket; filed at 3–28 events per
  boot on 16/16 boots). 200 green boots say nothing whatsoever about it.
- **`CTX596` divergence (53/200 boots) and `RET_DISPATCH_REFUSED` (0/200) are reported, not gated.**
  Their values here are observations, not verdict inputs.
- **This is one configuration, not a replication.** 200 boots, one host, one profile (`boot_tests`),
  two CPU models, one sequential pass. It is the largest single sample the campaign has taken and it
  is exactly what E2 asked for; it is not two independent 100-boot samples, and the `testing` profile
  (#562, red 5/5) and the `-smp 1` configuration (#620/#617) are sampled by no gate here.
- **The standing rule survives the green.** A future *50*-boot battery remains arithmetically silent
  about #635, #576, #626 and #641 — `P(0 | 1.5%, 50) = 0.47`. Quote this table rather than re-deriving
  it.

### 3.3 The E1 gates, read with the same arithmetic

- **G1 GREEN 20/20 is the first re-measurement of #599** since it was filed at 3–5/20 on 2026-08-18,
  pre-#632/#634/#642/#645. `P(0 | 15%, 20) = 0.039`; `P(0 | 25%, 20) = 0.003`. That is consistent
  with #599 having been repaired by the intervening work and is **not proof** — one run, and the
  gate's own standard is 100% of 20, which it met. #627 (~1/20 inside the same gate) did not fire:
  `P(0 | 5%, 20) = 0.36`, i.e. no information.
- **G2's Phase 5 passed, so #646 (2/6 on `main` @ `102317b4`) did not reproduce in one look.**
  `P(0 | 33%, 1) = 0.67` — no information. The gate aborts at the first failing phase, and Phase 1c
  (R1/#610) fires before Phase 5, so this pass could take exactly one sample of the refusal-drain
  oracle. #646 stands unrefuted.
- **G3 GREEN 1/1**: #598's live 1/25 did not fire. `P(0 | 4%, 1) = 0.96` — a data point of zero, not
  a measurement.
- **G4 at 18/20**: #630 (filed 2/20 = 10%) did not appear in this independent 20-boot sample.
  `P(0 | 10%, 20) = 0.12` — entirely expected under "unchanged"; #630's 10% stands. #554, #629 and
  #631 likewise did not fire and are likewise untouched by this sample.

---

## 4. What the data supports — stated, not actioned

Each line below is an adjudication for someone else to make. This document takes none of them.

| Item | What the data supports | What it does not support |
|---|---|---|
| **#606** (8%, boot wedge) | A close-retake. `P(0 \| 8%, 200) = 5.7 × 10⁻⁸`: either the defect was repaired by the #609/#635 work or the filed 8% was badly over-estimated. | Closing it silently. The retake should say which of those two it believes and on what basis. |
| **#635** | Accepting the **specimen-family** closure — the `ESR 0x8600000e`, `FAR == ELR` bucket at 0/200 with `P(0 \| 1.5%, 200) = 0.049`. | Closing #635 outright. The live corrupted-`x30` face is census-only-visible and unaddressed (§3.2). |
| **#641** | Restating the issue at its measured ceiling (~0.33% pooled, 95% upper bound ~1.5%). | A close. `P(0 \| 1%, 200) = 0.134`. |
| **#576**, **#626** | Carrying forward with the residual written into the issue: `P(0 \| 1.25%, 200) = 0.081` for #576; `P(0 \| 4%, 100 max) = 0.017` for #626 against a point estimate that rests on one observation. | A close for either. |
| **#599** | Recording this 20/20 on the issue as a first clean re-measurement. | Closing it on one run. A second clean 20 would make the case. |
| **#646**, **#598**, **#630**, **#554**, **#629**, **#631**, **#605**, **#644**, and the starved-gate residents | Nothing. Not sampled, or sampled far below their N₉₅. | Any inference at all. |
| **#610**, **#608**, **#636** | Each reproduced at this SHA with a preserved serial; each is live on `main`. | — |

---

## 5. Remaining exit gates

E1 and E2 are discharged by this document and the runs it records. **E3 and E4 remain.**

### E3 — repair the two detector defects that shipped knowingly disclosed

Both are detector blindness, and forward phases will read green off these ratchets.

**(a) PR #648 F2 — `item_path_is_test_fixture` substring-tests for `test`.** It matches the literal
inside `#[cfg(...)]`, so `#[cfg(not(test))]` **production** items are wrongly exempted from the
derived blocking-state rule. Proven at merge time with a planted `park_probe`: census A caught it as
a new row, the class rule did not fire. It was disclosed in the PR body, **not fixed and not filed**.
*Acceptance:* replace the substring test with a positive predicate — require `test` **enabled**, not
merely present — proven by the planted `#[cfg(not(test))] fn park_probe` firing the **derived class
rule**, not merely census A's re-anchor.

**(b) #643 — two ERET consumers with no admission arm**, including the `boot.S` `tpidr_el1 == 0`
path, and a unification ratchet that catches only a re-implemented private admission pair and can
never see a newly added bare ERET. *Acceptance:* either give the two consumers an admission arm or
teach the ratchet to see a bare ERET, with a mutation that reddens when an unguarded ERET is added.

**(c) PR #648 F3** — two blocking-state publication shapes no census sees (struct-literal
`Thread { state: ThreadState::Blocked, .. }`; import-aliased `ThreadState`). Neither exists in the
tree today. If not fixed, it gets a filed issue. The same applies to F2 if it is not repaired: a
disclosed-but-unfixed violation is not someone else's problem.

### E4 — operator ruling on x86's disposition (a decision, not work)

This baseline sharpens the inputs rather than changing them. x86 measured **18/20 (90%)** at
`f96ea36c` with both reds attributed (#608, #636). #630's filed 10% is **unrefuted** by this sample
(`P(0 | 10%, 20) = 0.12`). #554 (boot thread wedges in the RING3_SMOKE disk read holding the PM lock;
the read itself misses 22/22) and **#540 (no x86 gate mode exercises teardown at all)** are untouched
by any leg here. The ruling required is one of:

- **(a)** x86 reliability becomes its own named successor arc;
- **(b)** x86 gates are explicitly demoted to non-blocking, with the reds disclosed on every
  subsequent PR;
- **(c)** #540 folds into P6a's scope, since P6a's x86 leg otherwise has nowhere to run.

P6a walks straight into #540 either way; leaving x86 implicit hands the next phase an unowned red.

### Deliberately not exit gates

#635 / #576 / #626 / #641 and the low-rate abort tail stay **open and gate-failing**. They do not
gate the exit precisely because the gate now fails on them with no tolerance and no rate ceiling, and
their rates sit below what any affordable battery resolves. #647 stays carried to P9 by the #580
scoped close. #605 is worth an early forward-phase look as a correctness question, not a gate
question.

---

## 6. Method and provenance

- **No code changes** were made for any leg, on any host. Both hosts were confirmed free of QEMU
  processes at the end (`pgrep` empty on the Mac and on beast). No Parallels VM was started for this
  work (Parallels is not in the E1 gate list), so there was none to stop.
- **Mac legs** ran strictly sequential, one QEMU at a time. **Beast leg** ran in Incus container
  `breenix-x86` against `/root/bx-main`, a worktree hard-reset to `origin/main` at the same SHA.
- **Disclosed infra fix (beast, not a repository change and not a re-run to erase a red):** the
  worktree's `rust-fork` symlink pointed at a stale Mac path
  (`/Users/wrb/fun/code/breenix-parallels/rust-fork`), failing the first build with
  `ERROR: forked Rust library not found`. It was repointed to `/root/breenix/rust-fork-real`. The
  kernel then built cleanly, userspace was rebuilt (141 binaries) and both test disks regenerated
  before any boot batch ran. The four batches recorded above are the measurement; nothing in them was
  re-run.
- **Evidence of record.** G5's gate directory is
  `/tmp/breenix_aarch64_service_sequence_gate_20260824T015303Z-62010` (host-local, ephemeral; 200/200
  green, no specimens). G4's batch transcripts live on beast at `/root/bx-batch{1,2,3,4}.log` with
  full serials at `/root/bx-batch2-fail-serial.txt` (13 MB) and `/root/bx-batch3-fail1-user.txt`.
  **Correction to the E1 write-up:** its Mac-side pointer `scratchpad/580debt3/e1-beast.log` is a
  stub containing only the string `BATCH_1` — it is not a batch transcript, and the beast-host paths
  above are the evidence. The two small failing serials have been copied into
  `docs/planning/t3g-prb/serials/` (§2) so this baseline does not depend on ephemeral scratch space.
