# Head, green, `testing,external_test_bins`

Two boots of the branch head with the profile the merge gate runs, kept for
two things the other directories cannot supply: a committed head serial to
measure the heartbeat's serial cost on, and the direct demonstration that the
old string census is blind at this commit while the new one still reads.

## What was run

| item | value |
|---|---|
| repository | `/root/breenix-775` on the beast `breenix-x86` VM |
| commit booted | `365c20c2`, the branch head, unmodified |
| harness | `docker/qemu/run-x86-gate.sh 2 full` — `GATE: PASS (2/2 boot tests passed; mode=full build=16s boot=300s total=324s)` |

## Result

| boot | old census | old rc | new census | new rc | gate verdict |
|---:|---|---:|---|---:|---|
| 1 | `threads_saved_blocked=0 stranded=0 lines=4874` | 0 | `threads_saved_blocked=10 stranded=0 lines=4874` | 0 | PASS |
| 2 | `threads_saved_blocked=0 stranded=0 lines=4812` | 0 | `threads_saved_blocked=11 stranded=0 lines=4812` | 0 | PASS |

The old census reads 0 saved threads here because the records it parses are
gone from this commit — `grep -c` for both removed record strings is 0 in 2
of 2 `serial_kernel.txt`. Its exit code is still 0, which is exactly the
`threads_saved_blocked=0` reading `#702`'s filing leaned on and the reason
that reading can no longer be sourced from records. The new census reads 10
and 11 on the same two captures.

## Heartbeat serial cost, measured on these captures

Each snapshot is one `serial_println!`, so it costs its own text plus the
newline that macro emits, on COM1 only.

| boot | markers, COM1 | bytes added, COM1 | COM1 capture bytes | share | markers, COM2 |
|---:|---:|---:|---:|---:|---:|
| 1 | 10 | 930 | 57936 | 1.61% | 0 |
| 2 | 11 | 1051 | 54875 | 1.92% | 0 |

> **Corrected 2026-09-04 (#775 round 3, finding N9).** The paragraph below used
> to say the pre-removal boots had "an equal number of restores", and to quote
> ranges computed over `case-d/boot{1..5}` while `case-d` holds six boots. Both
> are re-derived here over the six, and the reduction is given in bytes so the
> kilobyte is not left ambiguous.

Against that, the three records this change removes ran 431 to 631 saves per
boot on the same profile at the pre-removal commit, over the six captures in
`../case-d/boot{1..6}/serial_kernel.txt`:

| boot | saves | restores | COM2 bytes |
|---:|---:|---:|---:|
| 1 | 552 | 552 | 543031 |
| 2 | 478 | 478 | 508320 |
| 3 | 480 | 478 | 513649 |
| 4 | 431 | 431 | 496992 |
| 5 | 449 | 449 | 503034 |
| 6 | 631 | 631 | 572581 |

Saves and restores are equal on 5 of the 6; boot 3 has 480 saves and 478
restores, which is the ordinary case of a capture ending with saves that had
not been matched yet. The COM2 capture shrinks from 496992–572581 bytes there
to 330341–335459 bytes here, so the removal takes between 161533 bytes
(496992−335459, 157.7 KiB / 161.5 kB) and 242240 bytes (572581−330341,
236.6 KiB / 242.2 kB) off COM2 per boot. The heartbeat adds about a kilobyte.

Round 3 moves the snapshot itself onto COM2 (finding N8), so from that round on
both figures are on the same channel; the per-snapshot byte cost is unchanged
and the counts are re-measured in `../../775-CENSUS-EQUIVALENCE-2026-09-04.md`.

## Files

Per boot: `serial_user.txt`, `serial_kernel.txt`, `old-census.txt`,
`new-census.txt`, `verdict.txt`, each census file carrying its exit code as
its last line. `gate.txt` is the gate transcript for both boots.
