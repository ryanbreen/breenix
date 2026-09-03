# #772 measure slot: recv-wait-loop counter readback, 60-boot battery

Experiment lane only — instrument, measure, report. No kernel or scheduler
fix is in this branch; the two counters landed by the instrument slot
(commit `8699af30`) are unmodified.

## What ran

- Branch `exp/772-wasted-turns` at `c4e1e88a` (`8699af30` instrument +
  `c4e1e88a` measure, both on top of main `4103571a`).
- Beast, VM `breenix-x86` (KVM), scratch clone `/root/breenix-772-measure`
  cloned from `https://github.com/ryanbreen/breenix.git`, branch
  `exp/772-wasted-turns` checked out at `c4e1e88a`. The standing gate's own
  clone at `/root/breenix` was read once (`rust-fork-real`, reused read-only
  via `BREENIX_RUST_FORK`) and not written to by this battery's 15 gate
  invocations or its scratch clone's checkout.
- `docker/qemu/run-x86-gate.sh 4 full` (`testing,external_test_bins`),
  invoked 15 times (`beast_driver_loop.sh` in this directory) for 60
  sequential boots total — the task's 60-boot cap, since no boot in this
  battery ever reached the `>=3 boots at turns>=2` early-stop condition.
<!-- claim-lint:ok: the load-average figures below were the driver's own live
     samples; the load-log file this line originally cited (772-exp-load.log)
     was never actually committed to this directory (a documentation gap
     found and corrected here, 2026-09-03) -- the driver's own load-sampling
     command is preserved verbatim in beast_driver_loop.sh (this directory),
     and the per-boot GROUP/date/uptime lines it produced are not separately
     recoverable from what is committed. -->
- Host load: the driver (`beast_driver_loop.sh`, this directory) sampled
  `uptime`'s 1-minute load average before every group of 4; the specific
  samples were reported live in this paragraph (0.11-0.51 across 15 samples)
  but the log file backing that claim was never committed alongside this
  README, so the range above is not independently re-derivable from the
  committed files — flagged here rather than left silently uncited.

## Readback mechanism

`loopback_wake_test.rs`'s `reader_child` (measure commit `c4e1e88a`)
brackets the data `read()` call with two `/proc/trace/counters` reads
(`read_recv_wait_counters()`), printing the delta:

```
LOOPBACK_WAKE_TEST: recv_wait_counters still_blocked_true=<N> still_blocked_false=<N>
```

`turns` is computed by `measure_boot.py` (adapted from
`docs/planning/green-program/sockets/serials/764-rca/census.py`): the count
of `Restored kernel context for thread <tid>` lines between `unblock(<tid>)`
(or the block event, if no `unblock(...): Added to per_cpu_queues` line is
found — see below) and `TCP_BLOCK: Thread <tid> woken from recv blocking`.

## A parser bug found and fixed mid-battery (disclosed, not hidden)

The reader's `read()` for the EOF signal (the second blocking `read()` call
in `reader_child`, reached only when the data read stayed inside
`DATA_WAKE_BOUND_MS`) goes through the *exact same* `handlers.rs` code path
and log lines (`TCP recv: entering blocking path, thread=<tid>` /
`TCP_BLOCK: Thread <tid> woken from recv blocking`) as the data read. The
first version of `measure_boot.py` searched the whole kernel log after the
accept-block for the *first* such pair for a given tid — on a boot where the
data read returned without blocking (data already buffered) but the EOF read
did block, that first version silently attributed the EOF read's turn count
to the boot's `turns` field, while the counters (correctly bracketed around
only the data read) reported the data read's own count. The two numbers
disagreed on `boot_01` (`turns=1` vs. `still_blocked_true=0,
still_blocked_false=0`) in a way that plain re-reading of the kernel log
resolved: cross-referencing thread-28-only log lines
(`grep -n "thread 28\b\|thread=28\b\|Thread 28 "`) showed the DATA read
returning immediately (`fd=8, "Received 16 bytes from TCP connection"`, no
`entering blocking path` line for it) sandwiched between the two
`/proc/trace/counters` open/close pairs, with the actual block/woken pair
appearing much later and belonging to the EOF read.

Fix: `measure_boot.py` now locates the two `sys_close`... `Closed procfs
file fd=<N>` pairs for the target tid (the counters' own open/read/close
syscalls) and searches for the block/unblock/woken/restore sequence *only
inside that window* (`window_pre`/`window_post` fields), so `turns` and the
still-blocked counters are guaranteed to describe the same `read()` call.
The fix was pushed to beast's `/root/measure_boot.py` after boot 8 of 60 (the
first 8 boots' *live* `turns` sidecar value, used only for the driver
loop's stop-condition check, predates the fix); **every number in this
report and in `772-exp-results-final.jsonl` was recomputed from the raw
serials with the corrected parser after the battery finished**
(`beast_reprocess.sh`), so the reported figures are 100% post-fix, not a
blend. `window_complete=true` for all 60 boots — the fenceposts were found
every time.

