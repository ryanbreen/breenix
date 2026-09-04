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

## Which emitter produced that marker

<!-- claim-lint:ok: round-3 finding N1 established that main.rs's
     idle_thread_fn body is never dispatched on x86; the attribution below is
     corrected accordingly and the corrected reason is measured, not assumed. -->
> **Corrected 2026-09-04 (#775 round 3, findings N1 and N2).** This section
> first attributed the marker to `report_heartbeat_if_due()` "called from
> `idle_thread_fn` (`kernel/src/main.rs`) or the loopback pump". Round 3
> measured that `idle_thread_fn`'s BODY is never dispatched on x86 — it is only
> the idle task's stored entry point — so it could not have been that. The
> emitter was the loopback pump's first pass, and round 3 moves the idle-side
> hook to `context_switch.rs::idle_loop()`, the loop x86 actually runs.

`grep -c 'USERSPACE TEST COMPLETE'` is 0 in both captures, on both serials.
The completion snapshot (`kernel/src/syscall/handlers.rs`) is emitted only
inside the `!has_other_userspace_threads` block that prints that line, so it
did not run on either boot. The one marker each boot carries therefore came
from `report_heartbeat_if_due()`, and at this commit the only reachable caller
was `loopback_pump_fn`. Its position agrees: it sits mid-line at line 32 of a
152-line `serial_user.txt` (151 in boot 2), immediately after the `[SW]<K>`
switch markers, at the pump's first pass.

Under round 1's design this profile produced no marker at all and the census
exited 2. It now exits 0 with a real reading, which is what makes the census
usable on a production serial rather than only on a `testing` one.

## What one marker means, and what it does not

> **Corrected 2026-09-04 (#775 round 3, finding N2).** The sentence this
> paragraph used to end on — "neither of these 2 boots ran long enough in guest
> monotonic time to test that" — is contradicted by the transcript committed
> beside it: `boot1/gate.txt` ends `console prompt count over 60s: 1 -> 2`, so
> each boot held steady state for at least 60 seconds. A working one-per-second
> heartbeat would have emitted about 60 snapshots. The real reason is N1: the
> only reachable emitter was the loopback pump, which blocks itself once there
> is no loopback traffic, and this profile produces none.

The rate limiter only permits a snapshot when at least a second of monotonic
time has passed since the last one, and it is shared by the two emitters this commit had. On
these two production boots that admitted exactly 1 snapshot each. 1 snapshot
is enough for census availability (rc=0 rather than rc=2); 1 snapshot per
boot does not measure a cadence, and this directory does not claim one.

The consequence is measurable and worth stating plainly: that single snapshot
is at COM1 line 32 of 152, while init's own thread is not added until COM2
line 519 (`Added thread 4 'init' to scheduler`). It therefore reads the ledger
BEFORE any userspace thread exists, so `saved=0` on these two boots is
structural, not a finding about the shipped profile's dispatch behaviour. What
round 3 changes and re-measures is in
`../../775-CENSUS-EQUIVALENCE-2026-09-04.md`.

`saved=0` is what the ledger held at the instant this snapshot was taken, and
per the correction above that instant precedes init. The non-vacuity evidence
for the same code lives in `../case-b-post-removal-mutation/`
(`saved=10 stranded=5`) and `../head-green/` (`saved=10` and `saved=11`).

## Files

Per boot: `serial_user.txt`, `serial_kernel.txt`, `gate.txt` (the full
production-gate transcript), `new-census.txt`, `heartbeat-counts.txt`.
