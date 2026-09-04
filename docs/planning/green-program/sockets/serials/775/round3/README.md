# Round 3 — the census heartbeat on the idle loop x86 actually runs

The 4 captures here were produced on the beast `breenix-x86` VM in
`/root/breenix-775`, one QEMU at a time, at the round-3 head of
`fix/775-dispatch-log-removal`. The gate boots are
`docker/qemu/run-x86-gate.sh 1 full` (150 s per boot); the production boots are
`docker/qemu/run-x86-prod-profile-boot-test.sh`, which builds with no
`--features` flag at all.

Round-2 finding N1 was that the idle-side heartbeat was wired into
`main.rs`'s `idle_thread_fn`, whose body is not dispatched on x86 at all. It now
lives at the top of `context_switch.rs::idle_loop()`. These four directories are
the measurement of that, and of the channel change (N8).
<!-- claim-lint:ok: N1's own repro counted 0 of 215 round-2 census markers
     preceded by an idle-dispatch breadcrumb; the boot-level consequence is the
     r3-idle-cadence capture below. -->

## `r3-head-green/` — the head, both emitters

`GATE: PASS (1/1 boot tests passed; mode=full build=17s boot=150s total=176s)`,
`x86 userspace gate: PASS - exited=22 expected>=10 nonzero=0 allowlist=0`.

| item | value |
|---:|---|
| census markers, `serial_kernel.txt` (COM2) | 104 |
| census markers, `serial_user.txt` (COM1) | 0 |
| first snapshot | `seq=1:tick=41:ms=550:saved=0:stranded=0` |
| newest snapshot | `seq=104:tick=27817:ms=138760:saved=11:stranded=0` |
| gaps between consecutive snapshots | min 208 ms, mean 1342 ms, max 27017 ms |
| `scripts/x86-strand-census.sh` | `threads_saved_blocked=11 stranded=0 lines=4906`, rc 0 |

The 104-to-0 split is finding N8 closed: the snapshot is on the kernel-log
channel the three removed records used, and no longer on the interactive user
console. The 27017 ms gap is the userspace test phase, where the CPU is
saturated and does not idle; the pump covers the first 43 seconds and the idle
loop covers the rest.

## `r3-idle-cadence/` — the same boot with the pump's heartbeat disabled

`pump-heartbeat-disabled.patch` removes the one call in
`kernel/src/net/loopback_pump.rs`, so 98 of the 98 snapshots in this
capture are idle-driven. This is the no-loopback-emission condition round 3 was asked to
measure. `GATE: PASS`, census rc 0.

| item | value |
|---:|---|
| census markers, COM2 | 98 |
| first snapshot | `seq=1:tick=8648:ms=43183:saved=11:stranded=2:tids=24,36` |
| newest snapshot | `seq=98:tick=27984:ms=139552:saved=11:stranded=0` |
| gaps | min 14 ms, mean 993 ms, **max 1190 ms** |

The first snapshot is at 43183 ms because the CPU does not idle before then:
while the userspace test programs run, a runnable thread is available on each
of the scheduler's picks that the capture records. From the
moment the CPU first idles to the end of the capture — 96 seconds — the largest
gap between snapshots is 1190 ms against a 1000 ms limiter. That is the cadence
the idle hook produces on its own; under the round-2 wiring this capture would
have had 0 idle-driven snapshots.

## `r3-idle-strand/` — the same, plus a deterministic strand

`pump-heartbeat-disabled.patch` again, plus `case-a/deterministic-strand/mutation-E.patch`
(14 lines in `Scheduler::wake_expired_timers` dropping the timer wake for any
thread with `blocked_in_syscall` set). 90 of the 90 snapshots here are
idle-driven and the strand is by construction.

`GATE: FAIL (0/1 passed, 1/1 failed)`, on the strand arm:

```
x86 userspace gate: FAIL - a thread was saved blocked in a kernel wait and was
still not restored at the latest census snapshot (see the strand census above)
```

