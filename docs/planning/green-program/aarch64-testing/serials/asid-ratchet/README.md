# TTBR0 ASID ratchet — captured runs

The 16 captured artifacts behind
`docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md`,
taken at the branch head that record describes.
claim-lint:ok: 16 of 16 files beside this README are covered by the 10 rows
below; 3 rows use brace notation for the 3 boots they cover.

| file | what it is |
|---|---|
| `01-structural-anti-vacuity-raw-adopt.txt` | the suite run with `process_root_ttbr0` deleted from `adopt_process_ttbr0` — 25 passed, 2 failed, and the new census names the reverted function |
| `02-runtime-anti-vacuity-prod-gate.txt` | the production gate run against a kernel built from that same revert: exit 1 on a non-0 `untagged` |
| `02-runtime-anti-vacuity-prod-serial.txt` | that boot's serial, where the census climbs to `untagged=14` |
| `03-strict-x3.txt` | the strict gate, 3 iterations, at the shipping head |
| `03-strict-boot{1,2,3}-serial.txt` | those 3 boots' serials |
| `04-prod-boot{1,2,3}.txt` | 3 production-gate runs at the shipping head |
| `04-prod-boot{1,2,3}-serial.txt` | those 3 boots' serials |
| `05-suite-green-with-census.txt` | the TTBR0 suite green at the shipping head, with the 17-call census printed |
| `06-structure-suites.txt` | every `tests/*_structure.rs` suite: 29 of 29 green, 546 cases |
| `07-strict-score-legs.txt` | the strict gate's scoring run over `03-strict-boot1-serial.txt` as captured and under 3 mutations of it |
