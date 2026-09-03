# #764 RCA -- `loopback_wake_test_child` exit 13 is dispatch latency, not a wake defect

<!-- claim-lint:ok: every count, latency figure and serial line number below is
     drawn from the four boot directories committed here, from battery.tsv (76
     rows, one per boot of the battery described in "The battery"), or from the
     25 boot directories in ../707-2026-09-02/x86-battery-r2/. census.py and
     summarize.py in this directory re-derive all of them with `grep`. -->

## The question

`loopback_wake_test`'s reader child exits 13 at
`userspace/programs/src/loopback_wake_test.rs:192-202` when `data_latency_ms`
exceeds `DATA_WAKE_BOUND_MS` (4000 ms). The issue's first specimen measured
4740 ms. `data_latency_ms` is a single number spanning the peer's pre-write
stamp to the reader's post-read stamp, so it cannot by itself say whether the
delay is (a) the writer being late, (b) the kernel losing or delaying the wake,
or (c) the reader being woken correctly and then not getting the CPU.

## The answer

(c). The wake is published inside the writer's own `write()` syscall, before
that syscall returns, and it is published once. What follows is the reader
waiting to run.

## The battery

76 boots, `docker/qemu/run-x86-gate.sh 4 full` (features
`testing,external_test_bins`), beast `breenix-x86` (KVM, `-cpu host`), branch
`fix/764-data-wake-latency` at `cd9bff44`, in a scratch clone at
`/root/rca764/breenix` cloned from `https://github.com/ryanbreen/breenix.git`.
19 script invocations of 4 boots each; the four smoke boots that validated the
instrumentation are counted in the 76 and use the same binary. Per-batch VM
load is in `host-load.txt` (`uptime` before and after each batch; the VM's
1-minute average ranged 0.21-0.97 across the run, with two batches at 0.94 and
0.97 and every other sample at or below 0.35). The beast host's own
1-minute average, sampled from this session, moved between 1.34 and 7.53 over
the run.

Outcomes for `loopback_wake_test_child`, from `battery.tsv`:

