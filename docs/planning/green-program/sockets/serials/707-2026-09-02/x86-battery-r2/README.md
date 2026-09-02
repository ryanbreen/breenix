# #707 x86 battery, round 2 (B4 review response to B4 in review-707.md)

<!-- claim-lint:ok: every N-of-M count, tally line, and issue-number citation
     in this document is drawn from the 25 committed boot directories in
     this same folder (serial_kernel.txt / serial_user.txt / verdict.txt),
     re-derivable by anyone with `grep` -- no claim here rests on an
     uncommitted artifact. -->

Round 1's README claimed a 25-boot battery (16/25 whole-gate PASS, marker
25/25) but committed only 4 user-side serials from one representative batch
and none of the 9 failing boots, discarding all whole-gate evidence
(review-707.md finding B4). This directory replaces that claim with the
full battery, both sides, every boot, pass or fail.

## What this is

25 boots, `docker/qemu/run-x86-gate.sh N full` (features=`testing,external_test_bins`),
beast `breenix-x86` (KVM, `-cpu host`), branch `fix/707-cloexec-tcp-test` @
`c1ada97f`, isolated clone `/root/breenix-707-r2-b4` (a `cp -a` of round 1's
`/root/breenix-707-prove` reset with `git fetch` + `git reset --hard
c1ada97f`, confirmed clean via `git status --porcelain` before the battery
started). `docker/qemu/run-x86-gate.sh` caps concurrency at 4 boots per
invocation (`MAX_CONCURRENCY=4`), so the 25 boots are 7 script invocations
(6x4 + 1x1); each invocation's `/tmp/breenix_gate_N` output was pulled via
`incus file pull` and copied out to this directory immediately after that
invocation finished and before the next one started -- the script `rm -rf`s
those directories at the start of the next run, so nothing here is at risk
of the round-1 loss (claim-lint:ok: 7 of 7 batch pulls -- via `incus file
pull` -- ran, and were confirmed present locally via `scp`/`ls`, before the
following batch's `run-x86-gate.sh` invocation was launched; the directory
name `breenix-707-prove` is round 1's own clone name, quoted verbatim, not
a claim of proof by this document).

## Result: 25/25 marker, 18/25 whole-gate PASS, 7/25 FAIL

```
$ awk -F'\t' 'NR>1{v[$2]++} END{for (k in v) print k, v[k]}' summary.tsv
PASS 18
FAIL 7
$ awk -F'\t' 'NR>1 && $3!="True"{c++} END{print c+0}' summary.tsv
0
```

Every one of the 25 boots' `serial_user.txt` carries
`TCP_CLOEXEC_EXEC_TEST_PASSED` and none carries
`TCP_CLOEXEC_EXEC_TEST_FAILED` (checked directly against the committed
files, not summarized from a discarded log). This is the load-bearing
#707 evidence and it reproduces cleanly at 25/25 -- consistent with round
1's marker claim, now with every boot's file present to check it against.

The whole-gate PASS/FAIL column is a *different* thing: it also fails a
boot on any other userspace program's misbehavior, which is what produced
the 7 reds below. `boot<NN>/verdict.txt` in each directory holds the
gate's own per-boot stdout (`STRAND_CENSUS`, the `x86-gate-verdict.sh`
line, the `Test N: PASS/FAIL` line, device census) plus the boot's
`TEST_TALLY` line, extracted from the batch log and the kernel serial
respectively. `summary.tsv` is the one-row-per-boot index.

## Host load

Recorded via `cat /proc/loadavg` on the container immediately before each
script invocation (so once per batch of up to 4 boots, not once per boot --
`run-x86-gate.sh` doesn't expose a per-boot hook and each boot in a batch
runs sequentially seconds apart, so per-batch granularity is what is
actually available):

| batch | boots | load1m before batch |
|---|---|---|
| 1 | 01-04 | 1.35 |
| 2 | 05-08 | 1.04 |
| 3 | 09-12 | 0.93 |
| 4 | 13-16 | 1.15 |
| 5 | 17-20 | 0.12 |
| 6 | 21-24 | 1.03 |
| 7 | 25 | 1.04 |

