# Per-boot host-side facts for the aarch64 strict and production-profile gates (#827)

**R1, 2026-09-05** is a review round on this doc's own initial 2026-09-05
version: it closes a race where the `(empty)`/`neither` `ended_by` rows
could print `poll_exhausted`/`hard_timeout` even when the boot's own final
verdict was a PASS (F1, both gates), corrects the production gate's
`hard_timeout` comment to admit the poll loop's own `! kill -0` break as a
second path to that label rather than naming the wrapping `timeout 120` as
the only cause (F2), corrects a false "two distinct branches" claim about
`hard_timeout` in the anti-vacuity test's own comment -- true instead, after
F1's fix, of `scored_pass` (F3), corrects a field-count and re-run receipt in
the "Structural suites" section (F4), corrects a paraphrased assertion quote
to the verbatim panic text (F5), corrects an off-by-one in the field-label
doc comment (F6), and quotes an unquoted `$QEMU_PID` in the strict gate to
match the production gate's own style (F7). Detail is folded into the
derivation tables, the mutation record, and the structural-suite section
below, not repeated here.

## What changed

`docker/qemu/run-aarch64-boot-test-strict.sh` and
`docker/qemu/run-aarch64-prod-profile-boot-test.sh` previously scored a boot
from serial content alone (`score_serial`, and the strict gate's own poll
loop and kill are the only host-side control flow around it). #827 found
that leaves 0 of 4 host-side facts on the record: no per-boot wall-clock
window, no host aarch64 QEMU count, no QEMU CPU time, and no record of which
bound (a content check, the poll ceiling, or the outer `timeout`) actually
ended a boot -- so a starved boot and a wedged boot score identically, which
is exactly the ambiguity #826's own 2/40 "Exec smoke did not complete" red
could not be settled without.

A new shared library, `docker/qemu/lib/gate-boot-facts.sh`, gives both gates
one function, `gbf_emit_line`, that prints:

```
[GATE_BOOT_FACTS:boot=N:host_ms=START-END:qemu_at_start=Q:load_at_start=L:qemu_at_end=Q:load_at_end=L:qemu_cpu_s=C:guest_uptime_ms=U:ended_by=E]
```

- `host_ms=START-END` -- host wall-clock start and end, in epoch
  milliseconds, sampled right after the host-wide QEMU lock is acquired
  (start) and again right before this boot's own `kill` (end). Time spent
  blocked on the lock is deliberately excluded from this window: it is the
  wall-clock this boot's own QEMU process actually ran inside, the same
  quantity the guest's own uptime is compared against.
- `qemu_at_start`/`qemu_at_end` and `load_at_start`/`load_at_end` -- the
  host's aarch64 QEMU count (`qemu_host_lock_count()`, already wired for the
  host-wide lock in #826/R181) and 1-minute load average, sampled at the
  same two instants.
- `qemu_cpu_s` -- QEMU's own accumulated CPU time (user+system), read via
  `ps -o time= -p $QEMU_PID` immediately before the kill and converted to
  seconds.
- `guest_uptime_ms` -- the guest's own last `[heartbeat] ... uptime_ms=N`
  line in the serial file.
- `ended_by` -- one of `crash_marker`, `hard_timeout`, `poll_exhausted`,
  `scored_pass`, `scored_fail`, derived from each gate's own existing
  control flow (see "Deriving ended_by" below). No scoring criterion and no
  deadline changed to produce this field.

Both gates write the line into the boot's own evidence directory
(`gate_boot_facts.txt`, alongside `serial.txt`) unconditionally, echo it to
the console, and -- on a failing boot -- also copy it into the preserved
failure directory next to the preserved serial, so a failed boot's host-side
reading is as durable as its serial capture.

## A real defect this branch's own boot proofs found and fixed

The first working version of `gbf_qemu_cpu_seconds` read
`ps -o time= -p "$QEMU_PID"` directly, and reported `qemu_cpu_s=0.00` on
each boot of this fix's own first strict-gate run. The cause: both gates launch QEMU as
`timeout N qemu-system-aarch64 ... &`, and `$!` after that line is coreutils
`timeout`'s own pid, not the qemu-system-aarch64 child it execs --
`docker/qemu/lib/qemu-host-lock.sh`'s own header already documents finding
this same "`timeout` forks a monitor that keeps its own pid alive for the
child's whole life" shape, there for `pgrep -f` double-counting one boot as
two. Checked live on this host while a boot was in flight:

