# PR-B stage 3 — the #635 tolerance is removed, and what the battery measured

Branch `fix/prb-producer-custody`, on top of the stage-2 producer repair (`f4ec3e60`).

Two changes to `docker/qemu/run-aarch64-service-sequence-gate.sh`:

1. the `635` bucket keeps its field-keyed classifier arm and loses its non-failing
   exemption — `count_635` is now in `run_profile`'s FAIL condition;
2. a non-zero production `[RESUME_PC_REFUSED:]` count now fails the profile instead of
   being reported.

Both are tightenings. The set of runs this gate passes is strictly smaller than before.

## Verdict of this stage, up front

**The service-sequence battery FAILED**, on a signature unrelated to either change:
one boot of 50 failed the aggregate boot-test score on
`interrupts:timer_interrupt_running`. Filed as **#644**, serial preserved in-repo.

Everything this stage set out to prove came back clean:

| what the change gates | result over 50 boots |
|---|---|
| `635` bucket | **0** |
| production `RESUME_PC_REFUSED` lines | **0** across 0/50 boots |
| `576` / `626` / `641` | 0 / 0 / 0 |
| every other named bucket | 0, except `BOOT_TEST_FAIL` = 1 |

The failing bucket, `BOOT_TEST_FAIL`, was already hard-failing before this change and is
untouched by it.

## 1. Builds — 0 kernel warnings in both profiles

Both on `aarch64-breenix-kernel.json`, each after `touch kernel/src/lib.rs` so the kernel
really recompiled.

```
cargo build --release --features boot_tests \
  --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

```
cargo build --release --features testing,external_test_bins \
  --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

| profile | exit | lines matching `^warning:` or `^error` |
|---|---|---|
| `boot_tests` | 0 | 1 |
| `testing,external_test_bins` | 0 | 1 |

The one line is identical in both and is not this kernel's code:

```
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (/Users/wrb/.rustup/toolchains/nightly-2025-06-24-aarch64-apple-darwin/lib/rustlib/src/rust/library/core)
```

That is the `rust-src` future-incompat notice about the upstream `core` crate, emitted on an
unmodified tree too. **Kernel warnings: 0 in both profiles.**

## 2. `docker/qemu/run-aarch64-service-sequence-gate.sh --rebuild` — FAILED

25 boots per profile across `max` and `cortex-a72` (the defaults), IOPS 2000, 45 s timeout.
Run directory `/tmp/breenix_aarch64_service_sequence_gate_20260823T115457Z-58489`.

### Profile `max` — PASSED

```
Profile max census
  575           0
  576           0
  626           0
  635           0
  641           0
  DATA_ABORT    0
  CLONE_EXEC    0
  STRAND        0
  BOOT_TEST_FAIL 0
  596           0
  612           0
  609           0
  P5B           0
  GREEN         25
  UNATTRIBUTED  0
  GREEN rate: 25/25 (100.0%) — census-only: every non-GREEN bucket is gate-failing, with no exceptions, including the open #576, #626, #635 and #641 defects
  CTX596 divergence: 12 marker line(s) across 10/25 boot(s) — reported, not gated
  RET dispatch refused: 0 marker line(s) across 0/25 boot(s) — reported, not gated
  Resume PC refused: 0 marker line(s) across 0/25 boot(s) — gate-failing
Profile max gate: PASSED (575=0, 576=0, 626=0, 635=0, 641=0, DATA_ABORT=0, CLONE_EXEC=0, STRAND=0, BOOT_TEST_FAIL=0, 596=0, 612=0, 609=0, P5B=0, UNATTRIBUTED=0, RESUME_PC_REFUSED=0)
```

### Profile `cortex-a72` — FAILED

```
Profile cortex-a72 census
  575           0
  576           0
  626           0
  635           0
  641           0
  DATA_ABORT    0
  CLONE_EXEC    0
  STRAND        0
  BOOT_TEST_FAIL 1
  596           0
  612           0
  609           0
  P5B           0
  GREEN         24
  UNATTRIBUTED  0
  GREEN rate: 24/25 (96.0%) — census-only: every non-GREEN bucket is gate-failing, with no exceptions, including the open #576, #626, #635 and #641 defects
  CTX596 divergence: 15 marker line(s) across 11/25 boot(s) — reported, not gated
  RET dispatch refused: 0 marker line(s) across 0/25 boot(s) — reported, not gated
  Resume PC refused: 0 marker line(s) across 0/25 boot(s) — gate-failing
Profile cortex-a72 gate: FAILED (575=0, 576=0, 626=0, 635=0, 641=0, DATA_ABORT=0, CLONE_EXEC=0, STRAND=0, BOOT_TEST_FAIL=1, 596=0, 612=0, 609=0, P5B=0, UNATTRIBUTED=0, RESUME_PC_REFUSED=0)
```