| item | value |
|---:|---|
| census markers, COM2 | 90 |
| snapshots reading `stranded>0` | 90 of 90 |
| first snapshot | `seq=1:...:stranded=4:tids=24,25,26,36` |
| newest snapshot | `seq=90:tick=27880:ms=139014:stranded=3:tids=24,26,36` |
| gaps | min 1001 ms, mean 1025 ms, max 2004 ms |
| `scripts/x86-strand-census.sh` | rc 1, naming threads 24 `loopback_wake_test`, 26 `futex_handoff_oracle`, 36 `loopback_wake_test_child_22_main` |

This is the pair the N1 fix exists for: with the loopback pump unable to emit,
the idle loop alone published the ledger 90 times over the last 91 seconds of
the boot — the largest gap between two of them is 2004 ms — and named the
stranded threads at the end of it.

## `r3-production/` — the zero-feature shipped profile

5 boots at this head: 5 of 5 printed `PASS: x86 production profile reached
steady state with the teardown census at rest`, and 5 of 5 reported a console
prompt count of `1 -> 2`. The `serial_kernel.txt` and `serial_user.txt` in this
directory are boot 1's; `gate-1.txt` through `gate-5.txt` are the five
transcripts.

Boot 1's markers, both of them, verbatim:

```
[DISPATCH_STRAND_CENSUS:seq=1:tick=4:ms=1033:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
[DISPATCH_STRAND_CENSUS:seq=2:tick=1370:ms=9356:saved=4:stranded=2:tids=4,10:tid_overflow=0:ledger_overflow=0]
```

`seq=1` is the loopback pump's first pass, before init exists — that is the one
marker round 2's production boots carried, and why they read `saved=0`. `seq=2`
is new: it is the idle loop, and it reads real ledger state.

**What this profile does NOT give, stated plainly.** The shipped kernel barely
idles: boot 1 has 1 `<I>` idle dispatch across 2028 `[SW]` context switches, so
the newest snapshot is at 9356 ms of a boot that then ran for another two
minutes. Reading it as a verdict is therefore wrong here, and
`scripts/x86-strand-census.sh` on this capture exits 1, naming thread 4
(`init`) and thread 10 (`exec_smoke`) — both legitimately parked in a syscall
at that instant, not stranded. No in-repo consumer does that: the production
gate (`run-x86-prod-profile-boot-test.sh`) does not call
`scripts/x86-gate-verdict.sh`, and the 3 callers that do run on the test
profile. What this directory measures is census AVAILABILITY in the shipped
build — the module compiles, the emitters run, the marker is well formed on
COM2 — and the cadence measurement is `r3-idle-cadence/`, not this.
<!-- claim-lint:ok: the 5 gate transcripts beside this file are the boots; the
     caller census is ratcheted by tests/dispatch_strand_census_structure.rs. -->

The census now prints the evidence for that judgement itself: on this capture
it reports `latest snapshot seq=2 tick=1370 at 9356 ms; 2 valid snapshot(s),
previous 8323 ms earlier`.

## `builds/`

| transcript | result |
|---|---|
| `x86-testing.txt` | `cargo build --release --features testing,external_test_bins --bin qemu-uefi`, `Finished ... in 15.04s`, exit 0, 0 lines matching `^(warning\|error)` |
| `x86-zero-feature.txt` | `cargo build --release --bin qemu-uefi`, `Finished ... in 15.37s`, exit 0, 0 such lines |
| `aarch64-kernel.txt` | the soft-float kernel target, `Finished ... in 29.97s`, exit 0, 1 such line |
| `structure-test.txt` | `cargo test --test dispatch_strand_census_structure`, 6 passed 0 failed |
| `verdict-test.txt` | `cargo test --test x86_gate_verdict_test`, 14 passed 0 failed |
| `structure-tests-26-targets.txt` | the `tests/*_structure.rs` targets, one per invocation: 26 targets, 26 exit 0, 505 passed, 0 failed |

<!-- claim-lint:ok: the aarch64 warning names only the toolchain's own core
     package; the same established exception is recorded in
     docs/planning/t3g-prb/PRB-STAGE3-GATE-RESULTS.md. -->
The aarch64 warning line is the pinned nightly's future-incompatibility notice
for `core v0.0.0` in the toolchain's own source tree. No warning names a
Breenix crate. Each build was forced to recompile by touching
`kernel/src/task/dispatch_strand_census.rs` first.