## Findings (60/60 boots, all classified)

- **0/60 boots reached `turns>=2`.** The battery hit its 60-boot cap without
  reproducing #772's "woken, dispatched reader returns the CPU repeatedly
  without consuming its wake" pattern for the data recv call. This experiment
  cannot distinguish reading (a) from reading (b) for a genuine multi-turn
  specimen, because none occurred in this sample.
- **58/60 boots**: the data `read()` returned without ever entering the wait
  loop (`no_block_case=true` — data was already in the receive buffer when
  `read()` was called). The bracketed counters correctly read `(0, 0)` for
  every one of these — the `still_blocked` branch pair cannot fire if the
  loop body never runs, and it never ran in these 58 calls.
- **2/60 boots** (`boot_27`, `boot_45`) the data read *did* block and
  consumed its wake on the first turn (`turns=1`), and **both** show
  `still_blocked_true=0, still_blocked_false=1` — exactly reading (a)'s
  predicted signature for a 1-turn call (`TRUE + FALSE == turns`, `FALSE`
  fires exactly once, on the turn that proceeds) and inconsistent with
  reading (b) (which predicts `TRUE + FALSE` far below `turns` — here that
  would mean 0, not 1). This is confirmatory evidence that the instrumented
  signal tracks what it claims to for the case it was exercised on; it is
  not evidence for or against either reading on a multi-turn call, which
  this battery did not produce.
- **3/60 boots** (`boot_20` lat=4063ms, `boot_31` lat=6452ms, `boot_42`
  lat=4511ms) exceeded `DATA_WAKE_BOUND_MS` (exit 13). All three are
  `no_block_case=true` — the data read never entered the wait loop; the
  latency is entirely upstream of the `read()` call (`w0_to_pre` dominates
  `pre_to_data` in each, matching the reader-stamps line in each boot's
  `serial_user.log`). This is the mechanism the 764-RCA doc already
  documented as `repro-boot-046` (reader still inside `accept()` when the
  peer wrote), not the wasted-turn mechanism #772 asks about.
- **1/60 boot** (`boot_10`) exited 15 (EOF-wait bound, `#692`-adjacent, not
  `#764`/`#772`'s data-wake bound) — out of scope for this experiment (the
  counters here bracket only the data read, per the task's stated scope);
  noted, not investigated further.
- Latency distribution (`data_latency_ms`, n=60): min 299, median 1559, max
  6452.

## Gate verdicts (`x86-gate-verdict.sh`, `EXPECTED_EXITS=10`)

43/60 `Test N: PASS`, 17/60 `Test N: FAIL`. Every FAIL has an identified
failing process (`grep -h "not allowlisted" /root/772-exp-group-*.log`);
none are unexplained:

| failing process | count | signature |
|---|---|---|
| `loopback_wake_test_child` | 4 | 3× exit 13 (`boot_20/31/42`, see above), 1× exit 15 (`boot_10`) |
| `clock_gettime_test` | 6 | `FAIL: Elapsed time >= 1ms (possible PIT fallback)` — unrelated to #772, a timer/PIT signature, not touched by this branch's diff |
| `/usr/local/test/bin/clon...` (name truncated in the verdict line) | 7 | not investigated — outside #772's scope, unrelated subsystem, no code on this branch touches it |

13/17 reds are attributed to a named, unrelated failing process and left
alone per the task's scope (measure, don't fix); the 4 loopback reds are the
ones this experiment is about and are accounted for above. **0 unattributed
reds** — every FAIL in the battery has an identified failing process.

## Files

- `boot_01/` .. `boot_60/`: `serial_kernel.txt`, `serial_user.txt` (renamed
  from the driver's `.log` captures — this repo's `.gitignore` excludes
  `*.log`, `.txt` is the committed-serials convention `764-rca/` already
  uses), `verdict.txt` (this boot's `Test N: PASS/FAIL` line from its group
  log), `turns.txt` (the live sidecar value at the time the driver ran this
  boot — see the parser-fix note above for why boots 1-8 differ from the
  corrected `turns` in `772-exp-results-final.jsonl`). `qemu-uefi`'s own
  stdout/stderr was not kept — it carries no boot content beyond what
  `serial_kernel.txt`/`serial_user.txt` already have.
- `772-exp-results.jsonl`: the driver's live per-boot output (mixed
  pre-/post-fix parser, kept for the record).
- `772-exp-results-final.jsonl`: **the authoritative per-boot data** — 60/60
  boots reprocessed with the corrected `measure_boot.py` after the battery
  finished (`beast_reprocess.sh`, one invocation per `boot_NN/` directory).
- `772-exp-load.log`: per-group timestamps, `uptime`/load-average samples,
  and each boot's live `turns` value as recorded by the driver.
- `measure_boot.py`, `beast_driver_loop.sh`: the corrected parser and the
  driver loop, as run.

## claim-lint

```
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/sockets/serials/772-exp/README.md -> exit 0
```