### Total

```
Total census
  575           0
  576           0
  626           0
  635           0
  641           0
  DATA_ABORT    0
  CLONE_EXEC    0
  STRAND        0
  BOOT_TEST_FAIL 1
  596           0
  612           0
  609           0
  P5B           0
  GREEN         49
  UNATTRIBUTED  0
  GREEN rate: 49/50 (98.0%) — census-only: every non-GREEN bucket is gate-failing, with no exceptions, including the open #576, #626, #635 and #641 defects
  CTX596 divergence: 27 marker line(s) across 21/50 boot(s) — reported, not gated
  RET dispatch refused: 0 marker line(s) across 0/50 boot(s) — reported, not gated
  Resume PC refused: 0 marker line(s) across 0/50 boot(s) — gate-failing

ARM64 #575 SERVICE SEQUENCE GATE: FAILED
Non-GREEN boots:
  /tmp/breenix_aarch64_service_sequence_gate_20260823T115457Z-58489/cortex-a72/serial-10.txt: BOOT_TEST_FAIL — boot test failure: [TEST:interrupts:timer_interrupt_running:FAIL:timer tick counter not advancing - interrupts not firing] (qemu_status=0) [early, 19s]
```

### The one non-GREEN boot

Verbatim census row (`cortex-a72/census.tsv`, tab-separated):

```
boot	bucket	end	seconds	ctx596_divergence	reason	serial	ret_dispatch_refusals	resume_pc_refusals
10	BOOT_TEST_FAIL	early	19	2	boot test failure: [TEST:interrupts:timer_interrupt_running:FAIL:timer tick counter not advancing - interrupts not firing] (qemu_status=0)	/tmp/breenix_aarch64_service_sequence_gate_20260823T115457Z-58489/cortex-a72/serial-10.txt	0	0
```

Verbatim classifier reason:

```
boot test failure: [TEST:interrupts:timer_interrupt_running:FAIL:timer tick counter not advancing - interrupts not firing]
```

Preserved in-repo:

```
docs/planning/t3g-prb/serials/prb-stage3-ss-gate-cortexa72-boot10-timer-not-advancing-20260823T115457Z.txt
```

The gate was NOT adjusted in response to this red. Filed as **#644** with the
characterisation below.

**What the failing boot did.** It completed the whole service sequence — `[init] Boot script
completed`, `[spawn] path='/bin/bounce'`, 15 heartbeats after it, all 107 boot tests run with
exactly one failure (`[TESTS_COMPLETE:107/107:FAILED:1]`), zero instruction aborts, zero data
aborts, zero panics, zero resume-PC refusals, zero ret-dispatch refusals. Four other timer
tests passed in the same boot, on both sides of the failure:

```
225:[TEST:timer:timer_init:PASS]
248:[TEST:timer:timer_ticks:PASS]
255:[TEST:timer:timer_delay:PASS]
259:[TEST:timer:timer_monotonic:PASS]
270:[TEST:interrupts:timer_interrupt_running:FAIL:timer tick counter not advancing - interrupts not firing]
352:[TEST:timer:timer_quantum_reset_aarch64:PASS]
```

so the global `TICKS` counter demonstrably advanced elsewhere in that same boot.

**Not established here.** Whether this signature is branch-caused or pre-existing is not
settled by this run, and #644 says so. It appears in no serial preserved in this repository
before today, but no prior campaign preserved a green boot's serial either, so absence is not
evidence. Settling it needs a same-profile baseline soak on `main`
(`-cpu cortex-a72 -smp 4`, IOPS 2000, same boot count). Observed rate on this kernel: 1 in 45
`cortex-a72` boots across this gate and the strict gate below (~2%), 0 in 25 `max` boots.

## 3. `docker/qemu/run-aarch64-prod-profile-boot-test.sh` — PASSED

```
PASS: production profile reached bsshd with the futex oracle seam absent
Observed: [FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe=-110]
Observed: [init] futex_handoff_oracle exited pid=6 code=0
Observed: bsshd: listening on 0.0.0.0:2222
Observed kernel oracle marker count: 0
Observed block EINTR oracle marker count: 2
Observed block EINTR oracle failure count: 0
Observed crash marker count: 0
```

