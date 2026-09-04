# Round-8 R13 smoke — 3 boots per profile at the round-8 head

Each of the 17 artifacts beside this README was produced by running the command
named beside it in this worktree, at the head this directory is committed with.

| profile | command | artifacts |
|---|---|---|
| strict (`boot_tests`) | `./docker/qemu/run-aarch64-boot-test-strict.sh 3` | `strict-3boots-run-log.txt`, `strict-boot{1,2,3}-serial.txt` |
| production (no features) | `./docker/qemu/run-aarch64-prod-profile-boot-test.sh` x3 | `prod-boot{1,2,3}-run-log.txt`, `prod-boot{1,2,3}-serial.txt` |
| testing (`--features testing`) | `./docker/qemu/run-aarch64-testing-profile-boot-test.sh 3` | `testing-3boots-run-log.txt`, `testing-boot{1,2,3}-serial.txt` |

`neon-guard-*.log` is `scripts/check-kernel-no-neon.sh` against each profile's
artifact.

## Verdicts

* **strict: 3/3.** `[OK] Boot 1: SUCCESS`, `[OK] Boot 2: SUCCESS`,
  `[OK] Boot 3: SUCCESS`, `PASS: 3/3 boots succeeded`.
* **production: 1/3.** `FAIL: seam-absent timeout marker count must be exactly
  one`, `FAIL: Poll TCP oracle marker missing`, then
  `PASS: production profile reached bsshd with the futex oracle seam absent`.
  The second of those is the R7-003 signature, reproduced on the head; the A/B
  that attributes it is `../prod-ab/VERDICTS.md`.
* **testing: marker + loaded 3/3; post-loader lockup, #728-signature 3/3;
  unattributed lockups 0/3; userspace panics counted per boot.** The gate's own
  summary line is `PASS-WITH-ATTRIBUTED-LOCKUP`, never "clean" — a boot with a
  five-second lockup dump in it is not a clean boot, which is what review
  finding R7-004 was about.

claim-lint:ok: 3, 3 and 3 boots, `strict-3boots-run-log.txt`,
`prod-boot1-run-log.txt` and its two siblings, `testing-3boots-run-log.txt`.

## Re-scoring any of these serials

The testing gate reads a committed serial back through the same function that
scored it live:

```
$ ./docker/qemu/run-aarch64-testing-profile-boot-test.sh --classify \
      docs/planning/green-program/aarch64-testing/serials/r8/smoke/testing-boot1-serial.txt
```
