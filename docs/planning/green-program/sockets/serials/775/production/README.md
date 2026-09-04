# Production profile — the census exists in the kernel that ships

<!-- claim-lint:ok: F6's own finding text states main's three records were
     gated only on `not(feature = "quiet_dispatch_log")`, off by default, and
     names kernel/src/task/mod.rs:11 as the new predicate. -->
Round-1 finding F6 was that the replacement census compiled only under
`feature = "testing"`, where main's three dispatch records had been gated on
`not(feature = "quiet_dispatch_log")` alone and so compiled into the x86
production profile too. F6 was filed non-blocking because no in-repo gate ran
the census over a production serial. That is still true of the gate scripts, but
the narrowing itself is closed: `kernel/src/task/mod.rs` now gates the module
on `target_arch = "x86_64"` alone, and this directory is the measurement that
the emission survives into a zero-feature build.

## What was run

| item | value |
|---|---|
| repository | `/root/breenix-775` on the beast `breenix-x86` VM |
| commit booted | `365c20c2`, the branch head, unmodified |
| harness | `docker/qemu/run-x86-prod-profile-boot-test.sh`, twice, back to back |
| build | that script's own `cargo build --release --bin qemu-uefi` — no `--features` flag at all |

## Result

Both runs printed, at line 249 of their transcripts:

```
PASS: x86 production profile reached steady state with the teardown census at rest
```

Both captured serials carry a census snapshot on COM1:

| boot | census markers, COM1 | census markers, COM2 | `scripts/x86-strand-census.sh` | rc |
|---:|---:|---:|---|---:|
| 1 | 1 | 0 | `STRAND_CENSUS: threads_saved_blocked=0 stranded=0 lines=2213` | 0 |
| 2 | 1 | 0 | `STRAND_CENSUS: threads_saved_blocked=0 stranded=0 lines=2459` | 0 |

The last (and only) marker in each, verbatim from `serial_user.txt`:

```
[SW]<K>[SW]<K>[DISPATCH_STRAND_CENSUS:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
```

## Why that marker has to be the heartbeat

`grep -c 'USERSPACE TEST COMPLETE'` is 0 in both captures, on both serials.
The completion snapshot (`kernel/src/syscall/handlers.rs`) is emitted only
inside the `!has_other_userspace_threads` block that prints that line, so it
did not run on either boot. The one marker each boot carries therefore came
from `report_heartbeat_if_due()`, called from `idle_thread_fn`
(`kernel/src/main.rs`) or the loopback pump — the two housekeeping contexts
R125 permits. Its position supports that reading: it sits mid-line at line 32
of `serial_user.txt`, immediately after the `[SW]<K>` idle-switch markers.

Under round 1's design this profile produced no marker at all and the census
exited 2. It now exits 0 with a real reading, which is what makes the census
usable on a production serial rather than only on a `testing` one.

## What one marker means, and what it does not

The rate limiter only permits a snapshot when at least a second of monotonic
time has passed since the last one, and it is shared by both emitters. On
these two production boots that admitted exactly 1 snapshot each. 1 snapshot
is enough for census availability (rc=0 rather than rc=2). It is not a claim
that a long-running production kernel emits one per second: neither of these
2 boots ran long enough in guest monotonic time to test that, and no
measurement here bounds the steady-state cadence.

`saved=0` is the honest reading for this profile and not a vacuity: a
zero-feature boot runs `/sbin/init` and the console tasks, and 0 threads had a
blocked kernel context saved while these captures ran. The non-vacuity
evidence for the same code lives in `../case-b-post-removal-mutation/`
(`saved=10 stranded=5`) and `../head-green/` (`saved=10` and `saved=11`).

## Files

Per boot: `serial_user.txt`, `serial_kernel.txt`, `gate.txt` (the full
production-gate transcript), `new-census.txt`, `heartbeat-counts.txt`.
