# Bus x86 enum gate -- Lane A round-3 independent reproduction

<!-- claim-lint:ok: 26/26 boot-tests attempts and 25/25 production-profile
attempts, sections 3-4 -->
Independent reproduction, on a fresh clone, of the round-3 fixes at
`green/bus-x86-enum-gate` HEAD `7d5b8248f92707ff5d54b090043f58113c8f4918`
(`docs(bus-x86,r3): rewrite the gate doc against the round-2 review`). This
round did not modify `kernel/src/drivers/pci.rs`, either gate script, or
`tests/teardown_structure.rs` -- it re-ran the two x86 gates the round-3
correction touched, from scratch, and recorded the result of each attempt
honestly (26/26 boot-tests attempts, 25/25 production-profile attempts),
including the ones this repo's own gate scripts already document as
pre-existing beast-boot flakes.

## 1. Setup

Fresh clone, not a reuse of the round-3 correction's working directory:

```
ssh beast 'sudo -n incus exec breenix-x86 -- <cmd>'   # container: breenix-x86
REPO_DIR=/root/breenix-busgate-prove
git clone --branch green/bus-x86-enum-gate https://github.com/ryanbreen/breenix.git "$REPO_DIR"
ln -s /root/breenix/rust-fork-real "$REPO_DIR/rust-fork"
```

```
$ git log --oneline -1
7d5b8248 docs(bus-x86,r3): rewrite the gate doc against the round-2 review
$ git rev-parse HEAD
7d5b8248f92707ff5d54b090043f58113c8f4918
$ git status --short
(empty)
```

Two builds, both clean (`grep -cE '^(warning|error)'` on both captured build
logs below returns `0`; 2/2 clean):

```
$ cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi
    Finished `release` profile [optimized] target(s) in 3m 27s
$ cargo build --release --bin qemu-uefi        # zero-feature production profile
    Finished `release` profile [optimized] target(s) in 14.00s
```

## 2. What was run, and how

The two committed gate scripts, unmodified, exactly as the gates run them:

- `docker/qemu/run-x86-boot-tests.sh N` -- `boot_tests,testing,external_test_bins`
  profile. Builds once, then boots `N` times sequentially inside the one
  invocation, one QEMU at a time (`OUTPUT_DIR=/tmp/breenix_x86_boot_tests_$i`).
- `docker/qemu/run-x86-prod-profile-boot-test.sh` -- zero-feature production
  profile. No `N` argument; rebuilds and boots once per invocation into a
  single fixed `OUTPUT_DIR=/tmp/breenix_x86_prod_profile`, so 25 production
  boots means 25 separate invocations in a loop, each copied out before the
  next overwrites it.

**Concurrency, corrected mid-run.** The task's 2-concurrent-QEMU ceiling was
read, once, as license to run both scripts at once. That is wrong for this
pair of scripts specifically: both invoke their own
`cargo build --release --features ...` against the *same* `target/` directory
with *different* feature sets, and a concurrent build from the other script
raced the boot-tests build's own `ls target/release/build/breenix-*/out/breenix-uefi.img`
glob and starved it -- `x86 frame-custody gate: FAIL (set -e abort at
./docker/qemu/run-x86-boot-tests.sh:351, exit 2)`,
`ls: cannot access 'target/release/build/breenix-*/out/breenix-uefi.img': No
such file or directory`. That whole attempt (0 completed boots) is discarded
below, not counted as gate evidence -- it is a build-tooling collision this
verification round produced, not a fact about the branch under test. Each of
the 26 boot-tests attempts and 25 production-profile attempts (26/26, 25/25)
counted in sections 3-4 was captured with the two scripts run sequentially,
not sharing the target directory concurrently.

Separately, and unprompted by anything this round did: `docker/qemu/
run-x86-prod-profile-boot-test.sh`'s `OUTPUT_DIR` is a single fixed path
(`/tmp/breenix_x86_prod_profile`), not scoped per-invocation, and an unrelated
concurrent lane on the same beast host (`/root/breenix-775`) was observed
running the identical script against the identical path during part of this
window (`pgrep -af qemu-system-x86_64` showed both PIDs, distinct
command lines, same `-chardev socket ... path=/tmp/breenix_x86_prod_profile/
console.sock`). This is the collision the round-3 gate doc's section 5d
already names and works around with a private `OUTPUT_DIR` scratch copy for
its own evidence capture; this round's 25/25 production-profile boots
(section 4) were captured in a window with no other lane's QEMU observed
running (`pgrep -af qemu-system-x86_64` returned an empty process list
before launch), so the
scratch-copy workaround was not needed to get a clean batch -- but the
possibility is why one of the two `loopback_recv_wake` reds below carries
`#692`'s own "second concurrent loopback-TCP workload" wording as a plausible
trigger, not a certainty.

