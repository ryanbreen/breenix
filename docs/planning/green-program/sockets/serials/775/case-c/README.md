# Case C — the no-record serials, replayed three ways

Round-1 finding F2 was that exit 2 from the census (no marker in the log) had
become reachable on any boot that does not finish userspace, and that the
verdict script folded it into the same sentence as a real strand — destroying
the `#702`-vs-strand-family distinction `docker/qemu/run-x86-gate.sh:207-219`
states as a requirement. F2 named
`docs/planning/green-program/tty/serials/x86-prod-profile-gate-with-tty-canary-run1-20260831.txt`
and said serials like it reproduce the misclassification.

This directory replays that whole population through three script pairs.

## Population

```bash
grep -L "DISPATCH_STRAND_CENSUS" docs/planning/green-program/*/serials/*.txt \
  | xargs grep -L "Saved kernel context" | sort
```

<!-- claim-lint:ok: the count is the grep's own output, 102 of the 121 files
     that match docs/planning/green-program/*/serials/*.txt; the paths are
     listed in three-way-replay.txt. -->
102 of the 121 files matching that glob carry neither a census marker nor a
`Saved kernel context` record. On those 102 inputs each mechanism finds none
of its own records, which is exactly where F2 bites.

## Three script pairs

| pair | census | verdict |
|---|---|---|
| main | `git show bfbb7575:scripts/x86-strand-census.sh` | `git show bfbb7575:scripts/x86-gate-verdict.sh` |
| round 1 | `git show 66d68849:scripts/x86-strand-census.sh` | same file as main — `git diff bfbb7575 66d68849 -- scripts/x86-gate-verdict.sh` is empty (0 lines) |
| this head | `scripts/x86-strand-census.sh` | `scripts/x86-gate-verdict.sh` |

Each verdict script was run from a directory holding its own census script
under the name it calls (`$SCRIPT_DIR/x86-strand-census.sh`) plus a copy of
`scripts/x86-gate-allowlist.txt`, with `EXPECTED_EXITS=10`.

## Result

| pair | census rc=0 | census rc=2 | verdicts naming a strand |
|---|---|---|---|
| main | 102 | 0 | 0 of 102 |
| round 1 | 0 | 102 | 102 of 102 |
| this head | 0 | 102 | 0 of 102 |

The main and head verdict *sentences* agree on 102 of 102 files and differ on
0. Their distribution: 101 `FAIL - USERSPACE TEST COMPLETE was absent; boot
did not finish`, and 1 `PASS - exited=86 expected>=10 nonzero=0 allowlist=0`
(`docs/planning/green-program/tracing/serials/x86-r4-strand-excerpt.txt`,
which is an excerpt that does carry the completion markers).

The head keeps main's rc=2-vs-rc=0 difference — on 102 of 102 files the head
census has zero markers to read and says so — while restoring main's verdict.
That is the point of the rc=2 arm: unavailability is not evidence of a strand,
so the verdict continues to the ordered first-cause checks.

## Files

| file | what it is |
|---|---|
| `three-way-replay.txt` | one row per input file: path, the three census exit codes, the three verdict sentences |
| `reviewer-named-specimen-transcript.txt` | the six commands run verbatim on the file F2 named |

The reviewer-named specimen, in full:

```
### main (bfbb7575) census
STRAND_CENSUS: threads_saved_blocked=0 stranded=0 lines=304
rc=0

### round-1 (66d68849) census
strand census: expected exactly one DISPATCH_STRAND_CENSUS line, found 0
rc=2

### this head census
strand census: no DISPATCH_STRAND_CENSUS line found
rc=2

### main (bfbb7575) verdict, EXPECTED_EXITS=10
x86 userspace gate: FAIL - USERSPACE TEST COMPLETE was absent; boot did not finish

### round-1 verdict, EXPECTED_EXITS=10
x86 userspace gate: FAIL - a thread was saved blocked in a kernel wait and never restored (see the strand census above)

### this head verdict, EXPECTED_EXITS=10
x86 userspace gate: census unavailable; continuing with ordered first-cause checks
x86 userspace gate: FAIL - USERSPACE TEST COMPLETE was absent; boot did not finish
```

## What this does not cover

<!-- claim-lint:ok: the aarch64 and host-tool files in the population are
     listed by path in three-way-replay.txt; the x86 boot captures among them
     are the ones F2 cited. -->
The population is defined by the grep above, not by architecture: it includes
aarch64 gate-verdict transcripts and host-tool logs alongside x86 boot
captures. Those are not inputs any live caller feeds these scripts. They are
in the table because widening the input set can only make the exit-code
contract harder to satisfy, and the head satisfies it on all 102 rows; the
x86 boot captures among them are the population F2 actually named.
