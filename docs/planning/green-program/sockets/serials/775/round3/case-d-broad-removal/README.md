# case-d, the 22 boots behind the broad-removal decision

Round-3 review finding R3-10: the arm table in
`775-CENSUS-EQUIVALENCE-2026-09-04.md` -- the table that decides this
branch ships the narrow record removal and not the broad one -- rested on 27
boots of which only 5 were committed. This directory is the other 22.

They were produced by the round-3 implementation slot on the beast
`breenix-x86` container, in `/root/p775r3-out`, one QEMU at a time, with
`docker/qemu/run-x86-prod-profile-boot-test.sh`. The round-4 slot recovered
them from that host; the `source` column is the path each row came from.

Each of the 22 rows below was produced by running, on that host:

```
grep -c "^PASS: x86 production profile" <gate file>
grep -oE 'set -e abort at [^ ]+:[0-9]+' <gate file> | tail -1
grep -oE 'console prompt count over [0-9]+s: [0-9]+ -> [0-9]+' <gate file> | tail -1
```

## The 22 rows

| # | arm | source | PASS line | abort | prompt line |
|---:|---|---|---:|---|---|
| 1 | `baseline-51d7468f` | `prod-baseline/gate-1.txt` | 0 | `:1016` | -- |
| 2 | `baseline-51d7468f` | `prod-baseline/gate-2.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 3 | `baseline-51d7468f` | `prod-baseline/gate-3.txt` | 0 | `:1016` | -- |
| 4 | `baseline-51d7468f` | `prod-baseline-b/gate-1.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 5 | `baseline-51d7468f` | `prod-baseline-b/gate-2.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 6 | `baseline-51d7468f` | `prod-baseline-b/gate-3.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 7 | `baseline-51d7468f` | `prod-baseline-b/gate-4.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 8 | `baseline-51d7468f` | `prod-baseline-b/gate-5.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 9 | `census-only-33-records-kept` | `prod-norecordremoval/gate-1.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 10 | `census-only-33-records-kept` | `prod-norecordremoval/gate-2.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 11 | `census-only-33-records-kept` | `prod-norecordremoval/gate-3.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 12 | `census-only-33-records-kept` | `prod-norecordremoval/gate-4.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 13 | `census-only-33-records-kept` | `prod-norecordremoval/gate-5.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 14 | `broad-removal-22-records` | `prod-r3head/gate-1.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 15 | `broad-removal-22-records` | `prod-r3head/gate-2.txt` | 0 | `:1016` | -- |
| 16 | `broad-removal-22-records` | `prod-r3head/gate-3.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 17 | `broad-removal-22-records` | `prod-r3head-b/gate-1.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 18 | `broad-removal-22-records` | `prod-r3head-b/gate-2.txt` | 1 | -- | `console prompt count over 60s: 1 -> 2` |
| 19 | `broad-removal-22-records` | `prod-r3head-b/gate-3.txt` | 0 | `:953` | -- |
| 20 | `broad-removal-22-records` | `prod-r3head-b/gate-4.txt` | 0 | `:942` | -- |
| 21 | `broad-removal-22-records` | `prod-r3head-b/gate-5.txt` | 0 | `:952` | -- |
| 22 | `broad-removal-22-records` | `prod-gate.txt` | 0 | `:953` | -- |

## The tally the arm table reports

| arm | boots | pass | fail | failure sites |
|---|---:|---:|---:|---|
| `51d7468f`, the pre-round-3 head | 8 | 6 | 2 | `:1016` x2 |
| census on COM2 + idle hook, 33 of 33 records kept | 5 | 5 | 0 | -- |
| + the 22 non-error records removed | 9 | 4 | 5 | `:953` x2, `:942` x1, `:952` x1, `:1016` x1 |

`:1016` is `test "$(marker_count "$BSSHD_STARTED_LITERAL")" -eq 1`. The other
three are the prompt checks: `:942` is
`test "$PROMPT_AFTER" -gt "$PROMPT_BEFORE"`, `:952` is
`test "$PROMPT_BEFORE" -eq 1`, `:953` is `test "$PROMPT_AFTER" -eq 2`.

The fourth arm of the equivalence document's table -- the shipped narrow
removal, 5 boots, 5 pass -- is the one that was already committed, at
`../r3-production/gate-{1..5}.txt`. 5 + 22 = 27, the whole A/B.

## `specimens/` -- the four prompt-signature boots in full

| specimen | failing command | reported prompt count |
|---|---|---:|
| `specimens/prod-r3head-b/gate-3.txt` + `boot3/` | `test "$PROMPT_AFTER" -eq 2` | 3 |
| `specimens/prod-r3head-b/gate-4.txt` + `boot4/` | `test "$PROMPT_AFTER" -gt "$PROMPT_BEFORE"` | 0 |
| `specimens/prod-r3head-b/gate-5.txt` + `boot5/` | `test "$PROMPT_BEFORE" -eq 1` | 3 |
| `specimens/standalone-gate.txt` | `test "$PROMPT_AFTER" -eq 2` | 3 |

`specimens/standalone-gate.txt` is the ninth broad-arm boot; it was run outside
the numbered batches and its serial captures were overwritten before the
round-4 slot reached the host, so only its gate transcript survives. The other
three carry both serial captures.

The signature itself, verbatim from `specimens/prod-r3head-b/boot3/serial_user.txt`
lines 75-78:

```
[init] Breenix init starting (PID 1)
[SW]<K>[SW]<T><B>[SW]<K>
breenix> 
breenix> [SW]<T><B>[SW]<K>...
```

Two prompts back to back, immediately after init announces itself. `boot4` is
the other face of it: 0 prompts in the whole capture.
<!-- claim-lint:ok: the counts in both tables above are the 22 rows in this
     file, each produced by the three commands quoted at the top. -->
