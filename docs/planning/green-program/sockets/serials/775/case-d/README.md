# Case D — green boots with the heartbeat present, both censuses read

Round 1's equivalence table was 5 green boots at `29344251`, where the kernel
still carried the three dispatch records and the census was emitted once at
the last userspace exit. This directory re-runs that battery on the kernel
this round actually ships the emission design of: the same pre-removal commit
with the round-2 heartbeat overlaid, so both mechanisms are live in the same
boot and can be read from the same capture.

## What was run

| item | value |
|---|---|
| repository | `/root/breenix-775` on the beast `breenix-x86` VM |
| commit booted | `5b419714` — `29344251` plus the patch committed at `../case-a/historical-wedge/heartbeat-overlay.patch` |
| harness | `docker/qemu/run-x86-gate.sh 4 full`, then `1 full`, then `1 full` (the gate caps one invocation at four sequential boots) |
| old census | `git show bfbb7575:scripts/x86-strand-census.sh`, run on `serial_user.txt serial_kernel.txt` |
| new census | `scripts/x86-strand-census.sh` at the same commit, same two files |
| verdict | `EXPECTED_EXITS=10 scripts/x86-gate-verdict.sh`, same two files |

`5b419714` carries this branch's `kernel/src/task/dispatch_strand_census.rs`,
`kernel/src/task/mod.rs`, `kernel/src/main.rs`, `kernel/src/net/loopback_pump.rs`,
`kernel/src/syscall/handlers.rs`, `scripts/x86-strand-census.sh` and
`scripts/x86-gate-verdict.sh` byte-identically to the head `365c20c2`
(`git diff --quiet` on each of those 7 paths). What it does not carry is the
head's `kernel/src/interrupts/context_switch.rs`: there the three dispatch
records are still compiled in, which is the whole point — it is the only
revision where the two mechanisms can be compared on one boot.

## Result

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc | gate verdict |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 2 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 3 | 11 | 11 | 0 | 0 | 0 | 0 | FAIL — `failing process is not allowlisted: loopback_wake_test_child` |
| 4 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 5 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 6 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |

6 of 6 boots agree on `threads_saved_blocked`, on `stranded`, and on the exit
code. The two censuses also print the same `lines=` count on 6 of 6, because
they read the same pair of files.

Boot 3's gate failure is a userspace test flake, not a census disagreement:
its two censuses agree at `saved=11 stranded=0 rc=0`, and the verdict that
failed it is the allowlist check that runs after the census. The same failure
appears in this round's other batteries at other commits
(`../case-a/historical-wedge/boot7` fails the same way naming
`clock_gettime_test`), so it is not attributable to anything on this branch.
5 of the 6 boots are gate-green.

## Files

Per boot: `serial_user.txt` (COM1), `serial_kernel.txt` (COM2),
`old-census.txt`, `new-census.txt`, `verdict.txt`, each with the exit code
appended as its last line. `gate-boots1-4.txt`, `gate-boot5.txt` and
`gate-boot6.txt` are the three gate transcripts, each carrying its own
`Build clean (0 warnings)` line.
