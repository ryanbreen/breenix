# #789 slice 2 — preserved runs

Artifacts for `../789-SLICE2-2026-09-04.md`, added in review round 1 (finding
m4: the round-0 anti-vacuity command was not re-runnable as written and no
artifact of it was kept). Each file below is captured output, not prose; the
command that produced it is recorded here beside it.

## confirm/

| File | Produced by |
|---|---|
| `ratchet-prefix-2026-09-04.txt` | This branch's ratchet over `origin/main`'s scheduler source. Recipe in the "Pre-fix run" section of the slice doc. Expected: exit 101, 7 unadmitted. |
| `ratchet-postfix-2026-09-04.txt` | `scripts/run-structure-tests.sh teardown_structure scheduler_lock`. Expected: exit 0, 2 passed. |
| `teardown-structure-full-2026-09-04.txt` | `scripts/run-structure-tests.sh`. Expected: exit 0, 83 passed. |
| `shipped-symbol-census-2026-09-04.txt` | `llvm-nm` over `target/aarch64-breenix-kernel/release/kernel-aarch64` for the 5 functions this slice touches, plus the `check-kernel-no-neon.sh` verdict for the same binary. The kernel was a default-feature build of the soft-float target. |
| `aarch64-boot-native-verdict-2026-09-04.txt` | `docker/qemu/run-aarch64-boot-test-native.sh` against a `--features boot_tests` build. Expected: PASSED. |

## serials/

| File | Produced by |
|---|---|
| `aarch64-boot-native-2026-09-04.txt` | The serial capture of the boot-test run above, copied from `/tmp/breenix_aarch64_boot_native/serial.txt`. 861 lines. |

## Reading the census lines

`scheduler_lock_acquisitions_are_irq_safe_by_shape` prints one `site …` row per
scheduler-lock acquisition it finds, then a `totals:` line. `local_mask=true`
means the site sits in a masked region the ratchet can see; `caller_derived=true`
means it does not, and admission came from the reverse call graph instead. A row
with both false is a failure, and the `totals:` line's last field counts them.

The rows come from the ratchet itself rather than from an instrumented copy of
the test file, so a reader can regenerate any of this with one command and
compare it against the capture.
