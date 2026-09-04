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

Against that, the three records this change removes were 431–552 saves and an
equal number of restores per boot on the same profile at the pre-removal
commit (`../case-d/boot{1..5}/serial_kernel.txt`), and the COM2 capture shrinks
from 496992–543031 bytes there to 330341–335459 bytes here. The heartbeat adds
about a kilobyte to COM1; the removal takes about 165–210 kilobytes off COM2.

## Files

Per boot: `serial_user.txt`, `serial_kernel.txt`, `old-census.txt`,
`new-census.txt`, `verdict.txt`, each census file carrying its exit code as
its last line. `gate.txt` is the gate transcript for both boots.
