# Round 5 — the aarch64 boots the two producers were missing, and the control that dates the derived bound

Round-5 review findings R4-10 and R4-2 both asked for evidence that did not
exist rather than for a sentence to be reworded. This directory is that
evidence.

## `aarch64/` — R4-10, the two scheduler producers booted on the other arch

`Scheduler::block_current_for_timer` and
`Scheduler::block_current_for_io_publish` are compiled on both arches, and
round 4 changed what both write into `thread.blocked_in_syscall` on the
strength of x86 boots plus one 5-line aarch64 COMPILE transcript. Outside
`kernel/src/task/scheduler.rs` the public entry points have 4 call sites --
`kernel/src/task/waitqueue.rs:76` and `:120`,
`kernel/src/task/completion.rs:323`, `kernel/src/net/tcp.rs:1497` -- which is
the waitqueue sleep path plus the `Completion` device-I/O path the AHCI and
virtio-blk drivers reach through.

Both gates ran on the ARM Mac against the kernel source of commit `645bab38`.
A diff of `kernel/` between that commit and the head adding these captures is
empty, so the source gated is the source shipped. At most 2 QEMUs ran at a
time.

| gate | command | boots | result |
|---|---|---:|---|
| strict | `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` then `docker/qemu/run-aarch64-boot-test-strict.sh 10` | 10 | `Successes: 10  Failures: 0  Success rate: 100%  Duration: 118s`, `PASS: 10/10 boots succeeded`, exit 0 |
| production profile | `docker/qemu/run-aarch64-prod-profile-boot-test.sh`, run 5 times | 5 | exit 0 on 5 of 5, each `PASS: production profile reached bsshd with the futex oracle seam absent` |

**Red attribution: there are 0 reds, so there are 0 to attribute.** The
attribution set the ruling names is #555, #576, #626, #586 and #609, and 0 of
those signatures appear. What was run over the 15 committed captures:

```
grep -ilE 'KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup' \
  aarch64/strict/boot*/serial.txt aarch64/prod-profile/boot*/serial.txt
```

0 of 15 files match. 10 of 10 strict boots carry `[EXEC_SMOKE:TARGET_OK]` once
and `TESTS_COMPLETE` twice. The only `softirq` and `timer_delay` strings in the
15 captures are that suite's own `[TEST:...:START]` and `[TEST:...:PASS]`
markers -- 10 each of `softirq_aarch64` and `softirq_registration`, and the
`timer:timer_delay` pair -- not the #555 or #536 signatures they would
otherwise resemble.
<!-- claim-lint:ok: the 0-of-15 and the 10-of-10 counts are that grep and
     `grep -c` over the captures in this directory, run at the head that
     commits them. -->

Layout: `aarch64/strict/gate.txt` is the gate's own transcript with
`boot{1..10}/serial.txt` beside it; `aarch64/prod-profile/boot{1..5}/` carries
a `gate.txt` and a `serial.txt` each; `builds/aarch64-boot-tests.txt` is the
build the strict gate consumed.

## `x86/control-round3-head/` — R4-2, the control that motivated the bound

Finding R4-2 pointed at
`../round4/boot-replay/control-round3-head/gate-2.txt`, a `GATE: PASS` boot at
the round-3 head whose census line reads:

```
strand census: latest snapshot seq=14 tick=9919 at 49727 ms; 14 valid snapshot(s), previous 7731 ms earlier, largest gap 23473 ms
```

That last snapshot is the last in the capture and arrived 7731 ms after the one
before it. If it is the completion-site snapshot -- which is what a boot
reaching `USERSPACE TEST COMPLETE` emits -- the age at the marker was 7731 ms:
a FAIL under round 4's 5000 ms bound and a PASS under the derived 15000 ms one.
**That inference cannot be checked from anything committed.** Those serials
were overwritten on the beast host before they could be recovered, exactly as
`../round3/case-d-broad-removal/` discloses for its own 18 rows.

So round 5 re-ran the control instead of arguing about it: same head
`3495c3f3`, same container `/root/breenix-775` on beast `breenix-x86`, same
command `docker/qemu/run-x86-gate.sh 1 full`, twice, one QEMU at a time. Both
boots are committed in full.

| boot | verdict | census markers | age at the completion marker |
|---:|---|---:|---|
| `boot1/` | `GATE: PASS (1/1 boot tests passed; mode=full build=14s boot=150s total=172s)` | 108 | `435 ms (newest cadence snapshot seq=21 at 50610 ms, completion snapshot seq=22 at 51045 ms, bound 15000 ms)` |
| `boot2/` | `GATE: PASS (1/1 boot tests passed; mode=full build=14s boot=151s total=173s)` | 106 | `805 ms (newest cadence snapshot seq=18 at 52583 ms, completion snapshot seq=19 at 53388 ms, bound 15000 ms)` |

Two things worth saying plainly about that table.

**It does not reproduce the boot R4-2 pointed at.** The archived transcript
carries 14 snapshots with a 7731 ms tail gap; these two carry 108 and 106 at a
roughly 1 s cadence. Same head, same host, same accelerator, 2 attempts. The
sparse-cadence boot is real -- its transcript is committed -- but this round
offers 0 explanations of it and re-derives 0 of its numbers.

**The census these two run under is not the one their gate transcript
printed.** Each `gate.txt` was produced by the round-3 script and reports
`lines=5377` and `lines=4973`; the script at this head reports 5378 and 4974 on
the same bytes. That 1-line difference is the round-3 R3-12 residual: the old
script streamed the captures through `cat`, which joins a file not ending in a
newline to the next one. Passing them as awk operands is what fixed it, and is
also what made the age line argument-order independent (finding R4-6).

## `builds/`

Transcripts at the pushed content. The three build commands were each forced to
recompile by touching `kernel/src/task/dispatch_strand_census.rs` first.

| command | where | result |
|---|---|---|
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | beast `breenix-x86`, at `7f319a1c` | `Finished ... in 16.32s`, exit 0, 0 lines matching `^(warning\|error)` |
| `cargo build --release --bin qemu-uefi` | beast `breenix-x86`, at `7f319a1c` | `Finished ... in 16.07s`, exit 0, 0 such lines |
| `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | ARM Mac | `Finished ... in 6.78s`, exit 0, 1 such line |
| `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json ...` | ARM Mac | `Finished ... in 7.58s`, exit 0, 1 such line |

The 1 warning line on each aarch64 build is the pinned nightly's
future-incompatibility notice for `core v0.0.0` in the toolchain's own source
tree, printed verbatim in the transcripts. It names 0 Breenix crates.

`structure-tests-targets.txt` is the `tests/*_structure.rs` family, one target
per invocation: 26 targets, 26 exit 0, 505 passed, 0 failed, 0 lines matching
`^(warning|error)`. `verdict-test.txt` is `tests/x86_gate_verdict_test.rs`
separately: 20 passed, 0 failed, up from round 4's 17 because this round adds
the truncated-capture arm (R4-5), the argument-order arm (R4-6) and the
silent-census arm found by breaking the tool.
