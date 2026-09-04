# Round-7 bisect — the verdict lines and the failing specimens

The round-7 bisect over the 40 first-parent commits between the fork point and
`e3dc4040` reported its per-step classes in a scratch report, with 0 of its
artifacts committed. R7-005..R7-008 asked for the evidence. The host still had it, so this
is the bisect's own output, committed:

| path | what it is |
|---|---|
| `table.tsv` | the per-step table the bisect wrote: label, sha, subject, and the `[FAIL] Boot N: <reason>` verdict for each of boots A/B/C |
| `results/*.stdout.log` | the strict gate's own stdout for each of the 42 single-boot invocations |
| `results/*.exitcode` | that invocation's exit status (`0` = the boot passed) |
| `specimens/{A,B,C}/` | the 38 failing serials those runs preserved, under the names the verdict lines cite |
| `build-guard-logs/neon_*.log` | `scripts/check-kernel-no-neon.sh` for each bisect build |

## Reading a verdict back to its serial

`table.tsv`'s cells name host paths of the form
`/tmp/breenix_aarch64_strict_failures_A/<timestamp>-boot1.txt`. The same file is
committed here as `specimens/A/<timestamp>-boot1.txt`. 32 of the 32 serial
paths named in `table.tsv` resolve under `specimens/`:

```
$ python3 - <<'PY'
import csv,os,re
rows=list(csv.reader(open('table.tsv'),delimiter='\t'))
found=missing=0
for r in rows[1:]:
    for cell in r[3:]:
        m=re.search(r'/tmp/breenix_aarch64_strict_failures_([ABC])/(\S+\.txt)',cell)
        if m:
            p=os.path.join('specimens',m.group(1),m.group(2))
            found+=os.path.exists(p); missing+=not os.path.exists(p)
print(found,missing)
PY
32 0
```

claim-lint:ok: 32 cited serial paths resolve, 0 missing — the loop above.

## Counts this directory supports

* 42 single-boot invocations (`ls results/*.exitcode | wc -l` -> 42), of which 4
  exited 0: the three fork-point boots (`p0_forkpoint_{A,B,C}`) and
  `p8_023e049d_C`.
* 38 failing serials preserved (13 + 13 + 12 across `specimens/{A,B,C}`).
* The head sample (`results/HEAD_e3dc4040*`): 6 boots, 4 classified `CPU
  exception` and 2 `Futex handoff oracle marker missing or failed`. The
  bisect report's "2/3 at head" is round 1 of those two 3-boot rounds; both
  rounds read 2 of 3.

claim-lint:ok: 42 exitcode files, 4 zeros, 38 specimens, 4-of-6 head classes —
each read out of this directory at write time.

## What is NOT here

The bisect's own build logs beyond the no-NEON guard, its per-commit checkout
logs, and the `/tmp/bisect786/results/` intermediate `table.tsv` rebuilds were
scratch and are not committed. The one fixture (`target/ext2-aarch64.img`) the
bisect reused across its 13 kernel builds is a 256 MB binary and is not
committed either; the deviation it represents is stated in the bisect report and
repeated in `786-RCA-2026-09-04.md`.
claim-lint:ok: 13 kernel builds shared 1 fixture, per the bisect report's own
methodology section.
