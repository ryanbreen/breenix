# PR-B stage 2 — results

Branch `fix/prb-producer-custody`, on top of the stage-1 probes (`0a58274a`).

The repair: per-CPU idle/exception stacks now name their owner, the setters
refuse an install naming another CPU's slot, the return-SP install follows the
frame's pending exception level, thread id 0 is retired, and the reclaimed
thread control blocks are freed with interrupts masked.

## 1. Zero warnings, three profiles

All on `aarch64-breenix-kernel.json`, each after `touch kernel/src/lib.rs` so
the whole kernel really recompiled:

```
cargo build --release --features boot_tests \
  --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

| profile | exit | lines matching `^warning:` or `^error` |
|---|---|---|
| `boot_tests` | 0 | 1 |
| `testing,external_test_bins` | 0 | 1 |
| `boot_tests,percpu_stack_custody_oracle` | 0 | 1 |

The one line is identical in all three and is not this kernel's code:

```
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (/Users/wrb/.rustup/toolchains/nightly-2025-06-24-aarch64-apple-darwin/lib/rustlib/src/rust/library/core)
```

That is the `rust-src` future-incompat notice about the upstream `core` crate,
emitted on an unmodified tree too. **Kernel warnings: 0 in every profile.**

## 2. `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh` — PASSED

Serial: `docs/planning/t3g-prb/serials/prb-stage2-custody-gate-green-20260823T113949Z.txt`

```
ARM64 PERCPU STACK CUSTODY GATE: PASSED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=1:arm_verified=1:stimuli=1:accepted=0:overwritten=0:pad_intact=1:elr_slot=0xa11e00000000001f:spsr_slot=0xa11e000000000020:overlay=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=2:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=19777:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
[PERCPU_STACK_ALIEN:cpu=1:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:456]
[BOOT_TESTS:PASS]
[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1]
```

Both refusal records from the serial:

```
[PERCPU_STACK_ALIEN:cpu=1:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:455]
[PERCPU_STACK_ALIEN:cpu=1:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:456]
```

Two records, one per setter — `set_kernel_stack_top` and
`set_user_rsp_scratch` — both naming the probe's stimulus site rather than the
setter, which is `#[track_caller]` working through the whole chain.
`sp=0xffff000044000000` is `percpu_kernel_stack_top(7)`; `owner=unpublished`
because CPU 7 is offline under `-smp 4` and never ran `init_cpu`. Probe A's
image is untouched (`overwritten=0`), so `elr_slot`/`spsr_slot` read back as the
planted pattern words `0xa11e…001f` / `0xa11e…0020`.

EL census:

```
[USER_RSP_SCRATCH_EL_CENSUS:el0_installs=3:el1_skipped=0]
```

**A null result, stated as one.** Three installs this boot, all following a
frame whose SPSR really was returning to EL0; no install was skipped, because no
frame reached one of the two guarded sites with a pending EL1 return. (Earlier
runs of the same gate reported `el0_installs=4:el1_skipped=0` — the count varies
with how many dispatches happen before the verdict; `el1_skipped` was 0 in every
run.) The guard is therefore defensive here rather than firing: what the census
asserts is that the (pending EL, installed SP) pair was correct on every
install, not that the EL1 case was exercised.

## 3. `docker/qemu/run-aarch64-boot-test-native.sh` — PASSED

```
Attempt 1/5...
SUCCESS

=========================================
ARM64 BOOT TEST: PASSED
=========================================
```

Passed on the first attempt; no retry was used.

## 4. `docker/qemu/run-aarch64-boot-test-strict.sh` — PASSED

```
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 207s

=========================================
PASS: 20/20 boots succeeded
=========================================
```

## 5. `docker/qemu/run-aarch64-refusal-drain-gate.sh` — PASSED (no regression)

```
ARM64 REFUSAL DRAIN GATE: PASSED
[RESUME_PC_FOREIGN_ORACLE:aarch64:leg=G:armed=1:planted=1:record_cpu=7:canary_tid=10:canary_progress=33:foreign_reports=1:record_still_published=1:canary_present=1:canary_terminated=0:fatal=0:PASS]
[RESUME_PC_DRAIN_DEPARTURE_ORACLE:aarch64:leg=H:armed=1:fired=1:planted=1:victim_tid=12:canary_is_self=1:refusals=1:victim_present=1:victim_terminated=0:still_current=1:ptr_nulled=0:record_still_published=0:progress=32:fatal=0:PASS]
[BOOT_TESTS:PASS]
[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1]
```