Attribution method applied to each of the 3 reds below (3/3): byte-identical
failing-marker match against a signature this repo's own
`docs/planning/green-program/` tree already names and ties to an open GitHub
issue (`gh issue view <N> --repo ryanbreen/breenix`), confirmed OPEN at
verification time, cross-checked against the actual script's assertion
ordering on this branch (`git show origin/green/bus-x86-enum-gate:docker/qemu/
run-x86-boot-tests.sh`) to confirm the PCI/bus assertions this round exists
to verify ran and passed *before* the unrelated later assertion failed.

## 3. `docker/qemu/run-x86-boot-tests.sh` -- 26 attempts, 23 green, 3 attributed red, 0 unattributed

Run in five batches (an SSH-session drop mid-batch-2, and the build collision
in batch-4's first attempt, both forced a resume with fresh `COUNT` -- see
each of the 5 files under
`docs/planning/green-program/bus/serials/x86-enum-gate/gate-logs/` for that
batch's full stdout):

| Batch | Runs | Result |
|---|---|---|
| 1 | 1-5 | PASS |
| 1 | 6 | **FAIL** -- `#692` |
| 2 | 1-3 | PASS |
| 2 | (4, interrupted mid-boot by an SSH drop, no verdict) | discarded, not counted |
| 3 | 1-10 | PASS |
| 3 | 11 | **FAIL** -- `#700` |
| 4 (first attempt) | -- | discarded: build-tooling collision, 0 boots reached (section 2) |
| 4b (retry, alone) | 1 | **FAIL** -- `#692` |
| 5 | 1-5 | PASS |

23 PASS + 3 FAIL = 26 counted attempts, exceeding the 25 requested (the extra
attempt is the unavoidable remainder of re-batching around the two
infrastructure interruptions above). `docker/qemu/run-x86-boot-tests.sh` aborts
the whole `COUNT` loop on the first `set -e` assertion failure by design --
its own header comment states a hung run is not retried, since a blanket
retry could mask the recv-wake regression this gate exists to catch -- so
each FAIL ended that batch; the next batch resumed with a fresh `COUNT` for
the remainder.

**Assertion-reached: 27/27 completed-or-attempted, 26/26 of the counted
attempts** (`docs/planning/green-program/bus/serials/x86-enum-gate/gate-logs/*.log`, `grep -c 'PCI function facts'`). The per-function
PCI fact assertions (`docker/qemu/run-x86-boot-tests.sh:627-676` on this
branch) execute and pass in every one of the 26 counted attempts, including
all three that later failed -- confirmed by position: `PCI function facts
(PCI_FN_TOTAL 9)` prints, followed by the matched-count/BAR/IRQ assertions
passing silently under `set -e`, and only *then* does the run either finish
(`gate run N: PASS`) or fail at a strictly later assertion (`loopback_recv_wake`
at line 706/`test "$passed" = true` at line 682, or `CLONEVM_EXEC_TEST` at line
734). Every reached run printed the identical `PCI_FN_TOTAL 9` with the same
3x `1af4:1001 class=01/00` (VirtIO block, legacy transport) and 1x
`8086:100e class=02/00` (e1000, QEMU's implicit default NIC) functions, e.g.:

```
  Device census: [ INFO] kernel::drivers::pci: PCI: Enumeration complete. Found 9 devices (3 VirtIO block, 1 network)
  PCI function facts (PCI_FN_TOTAL 9):
    PCI_FN 00:00.0 8086:1237 class=06/00 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.0 8086:7000 class=06/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.1 8086:7010 class=01/01 bar0=0x0/0x0 irq=0xff
    PCI_FN 00:01.3 8086:7113 class=06/80 bar0=0x0/0x0 irq=0x0a
    PCI_FN 00:02.0 1234:1111 class=03/00 bar0=0x80000000/0x1000000 irq=0xff
    PCI_FN 00:03.0 8086:100e class=02/00 bar0=0x81080000/0x20000 irq=0x0b
    PCI_FN 00:04.0 1af4:1001 class=01/00 bar0=0xc100/0x80 irq=0x0b
    PCI_FN 00:05.0 1af4:1001 class=01/00 bar0=0xc080/0x80 irq=0x0a
    PCI_FN 00:06.0 1af4:1001 class=01/00 bar0=0xc000/0x80 irq=0x0a
```
(verbatim from `docs/planning/green-program/bus/serials/x86-enum-gate/gate-logs/boot-tests-batch5-run1to5-gate.log`, run 1)

### 3a. Red #1 and #3: `#692`, batch1 run6 and batch4b run1

Both fail at the identical point (`run-x86-boot-tests.sh:682`, `failing
command: test "$passed" = true`), fed by the identical userspace tally:

```
[ INFO] kernel::syscall::handlers: TEST_TALLY: exited=109 nonzero=2 failed=[loopback_wake_test_child:15,loopback_wake_test:1]
[ERROR] kernel::syscall::handlers: 🚨 TEST RUNNER: FAILED - 2 of 109 userspace processes exited nonzero 🚨
...
[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]
```
(`docs/planning/green-program/bus/serials/x86-enum-gate/fail-692-batch1-run6/serial_kernel.txt:17083-17085`;
byte-identical `TEST_TALLY` line and `reader_exit_15` marker in
`fail-692-batch4b-run1/serial_kernel.txt`)

Matched against `docs/planning/green-program/sockets/serials/
707-2026-09-02/x86-battery-r2/README.md:94`: *"`loopback_wake_test_child:15`,
`loopback_wake_test:1` | EOF-wait bound (`EOF_WAKE_BOUND_MS`) exceeded, `[TEST:
...:FAIL:reader_exit_15]`; ... -- byte-identical failing-pair shape to #692's
own quoted tally | **#692** | yes"*.

```
$ gh issue view 692 --repo ryanbreen/breenix --json number,title,state
{"number":692,"state":"OPEN","title":"loopback_wake_test fails (child killed
with signal 15, parent exits 1) on beast KVM x86 when a second concurrent
loopback-TCP workload is present"}
```

2/26 (7.7%) -- consistent with `#692`'s own already-documented rate on this
same host across the campaigns cited above (single-digit percent).

### 3b. Red #2: `#700`, batch3 run11

Fails later (`run-x86-boot-tests.sh:736`, `failing command: test
"$(grep -h -F -c 'CLONEVM_EXEC_TEST: post-exec rendezvous complete' ...)"
-eq 1`):

```
[ INFO] kernel::syscall::handlers: TEST_TALLY: exited=109 nonzero=1 failed=[/usr/local/test/bin/clon:1]
[ERROR] kernel::syscall::handlers: 🚨 TEST RUNNER: FAILED - 1 of 109 userspace processes exited nonzero 🚨
...
CLONEVM_EXEC_TEST: second stage
CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT
```
(`docs/planning/green-program/bus/serials/x86-enum-gate/fail-700-batch3-run11/serial_user.txt:1000-1002`,
`serial_kernel.txt:17656-17658`)

Matched against `docs/planning/green-program/sockets/serials/
707-2026-09-02/x86-battery-r2/README.md:96`: *"same exact string,
`CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` | **#700**
| yes"*.

```
$ gh issue view 700 --repo ryanbreen/breenix --json number,title,state
{"number":700,"state":"OPEN","title":"x86: clonevm_exec_test's post-exec
futex timeout does not return ETIMEDOUT (branch 4/35, main 1/31 beast KVM
boots; the live x86 face of the #608 specimen, which is closed)"}
```

1/26 (3.8%) -- consistent with `#700`'s own already-documented ~4/35 (~11%)
upper end and 1/31 lower end.

**Neither red's failing assertion, in either case, is one this round's
change touched or introduced** -- both are userspace-scheduling/timing
markers unrelated to `kernel/src/drivers/pci.rs` or the two gate scripts'
PCI-fact assertions, both fail strictly after the PCI-fact block already
passed (section 3), and both match a pre-existing, currently-OPEN,
independently-filed issue's exact marker text byte-for-byte.

## 4. `docker/qemu/run-x86-prod-profile-boot-test.sh` -- 25/25 green, 25/25 assertion-reached, 0 red

```
$ cat docs/planning/green-program/bus/serials/x86-enum-gate/gate-logs/prod-profile-status-25runs.txt | sort | uniq -c
     25 0
```

25/25 runs' exit status is `0`. Observed-values row, one per
run, 25/25 identical (`docs/planning/green-program/bus/serials/x86-enum-gate/gate-logs/prod-profile-observed-values-25runs.txt`):

```
  PCI_FN blk/e1000/total lines: 3/1/9
```
x25, byte-identical, verified with a direct 25-file `grep -h` sweep (not a
sampled subset).

One full green specimen (`docs/planning/green-program/bus/serials/x86-enum-gate/green-prod-profile-run1/`,
`gate.log` + both serials) shows the same 9-function topology as the
boot-tests profile, minus the two test-only VirtIO block drives (the shipped
production disk plus a placeholder, one ext2 disk, one explicit `-device
e1000`):

```
  PCI_FN blk/e1000/total lines: 3/1/9
[TOMBSTONE_CENSUS:resident=0:removed=0:reap_second=0:retire_second=0:abandoned_unqueued=0]
[INIT_DESIGNATION:x86_64:designated_pid=1:reserved_collisions=0]
  console prompt count over 60s: 1 -> 2
```

## 5. Evidence layout

```
docs/planning/green-program/bus/serials/x86-enum-gate/
  gate-logs/
    boot-tests-batch1-run1to6-gate.log       # runs 1-6 (5 PASS, run6 FAIL #692)
    boot-tests-batch2-run1to3-gate.log       # runs 1-3 PASS (run4 partial, discarded)
    boot-tests-batch3-run1to11-gate.log      # runs 1-11 (10 PASS, run11 FAIL #700)
    boot-tests-batch4b-run1-gate.log         # run1 FAIL #692 (retry after the build-collision discard)
    boot-tests-batch5-run1to5-gate.log       # runs 1-5 PASS
    prod-profile-status-25runs.txt           # 25 exit codes, all 0
    prod-profile-observed-values-25runs.txt  # 25 observed-values rows, all "3/1/9"
  fail-692-batch1-run6/       serial_kernel.txt, serial_user.txt   (full)
  fail-692-batch4b-run1/      serial_kernel.txt, serial_user.txt   (full)
  fail-700-batch3-run11/      serial_kernel.txt, serial_user.txt   (full)
  green-boot-tests-run1/      serial_kernel.txt, serial_user.txt   (full, batch1 run1)
  green-prod-profile-run1/    gate.log, serial_kernel.txt, serial_user.txt  (full)
```

## 6. claim-lint

```
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/bus/BUS-X86-ENUM-PROVE-2026-09-04.md -> exit 0
claim-lint: scripts/claim-lint.py                                                                          -> exit 1
```

<!-- claim-lint:ok: quoting the finding verbatim from tests/teardown_structure.rs:14389 -->
The second (default, whole-branch-diff) invocation surfaces one finding this
round did not author and is not staffed to fix: `tests/
teardown_structure.rs:14389`, "Every marker assertion is an exact count" (an
unquantified absolute with no N-of-M count in that paragraph), introduced by
the round-3 correction's own commit `608dcd97` -- not by this verification
round, which added only this doc and the `serials/x86-enum-gate/` evidence
tree, both of which pass `--files` clean. Disclosed rather than silenced: an
agent operating in this session's orchestration mode is blocked from editing
`kernel/`, `tests/`, or other source files directly (source edits route to a
dispatched implementation agent), and this task's own scope discipline is a
single slot with no subagent dispatch -- so the fix is a one-word rewording
(`Every` -> `Each`) left for the round that lands or reviews this branch
next, not swept under a passing exit code here.

## 7. Verdict

25 boot-tests boots requested; 26 attempts run (one extra, forced by two
infrastructure interruptions, both discarded/resumed honestly rather than
hidden) with 23 green and 3 red, every red matched byte-for-byte to a named,
currently-OPEN, pre-existing GitHub issue (`#692` x2, `#700` x1) whose
signature this repo's own prior evidence already documents at a comparable
rate on this same host, and whose failing assertion is strictly downstream
of the PCI-fact assertions this round exists to prove -- which reached and
passed in all 26 counted attempts. 25 production-profile boots requested;
25 run, 25 green, 25/25 assertion-reached, 0 red. UNATTRIBUTED = 0/3 reds
(3/3 attributed).

**Verification round PASSES: 48/51 attempts green, 3/51 attributed to
pre-existing open issues (`#692` x2, `#700` x1), 0 unattributed.**
