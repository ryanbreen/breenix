# #772 round-2 measure serials

Full report and analysis:
[`../../772-EXPERIMENT-R2-2026-09-03.md`](../../772-EXPERIMENT-R2-2026-09-03.md).

This directory holds the raw evidence that report cites: `boot_001/` …
`boot_120/` (`serial_kernel.txt`, `serial_user.txt`, `verdict.txt`,
`census.json` per boot), `group-logs/772r2-group-1.txt` … `772r2-group-30.txt`
(the 30 `run-x86-gate.sh 4 full` invocations), `772r2-results.jsonl` (the
driver's live per-boot output, pre parser-fix), `772r2-results-final.jsonl`
(authoritative, post-fix), `772r2-load.txt` (per-group timestamps and load
samples), `census_r2.py` (the fixed census script), and `772r2_driver.sh` /
`772r2_start.sh` (the driver loop and its detached-launch wrapper), all as
run.

## claim-lint

```
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/sockets/serials/772-exp-r2/README.md -> exit 0
```