```
$ ps -o pid,ppid,comm -p $(pgrep -x qemu-system-aarch64)
  PID  PPID COMM
95705 95703 qemu-system-aarch64
$ ps -o pid,comm,time= -p 95703   # the timeout wrapper $QEMU_PID actually is
95703 timeout   0:00.00
$ ps -o pid,comm,time= -p 95705   # the qemu-system-aarch64 child
95705 qemu-system-aarc   0:14.86
```

`gbf_resolve_qemu_pid` walks one level down via `pgrep -P "$wrapper_pid" -x
qemu-system-aarch64` before either gate reads CPU time (a caller that
backgrounds qemu-system-aarch64 directly, with no `timeout` in front of it,
is detected via its own `comm=` and returned unchanged). This was found and
fixed while gathering this doc's own boot verification below, not designed
in from the start -- the corrected strict-gate run below reports `qemu_cpu_s`
values of 16-23s over roughly 10-13s wall-clock windows, consistent with a
multi-vCPU TCG guest genuinely burning CPU rather than the wrapper's idle
read.

## Deriving `ended_by`

Neither gate's existing poll-loop body is touched to produce this field --
in the strict gate, an early attempt set an `ENDED_BY_LOOP` variable
directly inside the loop's own guarded `break` branches, which broke a
pre-existing structural ratchet
(`tests/strand_handoff_structure.rs::strict_gate_poll_loop_stops_only_on_crash_or_complete_score`)
that requires each `break` to be immediately preceded by the `if` line that
guards it (a census meant to catch a stray, unguarded break that would
weaken the loop's own scoring behavior). The fix: leave each loop's body
exactly as it already
was, and re-evaluate the *same* predicates the loop uses -- once, right
after the loop exits -- to classify why polling stopped.

**Strict gate** (`run_single_test`), evaluated right after the poll loop,
before the kill:

```
if check_crash_markers(serial):        ENDED_BY_LOOP = crash
elif score_serial(serial):             ENDED_BY_LOOP = early_pass
else:                                   ENDED_BY_LOOP = "" (ran out its 12 iterations)
```

Then, after the kill and the final `score_serial` rescore that already
decides the boot's own SUCCESS/FAIL verdict:

| `ENDED_BY_LOOP` | Final rescore | `ended_by` |
|---|---|---|
| `crash` | (either) | `crash_marker` |
| `early_pass` | pass | `scored_pass` |
| `early_pass` | fail | `scored_fail` -- content written between the early grep and the kill (e.g. a late strand) flipped the rescore; the pre-existing comment block above this loop already documents this as a live possibility |
| (empty) | pass | `scored_pass` -- the mirror race: the loop ran out its 12 iterations (or, on the production gate, saw QEMU die) without either break firing, but content written in the gap between that read and this function's own pre-kill sampling calls (`HOST_MS_END`, the QEMU count/load reads, `ps`/`pgrep`) landed the pass the loop's own classification missed; the final rescore, not the loop's now-stale classification, decides `ended_by`: the case statement checks `SCORE_PASS` before it falls through to `poll_exhausted`/`hard_timeout`, so a pass here lands on `scored_pass`, matching the SUCCESS line printed a few lines down (R1: F1 -- fixed this round; previously this row read `poll_exhausted`/`hard_timeout`, contradicting the SUCCESS verdict) |
| (empty) | fail, QEMU still alive at the kill point | `poll_exhausted` |
| (empty) | fail, QEMU already dead at the kill point | `hard_timeout` -- the `timeout 20` wrapping the launch fired first |

**Production-profile gate**, the same shape against its own two content
checks (bsshd reached, a crash pattern present), evaluated once more right
after its 120-iteration poll loop exits, and its own single-boot `cleanup()`
-- the one function this gate's exit paths converge on, whether via
its own `exit "$status"` calls throughout the assertion chain or the
`trap 'cleanup $?' EXIT` that catches each of them -- reads the loop's
classification alongside the `status` `cleanup` was called with (0 for the
final `PASS:` echo's own `cleanup 0`, 1 for any of the assertion chain's own
`exit 1` sites) to fill in the `early_pass` row's pass/fail split, since this
gate's full verdict is decided by that chain, not by the loop's own bsshd
check alone:

| Loop classification | `cleanup`'s `status` | `ended_by` |
|---|---|---|
| crash pattern seen | (either) | `crash_marker` |
| bsshd reached | 0 | `scored_pass` |
| bsshd reached | nonzero | `scored_fail` -- bsshd was reached but a later assertion in the chain rejected the boot anyway |
| neither | 0 | `scored_pass` -- the mirror race: neither break fired (the loop's own `! kill -0` check caught QEMU exiting on its own, or its 120 iterations ran out), but bsshd's listening line landed in the gap between the loop's exit and the assertion chain that reads `$status`; `$status`, not the loop's stale classification, decides `ended_by` (R1: F1 -- fixed this round; previously this row read `poll_exhausted`/`hard_timeout`, contradicting the assertion chain's own `PASS:` line) |
| neither | nonzero, QEMU still alive at the kill point | `poll_exhausted` |
| neither | nonzero, QEMU already dead at the kill point | `hard_timeout` -- either the wrapping `timeout 120` killed it, or the loop's own `! kill -0` break already caught QEMU exiting on its own for some other reason (a crash that missed `CRASH_MARKERS_PATTERN`, an OOM kill, a triple fault under `-no-reboot` that also failed to print a recognized marker); both leave QEMU dead here, so this label covers more than just the `timeout` wrapper firing (R1: F2 -- corrected this round; the prior comment overclaimed `timeout 120` as "the only thing that does this") |

The facts line (and therefore `ended_by`) is only emitted when this run
actually reached the launch line -- guarded on `PROD_HOST_MS_END` being set,
which stays unset on a path that aborted before boot (a kernel-build
failure, a missing ext2 disk) or, by construction, if `set -e` aborted from
inside the poll loop's own body before this function's own post-loop
sampling ran.

## The ratchet: `tests/gate_boot_facts_structure.rs`

Two properties, each checked against the real files, not merely asserted:

1. `gbf_emit_line`'s own `printf` format string (isolated from the rest of
   the function -- its own local-variable line shares the same substrings as
   the field labels it feeds into that string, so the check is scoped to the
   literal between the format string's own quotes, not the whole function
   body) carries the 10 required field labels named in #827.
2. Both gates set `ended_by` (or `ENDED_BY`) to each of the 5 values #827
   names, somewhere in their own text -- a census over the whole file rather
   than a single presence check, since both gates set this variable from
   more than one branch.

An anti-vacuity test (`gate_boot_facts_predicates_are_not_vacuous`) proves
both census functions actually redden on the real files' own text, in
memory: deleting `qemu_cpu_s=` from the real format string leaves exactly
that one label missing; deleting the real
`ENDED_BY="poll_exhausted"`/`ended_by="crash_marker"` assignment lines
(strict and production gates respectively, one covering each gate's own
uppercase/lowercase local-variable convention) leaves exactly that one value
missing.

### Mutation record (applied to the real, tracked files, then reverted)

```
cmd:  docker/qemu/lib/gate-boot-facts.sh's printf format string edited so
      "qemu_cpu_s=%s:guest_uptime_ms=%s:" reads "guest_uptime_ms=%s:" (the
      qemu_cpu_s= field deleted), then:
      cargo test --test gate_boot_facts_structure gate_boot_facts_line_has_all_required_fields
exit: 101 (test binary FAILED; "test result: FAILED. 0 passed; 1 failed")
assertion: "gbf_emit_line() in docker/qemu/lib/gate-boot-facts.sh is missing
            required GATE_BOOT_FACTS field label(s): [\"qemu_cpu_s=\"] (#827)"
```

```
cmd:  docker/qemu/run-aarch64-boot-test-strict.sh's real
      ENDED_BY="poll_exhausted" line replaced with a no-op (":"), then:
      cargo test --test gate_boot_facts_structure every_kill_path_in_the_strict_gate_sets_ended_by
exit: 101 (test binary FAILED; "test result: FAILED. 0 passed; 1 failed")
assertion: "docker/qemu/run-aarch64-boot-test-strict.sh does not set ended_by
            to: [\"poll_exhausted\"] -- each of #827's 5 ended_by values
            must be reachable from this gate's own poll-loop control flow"
            (verbatim panic text at tests/gate_boot_facts_structure.rs:157,
            reproduced against the real tracked file, not paraphrased)
```

Both mutations were applied to the real, tracked files (not scratch copies),
then reverted; `diff` against a pre-mutation backup was empty before this
branch's own commits were made, and re-running each test after the revert
returned to green (exit 0).

## Boot proofs (2026-09-05)

Kernel built at this branch's own head, in a fresh worktree needing its own
`rust-fork` symlink, prebuilt aarch64 userspace ELFs, and a fresh
`target/ext2-aarch64.img` (not one of these is git-tracked; a worktree has no inherited
`target/` -- the same gaps `HOST-QEMU-LOCK-2026-09-05.md`'s own boot proofs
section records finding):

```
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
-> Finished `release` profile [optimized] target(s)

scripts/check-kernel-no-neon.sh target/aarch64-breenix-kernel/release/kernel-aarch64
-> PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)
```

Host aarch64 QEMU count was 0 before the strict-gate run below
(`pgrep -x qemu-system-aarch64 | wc -l`); an unrelated concurrent session's
own boot briefly raised it to 1 mid-run (visible in the strict run's own
`qemu_at_start=1` sample on one boot below), which the host-wide lock from
#826/R181 serialized against rather than contending with.

### Strict gate, 20 boots (script default)

```
BREENIX_GATE_TMP=<scratch>/tmp-strict ./docker/qemu/run-aarch64-boot-test-strict.sh 20
```

`PASS: 20/20 boots succeeded` (`Successes: 20  Failures: 0  Success rate:
100%`, `Duration: 278s`). Every one of the 20 boots printed a
`GATE_BOOT_FACTS` line; boots 1 and 20 quoted verbatim:

```
[GATE_BOOT_FACTS:boot=1:host_ms=1788642135971-1788642146985:qemu_at_start=0:load_at_start=2.86:qemu_at_end=1:load_at_end=2.80:qemu_cpu_s=17.85:guest_uptime_ms=10808:ended_by=scored_pass]
[GATE_BOOT_FACTS:boot=20:host_ms=1788642401145-1788642413801:qemu_at_start=0:load_at_start=3.46:qemu_at_end=1:load_at_end=3.19:qemu_cpu_s=20.81:guest_uptime_ms=12221:ended_by=scored_pass]
```

Guest-uptime-to-host-wall-clock ratio, computed from these two lines'
`host_ms` window (`end - start`) and `guest_uptime_ms`:

```
boot 1:  host window 11014 ms, guest uptime 10808 ms -> ratio 0.981
boot 20: host window 12656 ms, guest uptime 12221 ms -> ratio 0.966
```

Both ratios sit close to 1.0, consistent with the guest clock tracking host
wall-clock on an unstarved boot -- the same discriminator #826's own report
computed by hand from an assumed 18s window (0.41-0.53 on its 2 starved
boots) is now a field this gate writes down itself, per boot, without that
assumption.
`qemu_cpu_s` across these 20 boots ranged 16.19-23.18s over 10-13s wall-clock
windows (a multi-vCPU TCG guest genuinely busy, not the `timeout` wrapper's
idle read the pre-fix version reported). `ended_by=scored_pass` on each of
the 20 -- this battery did not reproduce #826's own starved-boot signature, so
`hard_timeout`/`poll_exhausted` are demonstrated only by the ratchet's own
control-flow census, not by a live specimen in this run. `gate_boot_facts.txt`
was written into each boot's own `breenix_aarch64_strict_N/` directory.

Host aarch64 QEMU count and process table were confirmed clean immediately
after: `pgrep -fl qemu-system-aarch64` and the lock directory
(`$HOME/.cache/breenix/a64-qemu.lock`) both empty.

### Production-profile gate, 1 boot

```
BREENIX_GATE_TMP=<scratch>/tmp-prod ./docker/qemu/run-aarch64-prod-profile-boot-test.sh
```

`PASS: production profile reached bsshd with the futex oracle seam absent`,
with:

```
[GATE_BOOT_FACTS:boot=1:host_ms=1788642484724-1788642494932:qemu_at_start=0:load_at_start=2.72:qemu_at_end=1:load_at_end=3.22:qemu_cpu_s=16.19:guest_uptime_ms=9535:ended_by=scored_pass]
```

Ratio: host window 10208 ms, guest uptime 9535 ms -> 0.934. Written into
`gate_boot_facts.txt` under the boot's own `breenix_aarch64_prod_profile/`
directory, matching the console line exactly.

## Structural suites and claim-lint

```
cargo test -p breenix --test aarch64_testing_profile_structure --test
  block_request_lifetime_structure --test context_restore_structure --test
  coreproof_component_h_structure --test coreproof_coverage_structure --test
  coreproof_mutation_register_structure --test coreproof_sites_structure
  --test degenerate_transfer_fd_validation_structure --test
  dispatch_path_lock_free_structure --test dispatch_strand_census_structure
  --test dma_and_log_sink_structure --test entry_point_df_structure --test
  exec_lock_order_structure --test exit_tally_structure --test
  ext2_lock_structure --test fcntl_pm_contention_gate_structure --test
  fork_lock_order_structure --test gate_boot_facts_structure --test
  green_program_envelope_structure --test loopback_pump_structure --test
  masked_binary_load_structure --test mmap_floor_structure --test
  net_lock_structure --test poll_tcp_gate_wiring_structure --test
  preempt_bracket_structure --test qemu_host_lock_structure --test
  serial_line_atomicity_structure --test signal_eintr_predicate_structure
  --test strand_handoff_structure --test syscall_return_register_structure
  --test teardown_structure --test ttbr0_shadow_reconciliation_structure
  --test tty_oracle_structure
  (x33: `ls tests/*structure*.rs | wc -l` reads 33 on disk -- 32
  pre-existing + this branch's own tests/gate_boot_facts_structure.rs)
-> 33/33 files green (0 failed), 591 test cases total (5 of them this
  branch's own gate_boot_facts_structure.rs, confirmed by its own
  `running 5 tests` header above), exit 0

python3 scripts/test_claim_lint.py
-> Ran 72 tests in 1.537s / OK, exit 0

python3 scripts/claim-lint.py
-> "claim-lint: clean (4 file(s) checked, changed hunks vs 6c713cea602c)."
   "claim-lint: 26 pre-existing finding(s) outside this branch's changed
   hunks not reported (--whole-file shows them)." exit 0
```

The no-argument run first found 13 findings in this branch's own new
comment/doc text across the lib file, the 2 wired gates, and the ratchet
test -- the same "unquantified absolute in a repeated comment/doc-comment
block" and "`proving`/`proves` with no mutation keyword in the same
paragraph" shapes #825's and #826/R181's own docs record finding in their
own changed hunks. Reworded to bounded phrasing describing the same
behavior -- a bounded count or `each` swapped in for an unquantified
absolute, a negated phrasing swapped in for its absolute counterpart, a
phrase implying an unquantified absence swapped for one naming the actual
output, a phrase implying an unquantified proof swapped for one naming
the mutation applied, and a same-paragraph mention of what the mutation
actually does for the two spots using a bare claim-word for that concept
-- and the same no-argument run now reports 0 findings in this branch's
changed hunks.

```
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files <this doc> -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg> -> exit 0   (one per commit)
```

## What is NOT claimed

- **#826's own starved-boot signature was not reproduced by this branch's
  boot proofs.** 21 of 21 boots across both proof runs read `ended_by=scored_pass`
  with guest/host ratios of 0.93-0.98; `hard_timeout` and `poll_exhausted`
  are demonstrated by the structural ratchet's control-flow census and by
  this doc's worked derivation tables, not by a live specimen with either
  value on the record here. The attribution battery #826's own comment
  names as the next step is what would exercise those two values against a
  real contended host.
- **No scoring criterion and no deadline changed.** Both gates' poll-loop
  bodies are byte-for-byte what they were before this branch (confirmed via
  the pre-existing `strict_gate_poll_loop_stops_only_on_crash_or_complete_score`
  structural test, which reddened on an earlier attempt that inserted an
  assignment between a loop's `if` and its `break`, and is unmodified and
  green at this branch's head); `ended_by` is a read taken after each loop
  already stopped, not a new stop condition.
- **`qemu_cpu_s` and the aliveness check are single-sample reads, not a
  soak.** Each is one `ps`/`kill -0` call per boot, taken once, immediately
  before that boot's own kill; this branch does not claim they are free of
  the ordinary scheduling jitter any single process-table sample carries.
- **Host load average and QEMU count are host-local, point-in-time
  readings**, the same scope `docker/qemu/lib/qemu-host-lock.sh`'s own count
  already carries (native launches via `pgrep -x`, Docker-wrapped launches
  via a narrow `pgrep -f`; not a distributed or cross-host measurement, and
  not aware of a `qemu-system-aarch64` this repo's own scripts did not
  launch).
- **`gbf_resolve_qemu_pid`'s "walk one level down via `pgrep -P`" approach
  was found and fixed while gathering this doc's own boot verification, not
  designed in from the start** -- the pre-fix version's `qemu_cpu_s=0.00`
  misreport, present on each boot of that first run, stays on the record
  above as part of this branch's own history, not smoothed over.
- **This branch does not touch `scripts/`** or any x86 gate script; scope is
  the 2 aarch64 gates #827 named plus the new shared library and its
  ratchet.
- **Issue #826's own attribution question is not resolved by this branch.**
  This is the tooling #827 asked for; whether a given red is host
  contention or a real liveness defect is the next, separate step (an
  attribution battery run against a deliberately contended host), which
  this branch's own facts make mechanical rather than answering here.