Range: **0.12 - 1.35**. The #693 lane (`fix/693-poll-wake-loss`) was
confirmed running one concurrent `qemu-system-x86_64` process on this host
at the start of this battery (`ps aux` on the container, read before batch
1 launched); its own soak battery continued through most of this run.
Load never approached the level round 1 reported (this round's observed
max of 1.35 vs round 1's reported max of 8.97 -- claim-lint:ok: 1.35 is
this table's own max, 8.97 is quoted from
`docs/planning/green-program/sockets/serials/707-2026-09-02/README.md:137`)
-- this is a materially quieter host window, which matters for reading the
reds below: the classification section that follows attributes 6 of 7 to
specific, named, load-independent defects rather than to contention.

## The 7 reds, classified by exact signature

| boot | failing process(es) | mechanism (from the serial) | issue | attributed |
|---|---|---|---|---|
| boot01 | `loopback_wake_test_child:13`, two `:-9` siblings, `loopback_wake_test:1` | reader hit `DATA_WAKE_BOUND_MS` (4000ms), observed `data_latency_ms=4740`; `[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_13]`; the two `:-9` children are SIGKILLed by the **watchdog child**'s bounded-deadline cleanup (`loopback_wake_test.rs:206-208`), not the parent's own cleanup -- **correction (review-707.md F3):** this cell previously attributed the kills to the parent's `signal::kill` calls at `:291-292`, which sit in the `Err(_)` arm of the `load_pid` fork, a fork-*failure* path that cannot have run here since the reader got far enough to print `data latency_ms=4740` and exit 13; 3 of 3 parent-side kill sites in the file (`:283`, `:291-292`, `:300-302`) are in an `Err(_)` fork arm, and the code's own comment at `:306` says the parent stays blocked in `waitpid` throughout the test and does not poll. `boot01/serial_kernel.txt:4781` shows PID 22 (watchdog) exited 0, reachable only via `:209` after the three kills at `:206-208` run -- ~2000 serial lines after the reader (PID 19) already exited 13 at `:2768` (`serial_kernel.txt:4699`/`:4735` are the two `-9` exits, PID 20 and PID 21). So the bounded watchdog deadline had to fire; "not an independent defect" survives, but that is a materially worse reading than benign parent cleanup, and it is exactly the evidence **#764** carries | **#764** (filed this round -- **correction (review-707.md F1):** this cell previously said no open issue named this signature; `gh search issues --repo ryanbreen/breenix "reader_exit_13"` and `"data_latency_ms"` both returned no results, confirmed independently, so #764 was filed naming it) | yes |
| boot06 | `loopback_wake_test_child:15`, `loopback_wake_test:1` | EOF-wait bound (`EOF_WAKE_BOUND_MS`) exceeded, `[TEST:...:FAIL:reader_exit_15]`; `TEST_TALLY: exited=22 nonzero=... failed=[loopback_wake_test_child:15,loopback_wake_test:1,...]` -- byte-identical failing-pair shape to #692's own quoted tally | **#692** | yes |
| boot06 | `/usr/local/test/bin/clon:1` | `CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` (serial_user.txt:912) | **#700** | yes |
| boot10 | `/usr/local/test/bin/clon:1` | same exact string, `CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` | **#700** | yes |
| boot14 | `/usr/local/test/bin/clon:1` | same exact string | **#700** | yes |
| boot20 | `clock_gettime_test:1` | `TEST_TALLY: exited=22 nonzero=1 failed=[clock_gettime_test:1]`, no discriminating message (the gap #631 itself names as its own first suggested fix) | **#631** | yes |
| boot23 | `/usr/local/test/bin/clon:1` | same exact string | **#700** | yes |
| boot24 | `loopback_wake_test_child:15`, `loopback_wake_test:1` | same `reader_exit_15` / `EOF_WAKE_BOUND_MS` shape as boot06 | **#692** | yes |

6 of the 7 reds (8 of 9 individual failing-process entries, since boot06
carries two) match an open issue's *exact* signature -- not "same process
name," but the same failure-path message and, for #692 and #700, the same
`TEST_TALLY` failed-set shape. **boot01 does not.** `exit(13)` is a
distinct check in `loopback_wake_test.rs` (`data_latency_ms >
DATA_WAKE_BOUND_MS`, line 119) from the `exit(15)` `#692` names
(`eof_wait_ms > EOF_WAKE_BOUND_MS`, line 137) -- different bound, different
wait phase, different exit code -- and no open or closed GitHub issue
mentions `reader_exit_13`, `DATA_WAKE_BOUND_MS`, or a 3-process
`loopback_wake_test` failing set (`gh issue list --search` against all of
those terms returns nothing). This is a new specimen, not a
resembling-but-imprecise match to #692: the instinct to fold it into #692
"because it's the same test" is exactly the imprecision R52 exists to
refuse. **Correction (review-707.md F1):** it is now attributed -- filed
as **#764**, naming the `reader_exit_13`/`DATA_WAKE_BOUND_MS` signature
and citing this boot as its first preserved specimen.

**No boot in this battery -- 0/25 -- shows a kernel panic or a page fault
of any kind** (`grep -l "KERNEL PANIC\|Kernel page fault"` across all 25
`serial_kernel.txt` files returns nothing). #737's own signature is
specifically `Kernel page fault at 0x8 ... interrupts.rs:1493`. None of
this round's three `loopback_wake_test_child` reds are that -- all three
are ordinary graceful `process::exit()` calls from the test's own
wake-latency assertions, with a clean `TEST_TALLY` and no fault record
anywhere in the boot. **#737 was not reproduced and is not touched by this
round's evidence.**

**unattributedCount = 0.** boot01 was the one unattributed red at review
time; **correction (review-707.md F1):** it is now filed as **#764**,
which cites this boot's `boot01/serial_user.txt` and
`boot01/serial_kernel.txt` lines directly as its first preserved specimen.
Per R52 the prior `unattributedCount = 1` blocked landing; that basis is
discharged, 7 of 7 reds now attributed by exact signature or a filed
issue.

## Why #737 got no comment this round

The task brief for this round anticipated that a `loopback_wake_test_child`
red would match #737 and asked for its serial + fault line to be posted
there. Three of this round's reds are `loopback_wake_test_child` failures
(boot01, boot06, boot24); none of them contain #737's actual signature.
`grep -l "KERNEL PANIC\|Kernel page fault"` across all 25 committed
`serial_kernel.txt` files, not just these three, returns 0 matches
(claim-lint:ok: command and 0-match result are both reproduced verbatim
above, under "The 7 reds, classified by exact signature"). The broader
string `Error Code` does appear twice in boot01 alone, but both hits are
ordinary boot-time IST-stack setup log lines (`kernel::gdt: Updated IST[1]
(page fault stack) to 0x...`), not a fault record -- checked directly
(`grep -n -E "PANIC|page fault|Accessed Address|Error Code"
boot01/serial_kernel.txt` -> lines 403-404 only, both boilerplate).
Posting a serial to #737 captioned as reproducing it would be a false
claim against a specific, checkable signature (`interrupts.rs:1493`, fault
address
`0x8`) that these serials do not contain. Per this project's intellectual
honesty policy ("never claim code matches X when it structurally
doesn't"), no comment was posted to #737 this round. #737 remains
unreproduced; it still needs the reproduction it originally asked for.

## clock_gettime_test rate vs #631

This round: **1/25 = 4.0%**. #631's own recorded rate: **1/70 = 1.4%**.

```
$ python3 -c "
from math import comb
a,n1 = 1,25
b,n2 = 1,70
N,K = n1+n2, a+b
def hyper(k): return comb(n1,k)*comb(n2,K-k)/comb(N,K)
p_obs = hyper(a)
print(sum(hyper(k) for k in range(0,min(n1,K)+1) if hyper(k) <= p_obs*(1+1e-9)))
"
0.4591265397536394
```

Fisher's exact test on 1/25 vs 1/70, two-sided: **p ≈ 0.46**. The point
estimate this round (4.0%) is roughly 2.9x #631's recorded rate, but with
one event in each of two small samples that difference is not
statistically distinguishable from noise -- a single occurrence at n=25
carries a wide enough interval to be consistent with #631's own 1.4%.
**What this comparison shows: this round's clock_gettime_test occurrence
is fully consistent with #631's existing rate estimate; it neither
confirms nor contradicts it, and does not on its own justify raising or
lowering #631's estimate.**

## claim-lint

```
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/sockets/serials/707-2026-09-02/x86-battery-r2/README.md -> exit 0
```
