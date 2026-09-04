# Case B — post-removal head plus the bare-hlt poll wedge

<!-- claim-lint:ok: F1 and F8 are quoted from the round-1 review, whose text is
     reproduced in the gate-classification block of that review; the branch
     boots they describe are the 5-row green table in the round-1 revision of
     docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md. -->
Round-1 finding F8 said the new kernel ledger's red arm was unproven: the
branch's own battery was 5 green boots out of 5, so nothing in it showed the
ledger could report `stranded>0`. Round-1 finding F1 said the same mechanism
emitted nothing on 3 of 3 committed serials where main's string census had
found a real strand, because the only emission site sat behind the last
userspace exit, which a wedged boot does not reach.

This directory is one boot that answers both: the post-removal kernel, wedged,
with the census heartbeat naming five stranded threads while the boot is still
hung.

## What was run

| item | value |
|---|---|
| repository | `/root/breenix-775` on the beast `breenix-x86` VM |
| commit booted | `482a2e86` — `365c20c2` (this branch's head) plus `bare-hlt-wedge.patch` |
| harness | `docker/qemu/run-x86-gate.sh 1 full` (`--features testing,external_test_bins`, KVM, `cpu=host`) |
| mutation | `bare-hlt-wedge.patch` in this directory: `interrupts::disable()` + `hlt()` at both poll wait sites in `kernel/src/syscall/handlers.rs` |

`git diff --stat 365c20c2 482a2e86` reports `1 file changed, 12 insertions(+)`
in `kernel/src/syscall/handlers.rs`, which is that patch.

The patch is the adaptation of the historical mutation A recorded in
`docs/planning/green-program/sockets/serials/x86-mutation-A-bare-hlt-wedge-kernel-20260829.txt`.
It targets the poll wait sites rather than that specimen's original
instruction sites, which the evolved blocked-current check has made
unreachable.

## Files

| file | what it is |
|---|---|
| `serial_user.txt` | COM1 capture (`-serial file:`), where `serial_println!` and therefore the census marker land |
| `serial_kernel.txt` | COM2 capture, where `log::*` records land |
| `qemu_stdout.txt` | the QEMU process's own stdout for that boot |
| `gate.txt` | the full `run-x86-gate.sh` transcript, build line included |
| `census.txt` | `scripts/x86-strand-census.sh` output for this boot |
| `verdict.txt` | `EXPECTED_EXITS=10 scripts/x86-gate-verdict.sh` output for this boot |
| `results.txt` | the two exit codes and the booted commit |
| `bare-hlt-wedge.patch` | the mutation, as applied |

The `.log` names the gate writes were changed to `.txt` here because
`.gitignore` line 53 is `*.log`; the bytes are unchanged.

## What the boot shows

`grep -c "USERSPACE TEST COMPLETE"` is 0 in both captures and `grep -c
'TEST_TALLY:'` is 0 in both: this boot did not finish userspace. That is the
regime F1 named, and under round 1's single emission site it would have
produced no census line at all.

`serial_user.txt` carries twelve `[DISPATCH_STRAND_CENSUS:...]` snapshots
(`grep -o ... | wc -l` = 12 in `serial_user.txt`, 0 in `serial_kernel.txt`),
all from the heartbeat — the completion snapshot did not run. The last one is

```
[DISPATCH_STRAND_CENSUS:saved=10:stranded=5:tids=21,24,25,27,36:tid_overflow=0:ledger_overflow=0]
```

and `scripts/x86-strand-census.sh` turns it into five named threads plus the
summary, exiting 1:

```
strand census: thread 21 (tcp_cloexec_exec_test) saved blocked and never restored
strand census: thread 24 (loopback_wake_test) saved blocked and never restored
strand census: thread 25 (clonevm_exec_test) saved blocked and never restored
strand census: thread 27 (tcp_cloexec_exec_test_child_17_main) saved blocked and never restored
strand census: thread 36 (loopback_wake_test_child_22_main) saved blocked and never restored
STRAND_CENSUS: threads_saved_blocked=10 stranded=5 lines=4829
```

`scripts/x86-gate-verdict.sh` fails the boot on that reading
(`verdict_rc=1`). Both replay byte-identically from the committed captures on
another machine; the commands are in
`docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md`.

Three details worth keeping:

- The snapshots are not monotone. Their `stranded` values in order are
  0, 4, 5, 6, 4, 5, 5, 5, 5, 5, 5, 5. That is the ledger reporting live
  state, and it is why the consumer judges the LAST snapshot rather than any
  of them, and why an anchored whole-line match would be the wrong reader.
- 12 of the 12 markers sit mid-line, after other serial output on the same
  physical line (`...[SW]<K>[DISPATCH_STRAND_CENSUS:...]`), because COM1
  carries the userspace test chatter too. 0 of the 12 lines carry two markers;
  the consumer's awk loop advances past each match and keeps the last one
  found on the line anyway, which is what keeps a two-marker line from reading
  as its first value.
- `saved=10`, against the 11 the same profile reports on the three green
  case-A boots: one thread that ordinarily blocks did not get that far on
  this boot.
