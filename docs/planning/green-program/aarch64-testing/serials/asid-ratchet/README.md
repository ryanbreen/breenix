# TTBR0 ASID ratchet — captured runs

The 26 captured artifacts behind
`docs/planning/green-program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md`.
Files 01–07 are from the round as first written; 08–13 are from the R157 review
round that closed five findings against it, and are taken at the head this
branch now carries.
claim-lint:ok: 26 of 26 files beside this README are covered by the 20 rows
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
| `08-corridor-instruction-delta.txt` | R157/ASID-02: two production-profile builds differing only in the two `note_shadow_publish` calls, disassembled, with the per-symbol instruction counts and both listings of `switch_ttbr0_if_needed` |
| `09-suite-green-after-r157.txt` | the TTBR0 suite at the R157 head: 32 passed, with the census printed and the dispatch publish now `[normalised]` |
| `10-leg-restore-normalisation-deleted.txt` | R157/ASID-04 leg 1: the production gate with `process_root_ttbr0` deleted from `restore_process_ttbr0` — exit 0, a NULL result, because the fall-through to `adopt_process_ttbr0` re-normalises |
| `10-leg-restore-normalisation-deleted-serial.txt` | that boot's serial, 15 census lines, every one `untagged=0` |
| `11-leg-resume-fast-arm-raw-gate.txt` | R157/ASID-04 leg 2: the production gate with the blocking-resume fast arm publishing its caller's raw operand — exit 1 at `untagged=104` |
| `11-leg-resume-fast-arm-raw-serial.txt` | that boot's serial, 13 of 14 census lines `untagged` above 0, climbing across the poll/TTY/bsshd phase |
| `12-prod-boot-r157.txt` | the production gate at the R157 head: PASS, 15 census lines |
| `12-prod-boot-r157-serial.txt` | that boot's serial, ending `untagged=0:tagged=23871:kernel=26903:cleared=49986` |
| `13-strict-boot-r157.txt` | the strict gate at the R157 head: PASS 1/1 |
| `13-strict-boot-r157-serial.txt` | that boot's serial, 13 census lines, all `untagged=0` |