| verdict | boots |
|---|---|
| exit 13 (#764) | 2 -- `repro-boot-046`, `repro-boot-057` |
| exit 15 (#692) | 6 |
| no failure of this test | 68 |

The reproduction rate for exit 13 on this branch, host and profile is 2/76.
The issue's own first battery saw 1/25.

Preserved here: both exit-13 boots, and two clean comparators taken from the
same batches (`clean-boot-045` immediately precedes `repro-boot-046`,
`clean-boot-058` immediately follows `repro-boot-057`). `battery.tsv` carries
76 rows, but the four smoke boots reused batch-1's boot ids (`boot-001`
through `boot-004` each appear twice), so only 72 rows are tied to a distinct
boot id; neither duplicate is an exit-13 row, and the 2/76 rate is unaffected.

## Evidence 1 -- the wake is published inside the writer's `write()`

`repro-boot-057/serial_kernel.txt`, the reader is thread 30:

```
2210  TCP recv: entering blocking path, thread=30
2211  TCP_BLOCK: Thread 30 entering blocked state for recv
2288  TCP: Received 16 bytes of data                     <- payload in the rx buffer
2291  unblock(30): Added to per_cpu_queues[0]            <- reader published Ready
2292  TCP: Woke 1 connection waiters
2293  sys_write: Wrote 16 bytes to TCP connection        <- the writer's write RETURNS
```

Line 2291 is emitted at `kernel/src/task/scheduler.rs:2951`, inside the enqueue
arm of `unblock()`, which is reachable only after `thread.set_ready()` at
`scheduler.rs:2897`. So the reader's state is `Ready` and it names
`per_cpu_queues[0]` three lines before the writer's syscall returns.

`grep -n 'unblock(30)'` on that file returns two lines for the whole boot --
2123 (the accept wake) and 2291. The reader is woken once: no third `unblock(30)` line
appears before its read completes at 2623.

The issue's own first specimen has the identical shape at
`../707-2026-09-02/x86-battery-r2/boot01/serial_kernel.txt:2300-2305`, and two
`unblock(30)` lines for the boot, at 2210 and 2303.

## Evidence 2 -- the two reproductions, decomposed by the stamps

The instrumentation added on this branch stamps the writer's pre- and
post-write instants, the reader's accept return, its pre-read instant and its
post-read instant, each through `monotonic_ms()`.

**`repro-boot-057` -- reader blocked in `recv`, six dispatches missed:**

```
peer_stamps    conn=23495 w0=24237 w1=24582 write_ms=345
reader_stamps  w0=24237 acc=23454 pre=23454 data=29872
               w0_to_pre=0 pre_to_data=6418 lat=5635
```

`w0_to_pre=0` says the reader entered its blocking read before the peer wrote.
`write_ms=345` says the writer's own round trip was 345 ms. The remaining
5290 ms is after the wake. In the kernel serial the reader is restored six
times between the wake at 2291 and the line where its wait loop finally
observes the wake (`Restored kernel context for thread 30:` at 2313, 2349,
2407, 2490, 2558, 2620; `woken from recv blocking` at 2622).

**`repro-boot-046` -- the reader did not block in `recv`:**

```
peer_stamps    conn=23717 w0=23717 w1=25065 write_ms=1348
reader_stamps  w0=23717 acc=25063 pre=25063 data=28221
               w0_to_pre=1346 pre_to_data=3158 lat=4504
```

Here the reader was still inside `accept()` when the peer wrote:
`w0_to_pre=1346`. `grep -c 'TCP recv: entering blocking path, thread=28'` on
`repro-boot-046/serial_kernel.txt` returns 0 -- the payload was already in the
receive buffer when `read()` was called, so the read took the non-blocking arm.
`pre_to_data=3158` is therefore 3158 ms spent by a runnable userspace thread
between two adjacent syscalls, a `clock_gettime` and a `read` that had nothing
to wait for. No wake is involved in that interval at all.

Two different shapes, one cause: the reader is runnable and off the CPU.

## Evidence 3 -- three userspace probes bracket the 4000 ms bound

Distributions over the battery, from `battery.tsv`:

| probe | what it measures | n | min | p50 | p90 | max |
|---|---|---|---|---|---|---|
| `load_stamps max_gap_ms` | longest gap between consecutive clock samples in the load child's always-runnable 10 s spin | 74 | 503 | 1284 | 2626 | 6782 |
| `watchdog_stamps overrun_ms` | how far past its own 30 s deadline the timed sleep returned | 76 | 40 | 481 | 3076 | 17388 |
| `data latency_ms` | the quantity #764 bounds | 76 | 331 | 1241 | 2056 | 5635 |

The load-child probe is the load-bearing one: it is a thread that is runnable
for the whole window and whose only work is reading the clock, so a gap in its
samples is time it was not on a CPU. Its p90 is 2626 ms and its max is 6782 ms.
A 4000 ms bound sits inside that distribution, which is why #764 is rare rather
than absent.

The load child is SIGKILLed on an exit-13 boot before it spins, so those two
boots carry no `load_stamps`; they carry `reader_dispatch_probe` instead
(386 ms over 15067 samples on `repro-boot-046`, 492 ms over 66 samples on
`repro-boot-057`). That probe runs after exit 13 is already decided, in a
different phase of the boot, and is the weakest of the three.

## Evidence 4 -- the resumption census

A dispatch of a thread whose saved context is a kernel frame logs
`Restored kernel context for thread N:` from
`kernel/src/interrupts/context_switch.rs:962`. Counting those between the
`unblock(<reader>)` line and the `woken from recv blocking` line gives the
number of turns the reader was given before it consumed its wake. Over the 25
boots whose serials sit alongside
`../707-2026-09-02/x86-battery-r2/boot01/serial_kernel.txt`:

| turns | boots | `data latency_ms` |
|---|---|---|
| 1 | 20 | 318 to 1480 |
| 2 | 1 | 2258 |
| 3 | 1 | 2768 |
| 4 | 1 | 3155 |
| 7 | 1 | 4740 -- that battery's exit 13 |

(boot14 of that battery printed no accept-block line for port 54530, so it
carries no census; it passed at 1422 ms.)

Over this battery's 46 boots that have both an `unblock` line and a recv block:
39 at one turn, 6 at two, 1 at six -- the six being `repro-boot-057`.
`repro-boot-046` did not block in recv, so it has no census row.

## A calibration worth keeping

`data latency_ms` divided by the number of kernel-serial lines between the wake
and the reader's read is 9.1 to 11.3 ms on 36 of the 46 boots that carry both
numbers, and 9.1 to 18.5 on 43 of 46. In this profile a kernel serial line
costs about ten milliseconds of wall clock, so the reader's wait is, to first
order, the number of log lines the rest of the system emits while the reader
waits its turn. Applied to the issue's own specimen: 454 lines between the wake
at 2303 and the read at 2757, against a measured 4740 ms.

## The mechanism

The reader is woken correctly and promptly by the loopback data path -- the
wake runs to completion inside the writer's `sys_write`: the loopback packet is
drained in that same syscall, `wake_connection_waiters()` reports the waiter it
woke (`kernel/src/net/tcp.rs:1901`), and the readiness is published through
`Scheduler::unblock()` (`kernel/src/task/scheduler.rs:2884-2971`). From that
instant the reader is one runnable thread among the boot's own population --
`grep -c 'Added thread' repro-boot-057/serial_kernel.txt` returns 32, of which
22 carry `user: true` -- sharing a single ready queue: x86 builds
`MAX_CPUS = 1` (`kernel/src/task/scheduler.rs:1012`), so
`Scheduler::schedule()` (`scheduler.rs:1751`) round-robins one
`per_cpu_queues[0]`, re-enqueueing each preempted thread at the tail
(`scheduler.rs:1863`), with a 50 ms slice reset per switch
(`kernel/src/interrupts/timer.rs:33` `TIME_QUANTUM = 10` ticks at 200 Hz,
reset at `kernel/src/interrupts/context_switch.rs:436`). One turn through that
queue costs hundreds of milliseconds of wall clock in this profile, because the
kernel's DEBUG-level serial logging costs about ten milliseconds a line. Each
turn is itself a full dispatch of the reader -- a restore
(`context_switch.rs:962`), `set_running()` (`scheduler.rs:2103`), a fresh
quantum (`context_switch.rs:436`) -- and `sys_read`'s wait loop breaks the
moment `thread.state != Blocked` (`handlers.rs:1403-1410`), so a dispatched,
already-woken reader should consume its wake on the first turn, as it does on
39 of 46 census boots below. What `data_latency_ms` tracks is that turn count,
not queue length or a uniform per-pass cost: one turn is 318-1480 ms across the
707 battery's census, two is 2258 ms, three 2768 ms, four 3155 ms, and the
issue's own seven-turn specimen is 4740 ms. `repro-boot-046` shows the same
delay with no wake in it at all: 3158 ms between a `clock_gettime` and a `read`
that had data waiting. Why a dispatched reader gives the CPU back without
consuming a wake it already holds -- five of six turns on `repro-boot-057`, six
of seven on the issue's own specimen -- is not explained here; see "What this
does NOT establish", below.

## What this does NOT establish

- **Why a given dispatch does not consume the wake.** On `repro-boot-057` the
  reader is restored six times before its wait loop observes the wake, and on
  the issue's own specimen seven times. A restore and a save at the same RIP
  look identical whether the thread ran the `still_blocked` check at
  `kernel/src/syscall/handlers.rs:1403-1410` and read `Blocked`, or did not
  execute an instruction. Read against `unblock()`, which sets `Ready` at
  `scheduler.rs:2897`, and `schedule()`, which sets `Running` at
  `scheduler.rs:2103`, neither reading is derivable from the serials, and this
  work did not settle it. The experiment that would: a `trace_count!` on the
  `still_blocked == true` branch of that loop, read back through the lock-free
  tracing framework rather than through a log line, so the hot path is not
  disturbed.
- **Whether #692's exit-15 reds share this cause.** Six boots here hit exit 15.
  Their `eof_wait_ms` is a different wait phase with a different wake (a FIN,
  not data), and this work did not analyse them.
- **Whether the bound is the right bound.** Whether `DATA_WAKE_BOUND_MS` should
  be raised, or the profile's logging reduced, or the scheduler changed, is a
  decision this RCA does not make.

## Re-deriving the numbers

```
docs/planning/green-program/sockets/serials/764-rca/census.py \
    docs/planning/green-program/sockets/serials/764-rca/repro-boot-057
docs/planning/green-program/sockets/serials/764-rca/summarize.py \
    docs/planning/green-program/sockets/serials/764-rca/*boot-*
```

`census.py` prints the per-boot wake/dispatch census; `summarize.py` prints the
`battery.tsv` row format. `battery.tsv` here is the full 76-row table; only
four of those boots' serials are committed.