## 4. `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh` — PASSED (no regression)

```
ARM64 PERCPU STACK CUSTODY GATE: PASSED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=0:arm_verified=1:stimuli=1:accepted=0:overwritten=0:pad_intact=1:elr_slot=0xa11e00000000001f:spsr_slot=0xa11e000000000020:overlay=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=1:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=21380:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
[BOOT_TESTS:PASS]
[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1]
```

As expected: the gate edits are shell-side only and cannot affect this gate.

## 5. `docker/qemu/run-aarch64-boot-test-strict.sh` — PASSED

Same `-cpu cortex-a72 -smp 4` profile as the failing service-sequence boot, on the same kernel,
run immediately afterwards:

```
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 206s

=========================================
PASS: 20/20 boots succeeded
=========================================
```

This gate also fails on an aggregate boot-test failure, so those 20 boots are 20 more
`cortex-a72` samples that did not carry the #644 signature.

## 6. `cargo test --tests` — host-side structural suites PASS

`cargo test --tests` exits non-zero and stops at the first failing binary
(`async_executor_tests`), so the run below is `cargo test --tests --no-fail-fast`, which reaches
every binary.

Every host-side structural suite passes:

```
arm64_boot_post_test.rs                    ok. 3 passed; 0 failed; 32 ignored
block_request_lifetime_structure.rs        ok. 12 passed; 0 failed; 0 ignored
context_restore_structure.rs               ok. 77 passed; 0 failed; 0 ignored
dma_and_log_sink_structure.rs              ok. 4 passed; 0 failed; 0 ignored
exec_lock_order_structure.rs               ok. 34 passed; 0 failed; 0 ignored
exit_tally_structure.rs                    ok. 6 passed; 0 failed; 0 ignored
kernel_build_test.rs                       ok. 5 passed; 0 failed; 0 ignored
kernel_no_neon_guard.rs                    ok. 1 passed; 0 failed; 0 ignored
loopback_pump_structure.rs                 ok. 57 passed; 0 failed; 0 ignored
net_lock_structure.rs                      ok. 19 passed; 0 failed; 0 ignored
ring3_enosys_test.rs                       ok. 2 passed; 0 failed; 1 ignored
serial_line_atomicity_structure.rs         ok. 9 passed; 0 failed; 0 ignored
shared_qemu.rs                             ok. 2 passed; 0 failed; 0 ignored
signal_eintr_predicate_structure.rs        ok. 2 passed; 0 failed; 0 ignored
simple_kernel_test.rs                      ok. 3 passed; 0 failed; 0 ignored
stack_bounds_tests.rs                      ok. 17 passed; 0 failed; 0 ignored
strand_handoff_structure.rs                ok. 33 passed; 0 failed; 0 ignored
teardown_structure.rs                      ok. 59 passed; 0 failed; 0 ignored
x86_gate_verdict_test.rs                   ok. 5 passed; 0 failed; 0 ignored
```

`strand_handoff_structure` is 33 tests, up from 31: the two new ratchets in §7.

Twelve binaries FAIL, and all twelve are the x86_64 shared-QEMU integration suites:

```
async_executor_tests.rs   FAILED. 2 passed; 3 failed
boot_post_test.rs         FAILED. 2 passed; 1 failed
exception_tests.rs        FAILED. 2 passed; 2 failed; 1 ignored
guard_page_tests.rs       FAILED. 2 passed; 3 failed
interrupt_tests.rs        FAILED. 2 passed; 4 failed
keyboard_tests.rs         FAILED. 2 passed; 2 failed
logging_tests.rs          FAILED. 2 passed; 3 failed
memory_tests.rs           FAILED. 2 passed; 5 failed
ring3_smoke_test.rs       FAILED. 2 passed; 1 failed
syscall_tests.rs          FAILED. 2 passed; 1 failed
system_tests.rs           FAILED. 2 passed; 2 failed; 2 ignored
timer_tests.rs            FAILED. 2 passed; 4 failed
```

Their failure messages are all of the "the x86 kernel produced no output" shape — `Kernel POST
tests not completed`, `POST completion marker not found`, `Boot step 'Kernel entry point
reached' not found`, `IDT not loaded successfully`, and so on. This is the same environmental
limit recorded in `PRB-STAGE2-RESULTS.md` §"what it cannot prove here": this Mac is ARM-native
and x86 runs on beast. No suite touched by this stage is among them.