## 6. Host-side structural suites — PASSED

```
block_request_lifetime_structure           test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
context_restore_structure                  test result: ok. 77 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 27.32s
dma_and_log_sink_structure                 test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exec_lock_order_structure                  test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.29s
exit_tally_structure                       test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s
loopback_pump_structure                    test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.26s
net_lock_structure                         test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.45s
serial_line_atomicity_structure            test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
signal_eintr_predicate_structure           test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
strand_handoff_structure                   test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.42s
teardown_structure                         test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 12.03s
kernel_no_neon_guard                       test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.42s
```

`tests/arm64_boot_post_test.rs` also passes (`3 passed; 32 ignored`).

### Three existing ratchets had to change, and why

**`block_request_lifetime_structure::block_eintr_oracle_marker_is_pinned_in_the_gates`**
— failed on arrival, before any of this stage's edits:

```
oracle marker missing from 1 of 8 discovered aarch64 gate scripts: docker/qemu/run-aarch64-percpu-stack-custody-gate.sh
```

The stage-1 gate script was added without the `[BLOCK_EINTR_ORACLE:` marker
every other aarch64 boot_tests gate pins. Fixed by pinning it there too: the
gate now waits for the line, fails on a missing one, and fails on
`[BLOCK_EINTR_ORACLE:FAIL`, exactly as the refusal-drain gate does. Its header
comment also no longer claims the gate fails on purpose.

**`serial_line_atomicity_structure::unlocked_multi_byte_serial_write_census_is_pinned`**

```
left: Err(["+ kernel/src/arch_impl/aarch64/percpu.rs :: fn percpu_stack_install_permitted  (9 occurrences, expected none)"])
```

The refusal record is 9 `raw_uart_str` calls. It cannot take the serial lock: it
runs on the dispatch path from inside the setters themselves. Added as a pinned
anchor with that reason recorded next to it; the census's synthetic-writer
rejection test still passes, so the census is still closed.

**`teardown_structure::kernel_stack_release_ordering_is_structural`**

```
kernel-stack release mechanism: "reclaimed Box<Thread> values are not bound and dropped outside without_interrupts"
```

This is a direct polarity conflict with STEP 5, and it was resolved in STEP 5's
favour on the evidence, not by preference. The #609 RCA
(`docs/planning/teardown-unification/609-RCA-RETRACTION-2026-08-21.md` §2.3,
link 1) names *this exact drop*, "which sits **outside** that function's
`without_interrupts` block", as how a holder of `ARM64_STACK_BITMAP` became
preemptible. The old ratchet pinned the pre-#609 shape. Two things make masking
safe now: `ARM64_STACK_BITMAP` is an `IrqSafeMutex` since #632, so it cannot be
orphaned and the masked wait is bounded; and `idle_loop_arm64` still calls
`run_deferred_reclamation()` *before* its `msr daifset, #0xf`, so #632's
"reclamation out of masked idle" is untouched — this change masks only within
the reclaim call.

`check_reclaimed_threads_drop_after_unlock` now pins the invariant that matters:
the drop is inside the `without_interrupts` argument span AND after the
scheduler lock guard's scope closes. Both halves were proven non-vacuous by new
rejection cases in `kernel_stack_release_validator_rejects_unsafe_mutations` —
one for the pre-#609 shape (drop after the masked region returns) and one for a
drop still inside the guard's scope — each asserting the exact error string.

### `cargo test --tests` in full, and what it cannot prove here

`cargo test --tests` exits non-zero on this machine: `async_executor_tests`
reports 3 failures, all of the form

```
thread 'test_async_executor_starts' panicked at tests/async_executor_tests.rs:12:5:
Kernel POST tests not completed
```

These are x86_64 shared-QEMU integration tests. They are not caused by this
work, and they cannot pass on this host:

- an unmodified checkout of the stage-1 base `0a58274a` in a clean worktree
  cannot even build that target — the `x86_64` crate v0.15.5 fails against the
  pinned nightly with six `E0407` errors (`method 'forward_overflowing' is not a
  member of trait 'Step'`). The main tree only links it from stale cache;
- this Mac is ARM-native; x86 builds and tests run on beast.

**Not verified here, stated plainly:** the reclaim restructure in
`kernel/src/task/scheduler.rs` and the `#[track_caller]` on the x86_64
`set_kernel_stack_top` impl are shared-code changes that no x86 run in this
session exercised. They need an x86 boot on beast before this branch is
considered proven on both architectures.
