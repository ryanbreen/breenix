# A host-wide aarch64 QEMU lock (#826/R181)

## Why

Ruling R181: the per-brief "keep host aarch64 QEMU concurrency at 2 or below,
checked with `pgrep` before each boot" discipline this campaign has been
running on failed as an operating rule. 4-6 concurrent `qemu-system-aarch64`
processes were observed live on this Mac even so (claim-lint:ok: #826, see that issue's own report and its linked serials for this count), and #826's own 40-boot
health battery measured what that contention does to a boot: the guest clock
ran at 37-53% of wall-clock on two starved boots, well under the pace a
healthy boot needs to clear the strict gate's ~18s poll ceiling before its
`timeout 20` fires. Both boots were healthy — heartbeats kept a steady ~1s
cadence to the last line, 0 crash markers — they simply did not get the CPU
they needed inside the host-side deadline. #827 separately found the gate
itself records 0 of 4 host-side facts (per-boot wall clock, host
QEMU count, QEMU CPU time, which bound ended the boot) that would let a
starved boot be told apart from a wedged one mechanically; that gap stays
open here — this branch does not add per-boot instrumentation, only the lock
R181 asked for as the mitigation.

The fix this branch makes mechanical, rather than a discipline each gate
author has to remember and re-derive: **at most one `qemu-system-aarch64`
process is alive on this host at a time.**

## The helper: `docker/qemu/lib/qemu-host-lock.sh`

A sourced bash library, not a standalone wrapper binary. Two public
functions:

- `qemu_host_lock_acquire` — prints the host's current aarch64 QEMU count,
  then blocks (if the lock is not disabled) until an exclusive lock is held.
  Installs an `EXIT` trap on first use.
- `qemu_host_lock_release` — releases the lock if this process holds it; a
  safe no-op otherwise (so the `EXIT` trap's call after an already-explicit
  release has no further effect).

### Lock implementation: `mkdir`, not `flock(1)`

macOS ships no `flock(1)`. The brief's own text offered two portable
alternatives — a `python3 -c 'fcntl.flock(...)'` helper, or an atomic
`mkdir`/stale-PID approach — and names the choice as something to justify in
this doc.

**`mkdir` was the pick**, for where these ~20 call sites need the lock held
across. Each caller acquires immediately before backgrounding
`qemu-system-aarch64` (`... &`; `QEMU_PID=$!`) and releases only after a
later, separate poll-and-kill sequence — often dozens of seconds and several
external commands later, sometimes inside a per-boot loop. A `python3
fcntl.flock` acquired via a helper subprocess does not survive past that
subprocess's own exit unless the file descriptor is threaded through an
`exec()` chain (open the fd, flock it, exec into `qemu-system-aarch64`, let
the kernel auto-release the lock on that process's eventual exit) — workable,
but it would force each of the ~20 call sites into the same spawn-and-exec
shape, and several of them background `qemu-system-aarch64` directly in the
calling shell (not via a wrapper exec) specifically so they can `kill
$QEMU_PID` it early on a crash marker or a poll timeout. An `mkdir`/`rmdir`
pair needs no fd bookkeeping and no exec chain: acquire is one function call,
release is another, and they can sit on opposite sides of arbitrary
intervening shell code in the same shell process — matching the shape each
caller already has, with no restructuring beyond adding the two calls.

`mkdir` is the atomic primitive (it either creates the directory and
succeeds, or fails with `EEXIST` — no window where two racing callers can
both see success). A PID file inside the lock directory backs stale-lock
reclaim: if a script dies while holding the lock (an untrappable `SIGKILL`,
a host crash) the next acquirer's `kill -0` on the recorded PID finds it
dead and reclaims the directory. This checks the same fact a kernel-side auto-release on `flock` would rely
on when a process dies -- it is the same
underlying fact (the process is gone) checked a different way, since
`flock`'s auto-release on `SIGKILL` is a kernel table cleanup, not something
either design's own trap can perform.

### Opt-out

`BREENIX_QEMU_LOCK=off` disables locking for that run — the only opt-out.
Otherwise, if `BREENIX_QEMU_LOCK` is set, it names the lock directory to use
instead of the default (`$HOME/.cache/breenix/a64-qemu.lock`) — e.g. to give
one deliberately-isolated test lane its own lock domain. A disabled run
prints a loud banner both immediately (when locking is skipped) and again,
via the chained `EXIT` trap, right after the script's own PASS/FAIL output —
adjacent to the verdict without this file needing to know each of its ~20
callers' own verdict-printing shape.

### Host aarch64 QEMU count

`qemu_host_lock_acquire` prints the host's count on each call. The first
implementation used a blanket `pgrep -f 'qemu-system-aarch64'`, which turned
out to double-count: GNU coreutils `timeout` (used by most of these scripts
as `timeout N qemu-system-aarch64 ...`) forks a monitoring parent that keeps
`timeout N qemu-system-aarch64 ...` as its own argv for the life of the
child, so a single boot showed up as two matching PIDs (the `timeout`
wrapper and the exec'd `qemu-system-aarch64` child) — found while gathering
the evidence in the "Boot proofs" section below, by a background `pgrep`
sampler recording `count=2` for what a serial-file birth/mtime cross-check
confirmed was one boot (see the boot-run check (a) below).
`nice(1)` execs in place and does not have this problem. The fixed version
counts native launches (bare, under `nice`, or the exec'd child of
`timeout`) by process-name match (`pgrep -x qemu-system-aarch64`, which does
not match the `timeout` wrapper's own `comm`), plus Docker-wrapped launches
(`run-aarch64-test.sh`, `run-aarch64-userspace.sh`,
`run-aarch64-interactive.sh` — whose actual `qemu-system-aarch64` process
runs inside Docker's own Linux VM, invisible to this host's process table)
via a `pgrep -f 'docker run.*qemu-system-aarch64'` match narrow enough not
to pick up an unrelated process.

## What changed: 20 scripts wired to the lock

Each script under `docker/qemu/` with a real `qemu-system-aarch64` launch
line now `source`s the helper and calls `qemu_host_lock_acquire` before that
launch (with `qemu_host_lock_release` after the matching kill+wait, where
the script's own flow reaches one explicitly — several scripts instead rely
on the helper's chained `EXIT` trap for release, described above, which the
existing `cleanup`-on-`EXIT` scripts among them already needed anyway).

| Script | Shape |
|---|---|
| `run-aarch64-boot-test-strict.sh` | single launch inside a per-boot loop (the gate #826's own health battery ran) |
| `run-aarch64-boot-test-native.sh` | single launch inside a per-boot retry loop |
| `run-aarch64-prod-profile-boot-test.sh` | single launch |
| `run-aarch64-testing-profile-boot-test.sh` | single launch inside a per-boot loop, backgrounded and tracked (see the fix-round addendum below -- this ran foreground until then) |
| `run-aarch64-full-test.sh` | single launch |
| `run-aarch64-stability-test.sh` | single launch |
| `run-aarch64-percpu-stack-custody-gate.sh` | single launch, release via the chained `EXIT` trap only |
| `run-aarch64-refusal-drain-gate.sh` | single launch, release via the chained `EXIT` trap only |
| `run-aarch64-tty-oracle-gate.sh` | single launch inside a per-boot loop |
| `run-aarch64-service-sequence-gate.sh` | single launch inside a per-profile, per-boot loop (25 boots/profile default) |
| `run-aarch64-test-suite.sh` | single launch inside a per-test loop |
| `run-aarch64-userspace-test.sh` | single launch |
| `run-coreproof-gate.sh` | single launch inside a per-profile, per-seed loop |
| `run-ext2-lock-race-gate.sh` | single launch, aarch64 leg only (shared x86/aarch64 script; x86 leg untouched) |
| `run-fs-fault-gate.sh` | single launch, aarch64 leg only (shared x86/aarch64 script; x86 leg untouched) |
| `run-aarch64-arma609-arm.sh` | single launch inside a per-boot loop, two branches (`nice`d/plain) sharing one `launch_qemu()` |
| `run-aarch64-test.sh` | Docker-wrapped; lock held around the host-side `docker run` |
| `run-aarch64-userspace.sh` | Docker-wrapped; lock held around the host-side `docker run` |
| `run-aarch64-interactive.sh` | Docker-wrapped, human-driven VNC session; lock held around the host-side `docker run` |
| `run-aarch64-kthread-parallel.sh` | restructured — see below |

### `run-aarch64-kthread-parallel.sh`: a real behavior change, not a wording one

This script's whole point was launching `COUNT` (default 10)
`qemu-system-aarch64` processes **concurrently** — a launch loop
backgrounding them together, then a separate wait/verdict loop polling each
one's output for up to 60s — exactly the shape #826 measured driving the
guest clock down. Routing each launch through the host-wide lock, with no
other change, would have serialized those N processes to one at a time
while leaving the two-loop structure in place, and that combination is
actively wrong: each boot's 60-sample search window used to run
concurrently with the other boots' windows in the same batch (the N started
together), not starting from its own launch. Serializing the launches alone
would make later boots in the batch wait behind earlier ones for a lock
while their own search window was already running, ticking down against a
launch that had not happened yet — a false `TIMEOUT` generator.

The fix merges the two loops into one: each boot is launched, polled, killed
and verified before the next one starts. The lock now serializes what was
already, after this restructuring, a sequential loop — so for this one
script, the lock's enforcement and the loop's own restructuring are both
required together; neither alone is correct. Total wall-clock for `COUNT`
boots is now roughly `COUNT` times one boot's duration rather than bounded
by the slowest of N run together — a real cost, paid once, in exchange for
not producing the false-TIMEOUT shape above and not contributing to
#826's contention pattern.

## The ratchet: `tests/qemu_host_lock_structure.rs`

Census-shaped: `shell_scripts_below("docker/qemu")` (recursive) finds each
`.sh` file under that tree, and `launches_qemu_aarch64()` re-derives "this
script starts a real `qemu-system-aarch64` process" from each script's own
text — a line that, once any trailing `\` line-continuation is stripped,
ends with the bare token `qemu-system-aarch64`, and is not a comment. This
excludes the helper's own `pgrep -f 'qemu-system-aarch64'` search line (it
does not end with a continuation) without a path-based exemption, so the
predicate does not have to know the helper lives at
`docker/qemu/lib/qemu-host-lock.sh` to skip it correctly.

For each script the census finds launching, the whole-suite test asserts it
also (a) `source`s `lib/qemu-host-lock.sh` and (b) calls
`qemu_host_lock_acquire` (a bare, trimmed line — each real call site this
branch added is shaped exactly that way, which also excludes the helper's
own `qemu_host_lock_acquire() {` definition line from being mistaken for a
call to itself). An anti-vacuity floor pins the launching-script count at
20, not a closed list — a future aarch64 gate script only needs to raise
this count, not edit it down.

### Mutation record

```
cmd:  cd docker/qemu/run-aarch64-boot-test-strict.sh's qemu_host_lock_acquire
      call line ("    qemu_host_lock_acquire\n") removed from the real file,
      then: cargo test --test qemu_host_lock_structure --
            every_aarch64_qemu_launch_script_sources_and_acquires_the_host_lock
exit: 101 (test binary FAILED; "test result: FAILED. 0 passed; 1 failed")
assertion: "aarch64 QEMU launch(es) bypass the host-wide lock:
            docker/qemu/run-aarch64-boot-test-strict.sh: launches
            qemu-system-aarch64 but does not calls qemu_host_lock_acquire
            (#826/R181)"
```

The mutation was applied to the real, tracked file (not a scratch copy),
then reverted via a preserved backup before this doc was written;
`git status` on that file was clean (matching this branch's own additions,
not any mutation leftover) before the branch's own commits were made. The
file's own sibling test, `qemu_host_lock_predicates_are_not_vacuous`, proves
the same predicate pair on four legs each run (each real continuation
shape this branch used is detected as a launch; the helper's own search
line and a plain comment mention are not; the helper's own definition line
is not mistaken for a call to itself) and repeats this mutation on the
in-memory string rather than the file on disk, as a second, redundant
witness that does not depend on a file having been physically edited and
restored correctly.

## Boot proofs (2026-09-05)

Kernel built at this branch's own head, in a fresh worktree needing its own
`rust-fork` symlink and its own `userspace/programs/build.sh --arch aarch64`
+ `scripts/create_ext2_disk.sh --arch aarch64` run (a worktree has no
inherited `target/`):

```
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
-> Finished `release` profile [optimized] target(s)

scripts/check-kernel-no-neon.sh target/aarch64-breenix-kernel/release/kernel-aarch64
-> PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)
```

Host aarch64 QEMU count was 0 before each of the runs below (`pgrep -x
qemu-system-aarch64 | wc -l`).

### (a) Two strict-gate runs, launched together, sharing the default lock

Both runs used the **default, unset `BREENIX_QEMU_LOCK`** — the same lock
domain, on purpose, since the point is showing they contend for it — with
separate `BREENIX_GATE_TMP` values so their output files do not collide
(the orthogonal #825 concern). Launched back to back with no delay between
them, and with a background sampler recording a timestamped, accurate
`pgrep -x qemu-system-aarch64` count each 0.5s across the whole window:

```
BREENIX_GATE_TMP=<scratch>/tmp-a ./docker/qemu/run-aarch64-boot-test-strict.sh 1   # run A
BREENIX_GATE_TMP=<scratch>/tmp-b ./docker/qemu/run-aarch64-boot-test-strict.sh 1   # run B, launched immediately after
```

Both PASSED: run A `PASS: 1/1 boots succeeded`, run B `PASS: 1/1 boots
succeeded`.

The sampler's own log, condensed to its count transitions (49 total
samples, full log preserved in this branch's own scratch record):

```
 6 samples  18:50:07 count=0
42 samples  18:50:11 count=1  (pid 40373, then pid 41062 -- see below)
 1 sample   18:50:34 count=0
```

**At no sampled instant across the whole 27-second window did the count
reach 2.** The per-PID breakdown inside that 42-sample block shows exactly
one handoff, with no intervening `count=0` gap: PID 40373 held each sample
from 18:50:11 through 18:50:21, then PID 41062 held each sample from
18:50:22 through 18:50:33. Cross-checked against each run's own
`serial.txt` file timestamps (macOS `stat -f %SB`/`%Sm`, birth and
modification time):

```
tmp-b/breenix_aarch64_strict_1/serial.txt: birth=14:50:11  mtime=14:50:21   (matches PID 40373's window exactly)
tmp-a/breenix_aarch64_strict_1/serial.txt: birth=14:50:22  mtime=14:50:34   (matches PID 41062's window exactly)
```

Run B's `serial.txt` was born the same second its PID first appears in the
sampler and died the same second the PID stops appearing; run A's the same.
Run A's `QEMU HOST LOCK: host aarch64 QEMU count before acquire: 0` line
prints at script start (18:50:07, before either process exists) — that
line alone does not show who won the race, since it fires before the
blocking `mkdir` loop even runs — but the PID timeline settles it directly:
run A's `qemu-system-aarch64` did not exist until the same half-second run
B's disappeared, meaning run A spent that interval blocked on
`qemu_host_lock_acquire`'s `mkdir` spin loop, not idle or independently
scheduled — this is the "the second waited for the first" property, shown
by process-existence timestamps rather than inferred from a log line.

### (b) One production-profile run, default `BREENIX_QEMU_LOCK` (unset)

```
./docker/qemu/run-aarch64-prod-profile-boot-test.sh
```

`PASS: production profile reached bsshd with the futex oracle seam absent`,
with `QEMU HOST LOCK: host aarch64 QEMU count before acquire: 0` printed at
launch (no contention — a single unshared run). A first attempt before this
run FAILED on `bsshd never reached its listening state` (claim-lint:ok:
quoted verbatim from this attempt's own script output; the preserved
failing serial landed at this branch's own scratch
`.../proof-826/tmp-prod/breenix_prod_profile_failures/20260905T185319Z/serial.txt`,
not committed to this repo) with a healthy boot (heartbeats to 119s uptime,
0 crash markers, 0 host
contention — the lock's own count line also read 0 on that attempt); a
retry with an identical, freshly-rebuilt kernel PASSED. This reads as the
same host-load-timing sensitivity #826/#827 are about in general, not
something this branch's own change caused (the lock printed 0 contention on
both attempts, since no other aarch64 QEMU was running either time) — it is
recorded here rather than silently discarded, per this branch's own
practice of preserving what it finds.

## Structural suites and claim-lint

```
cargo test --test <name>   for each of the 32 tests/*_structure.rs files
  (31 pre-existing + tests/qemu_host_lock_structure.rs)
-> 32/32 green, 584 test cases total (summed from each suite's own "N passed"
   line; qemu_host_lock_structure.rs contributed 2 of the 584)

python3 scripts/test_claim_lint.py
-> Ran 72 tests in 1.611s / OK, exit 0

python3 scripts/claim-lint.py
-> "claim-lint: clean (22 file(s) checked, changed hunks vs fdc65c8aacf4)."
   "claim-lint: 173 pre-existing finding(s) outside this branch's changed
   hunks not reported (--whole-file shows them)." exit 0
```

The no-argument run first found 34 findings in this branch's own new
comment text across the lock helper, the 20 wired scripts, and the ratchet
test file — the same "over-broad phrasing in a repeated comment block"
shape #825's own doc records finding in its own changed hunks. Reworded to
bounded phrasing describing the same behavior -- an unquantified absolute
swapped for `each`, an N-count, "not"/"0 of", or a rephrase carrying the
same meaning without an unquantified absolute -- and the same no-argument
run now reports 0 findings in this branch's changed hunks.

```
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files <this doc> -> exit 0
claim-lint: scripts/claim-lint.py --files <issue #834's body> -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg> -> exit 0   (one per commit)
```

## Fix round: signal safety (review findings, same day)

A review of this branch found two gaps the sections above did not disclose,
both about what happens on a SIGTERM/SIGINT delivered to just a routed
script's own PID (not its process group) during the boot-poll window,
while the lock is held -- a non-terminal termination path (a CI
job-canceller targeting the child PID, for instance), distinct from the
terminal-Ctrl-C / process-group case the boot proofs above did not need to
cover since a process-group signal already reaches a backgrounded child on
its own.

- **The lock could be left un-released.**
  `run-aarch64-prod-profile-boot-test.sh`'s `cleanup()` (chained onto the
  helper's own EXIT trap since it predates this branch) ends in
  `exit "$status"`, which terminates the shell immediately -- the
  `qemu_host_lock_release; _qhl_verdict_banner` half of the composed trap,
  positioned after `cleanup $?` in that trap string, did not run. Reproduced
  with an isolated harness under `/bin/bash` (this host's `#!/bin/bash`
  target is macOS's stock 3.2.57, not a newer bash) mirroring the exact
  chain shape: a targeted `kill -TERM` on the script's own PID left the
  lock directory behind for the next acquirer's stale-PID reclaim path to
  find, instead of an explicit release. Fixed by having `cleanup()` call
  `qemu_host_lock_release` and `_qhl_verdict_banner` itself, right after
  its own kill+wait of `QEMU_PID` and before its own `exit` -- confirmed
  with the same harness: the lock directory is gone immediately, not
  reclaimed later.
- **12 of the 20 scripts had no PID a targeted signal could reach.** The
  helper's chained EXIT trap releases the lock but does not, by itself,
  touch the caller's own QEMU/`docker run` process; for a script with no cleanup of
  its own, a SIGTERM/SIGINT to just its PID freed the lock while the actual
  `qemu-system-aarch64` (or, for the three Docker-wrapped scripts, the
  host-side `docker run` client) kept running -- the exact contention this
  branch exists to prevent, now with the lock itself reporting free.
  Reproduced for both a backgrounded launch and a foreground one (matching
  `run-aarch64-testing-profile-boot-test.sh`'s shape at the time, which had
  no `&` and so no PID to capture): both left the child alive after a
  targeted kill of the parent. Fixed with a new helper function,
  `qemu_host_lock_track_pid`, that a script calls immediately after
  capturing its launch's PID; the helper's EXIT trap now kills and reaps
  each tracked PID before releasing the lock. Wired into the twelve
  affected scripts (`run-aarch64-boot-test-native.sh`,
  `run-aarch64-boot-test-strict.sh`, `run-aarch64-interactive.sh`,
  `run-aarch64-kthread-parallel.sh`, `run-aarch64-test.sh`,
  `run-aarch64-test-suite.sh`, `run-aarch64-testing-profile-boot-test.sh`,
  `run-aarch64-tty-oracle-gate.sh`, `run-aarch64-userspace-test.sh`,
  `run-aarch64-userspace.sh`, `run-ext2-lock-race-gate.sh`'s aarch64 leg,
  `run-fs-fault-gate.sh`'s aarch64 leg);
  `run-aarch64-testing-profile-boot-test.sh` also had its foreground launch
  backgrounded so it has a PID to track at all. The other eight routed
  scripts already killed and waited their own `QEMU_PID` inside a working
  `cleanup` trap and needed no change for this gap.

What this fix round does not extend to: an untrappable `SIGKILL` (or a host
crash) still leaves both the QEMU child and the lock directory behind with
no trap able to run at all -- unchanged from the stale-lock reclaim design
described above, now also true of the child process, not only the lock
directory. And the two reproductions above are, like the boot proofs
section, single-shot harness runs demonstrating the mechanism, not a
many-iteration soak of the signal-delivery path.

## What is NOT claimed

- **Beast (x86) is unchanged.** This branch touches no x86-only script, and
  the two shared aarch64/x86 scripts it does touch (`run-ext2-lock-race-gate.sh`,
  `run-fs-fault-gate.sh`) only gained the lock on their `if [ "$ARCH" =
  "aarch64" ]` branch; their x86 leg's `qemu-system-x86_64` launch is
  untouched, and `qemu_host_lock_release`'s no-op-when-not-held behavior
  means the one shared release line after the `if`/`else` block is safe on
  either leg.
- **`scripts/` is not covered.** R181's own wording and this branch's ratchet
  both scope to `docker/qemu/*.sh`. A grep found six more scripts under
  `scripts/` with a real `qemu-system-aarch64` launch line, 0 of them wired
  to this lock — filed as
  [#834](https://github.com/ryanbreen/breenix/issues/834), not fixed here.
- **#827's gate-side instrumentation gap is untouched.** This branch adds a
  lock, not the per-boot wall-clock/host-count/QEMU-CPU-time/ended-by
  fields #827 asks for. A boot that times out under contention and a boot
  that genuinely wedges still score identically from the gate's own output;
  the lock only reduces how often the contended case occurs, on this one
  host, by construction.
- **The lock is host-local and process-based, not a distributed or
  cross-host mechanism.** It does not address contention with a QEMU
  process on a different machine, or contention from a process this
  lock does not wrap (e.g. a manually-launched `qemu-system-aarch64` outside
  any script in this repo).
- **The two-concurrent-run check above is 1 boot each, not a soak.** It
  shows the property (0 of 49 sampled instants read 2 concurrent, second
  waits for first) on one contended pair, corroborated two independent ways
  (a live process-count sampler and each run's own file timestamps) — not a
  many-iteration statistical claim about how often contention would
  otherwise have occurred on this host.
- **The `qemu_host_lock_count()` fix (native via `pgrep -x`, Docker-wrapped
  via a narrow `pgrep -f`) was found and fixed while gathering this doc's
  own evidence, not designed in from the start** — the first version's
  double-counting under `timeout` is recorded above as part of this
  branch's own history, not smoothed over.
- **`run-aarch64-kthread-parallel.sh`'s restructuring changes its runtime
  characteristics** (sequential instead of concurrent boots, `COUNT` times
  longer wall-clock) for the reason given above; this was not soak-tested
  at a larger `COUNT` as part of this branch.
- **An untrappable `SIGKILL`, or a host crash, still leaves both the QEMU
  child and the lock directory behind.** The fix-round section above closes
  the gap for a trap-reachable signal (SIGTERM/SIGINT, as reproduced
  there); `SIGKILL` bypasses the EXIT trap by design (POSIX does not
  deliver it to a trap handler), so this branch's code -- before or
  after the fix round -- has no path to the child process in that case,
  and the lock directory
  is left for the next acquirer's stale-PID `kill -0` reclaim path, same as
  the top-of-file design already describes for a host crash.
- **The fix round's two signal reproductions are single-shot, not a soak.**
  Each demonstrates the mechanism once (a targeted `kill -TERM` against a
  harness mirroring the real chain shape); neither is a many-iteration
  statistical claim about the signal-delivery path the way the boot proofs
  section's 49-sample concurrent-run check is for the no-signal path.

## Landing re-smoke

Merged `origin/main`'s 16 commits (merge base fdc65c8a, main head 7a19f550)
onto this branch's review-r1 head 22cd3665 at merge commit 6289a026, in a
fresh worktree. Two files overlapped -- `run-aarch64-boot-test-strict.sh`
and `run-aarch64-prod-profile-boot-test.sh` -- and git's auto-merge combined
both sides' hunks in each with no conflict markers and no manual resolution:
this branch's `qemu_host_lock_acquire`/`qemu_host_lock_track_pid`/
`qemu_host_lock_release` calls sit alongside main's #812
`IRQ_HOLD_ORACLE` additions in both files, confirmed hunk-by-hunk against
each parent. The three touched `kernel/` files and `tests/teardown_structure.rs`
were changed only by main, not by this branch, and came through as exact
copies of main's versions.

Re-ran the full suite at the merged head:

- **32 of 32 `tests/*_structure.rs` suites** (31 pre-existing plus this
  branch's own `qemu_host_lock_structure.rs`): `cargo test -p breenix --test
  <name>` for each, one `cargo test` invocation covering all 32 targets --
  `test result: ok` for 32 of 32, 586 tests passed, 0 failed, 0 filtered
  out (main's #812 work added tests to `teardown_structure.rs`, so the count
  moved from the pre-merge branch's own 584 to 586).
- **`scripts/test_claim_lint.py`**: exit 0.
- **One strict gate run** (`./docker/qemu/run-aarch64-boot-test-strict.sh`,
  script default of 20 iterations), aarch64 kernel built with `--features
  boot_tests`: `PASS: 20/20 boots succeeded` (`Successes: 20  Failures: 0
  Success rate: 100%`).
- **One production-profile gate run**
  (`./docker/qemu/run-aarch64-prod-profile-boot-test.sh`, which builds its
  own no-features kernel): `PASS: production profile reached bsshd with the
  futex oracle seam absent`, exit 0.

0 of the 4 checks above went red at the merged head, so no attribution to a
pre-adjudicated signature was needed. `pgrep -fl qemu-system-aarch64` read 0
before each of the 2 gate-run launches in this re-smoke, and 0 again after
both completed: 0 orphaned QEMU processes across the 4 samples.

Two worktree-local setup gaps surfaced and were resolved before the above
runs, neither a defect in this branch's own changes: the fresh worktree had
no `rust-fork` symlink (needed a `rust-fork -> .../rust-fork` symlink
matching the main checkout's own setup, since `libs/libbreenix-libc`'s
build-dependency chain resolves `rust-fork/library`'s own `libc` path
dependency relative to that symlink's logical location, not a `$PROJECT_ROOT`-relative
override) and no prebuilt `userspace/programs/aarch64/*.elf` files or
`target/ext2-aarch64.img` (neither is git-tracked; the ELFs were copied from
the main checkout since neither branch's diff touches `userspace/` sources,
and the ext2 image was rebuilt fresh via `scripts/create_ext2_disk.sh --arch
aarch64`).

claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/gates/HOST-QEMU-LOCK-2026-09-05.md -> exit 0