## 7. Why the two new structural ratchets exist, and that they bite

`tests/strand_handoff_structure.rs` gained two tests, and two existing ones changed:

- `EXPECTED_NON_FAILING_SERVICE_SEQUENCE_BUCKETS` drops 2 → 1. `GREEN` is now the only
  non-failing bucket the gate has.
- `service_sequence_609_arm_is_field_keyed_and_untolerated` asserts `635` is in the FAILING
  bucket census rather than the non-failing one.
- **new** `service_sequence_resume_pc_refusals_fail_the_profile` — the derived per-profile
  resume-PC line accumulator must appear in the FAIL condition, and the census line must not
  describe itself as ungated.
- **new** `service_sequence_635_arm_is_field_keyed_and_untolerated` — the `635` arm must keep
  every one of its field-signature guard terms (`FAR == ELR`, `FAR != 0x0`, `ESR = 0x8600000e`,
  canonical kernel high-half `^0xffff[0-9a-f]+$`), `count_635` must be in the FAIL condition,
  and no removed-tolerance wording may survive anywhere in the gate.
- `service_sequence_ret_dispatch_refusals_are_counted_and_reported_not_gated` swapped its
  blanket `contains("refusal")` clause for exact `"$name"` terms on the three derived
  ret-dispatch counters. The blanket form cannot survive this change for a mechanical reason:
  `refusal_lines` is a substring of `resume_pc_refusal_lines`, so a substring test can no longer
  tell the two families apart and would read the intended tightening as a ret-dispatch
  regression. The counters are still *derived from the script* by following the data flow out of
  the `grep -cF "[RET_DISPATCH_REFUSED:"` line, so an alias is followed rather than evaded.

Each was mutation-proven singly against the gate script, reverting between mutations:

| mutation | expected red | observed |
|---|---|---|
| drop `[ "$count_635" -ne 0 ]` from the FAIL condition | `..._635_arm_is_field_keyed_and_untolerated` | FAILED (plus the 609 census test, which counts non-failing buckets) |
| drop `[ "$resume_pc_refusal_lines" -ne 0 ]` | `..._resume_pc_refusals_fail_the_profile` | FAILED |
| delete the `ESR = 0x8600000e` guard term from the 635 arm | `..._635_arm_is_field_keyed_and_untolerated` | FAILED |
| re-add the `[ATTRIBUTED, non-failing]` wording | `..._635_arm_is_field_keyed_and_untolerated` | FAILED |
| add `[ "$refusal_lines" -ne 0 ]` (gate ret-dispatch refusals) | `..._ret_dispatch_refusals_are_counted_and_reported_not_gated` | FAILED |

Restored script byte-identical after every mutation.

### Attribution is intact, and was checked against real serials

The point of removing the tolerance without touching the predicate is that a recurrence is still
named. All three preserved `#635` serials from the T3-G PR2 campaign still classify as `635`
under the edited classifier, not `UNATTRIBUTED`:

```
gate-clean100-cortexa72-boot3-stackpc-8600000e.txt   BUCKET=635
gate-clean100-cortexa72-boot37-stackpc-8600000e.txt  BUCKET=635
gate-clean100-max-boot30-stackpc-8600000e.txt        BUCKET=635
REASON=instruction abort matching the #635 kernel-stack-PC family (FAR=ELR=0xffff000054243f00 ESR=0x8600000e) — ATTRIBUTED by field signature, and gate-failing
```

The FAIL condition itself was truth-tabled with every counter zeroed except one:

```
all zero                                        -> PASS
635=1                                           -> FAIL
resume_pc_refusal_lines=1                       -> FAIL
ret_dispatch refusal_lines=7 (must stay PASS)   -> PASS
ctx596 divergence_lines=7 (must stay PASS)      -> PASS
609=1                                           -> FAIL
641=1                                           -> FAIL
UNATTRIBUTED=1                                  -> FAIL
```

## What this stage does and does not close

Closes nothing by itself. It removes a tolerance whose defect was repaired at source in stage 2,
and it turns a production resume-PC refusal into a failure. Over 50 boots this branch produced
zero `635` occurrences and zero production resume-PC refusals — which is what the removal
required and is the evidence a #635 close retake would rest on, at this sample size.

It leaves one red on the floor, #644, honestly failing the gate.
