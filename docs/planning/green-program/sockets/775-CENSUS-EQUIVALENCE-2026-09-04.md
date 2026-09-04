# #775 dispatch strand census equivalence

## Scope and method

The comparison ran on beast in the `breenix-x86` Incus container from the
fresh clone `/root/breenix-775`, at migration commit `29344251`.  The three
dispatch records were still compiled in: this commit precedes their removal
and the profile did not enable `quiet_dispatch_log`.

<!-- claim-lint:ok: commit 29344251 is the table's tested revision; its parent is main at bfbb7575 and git diff 29344251^..29344251 retains all 3 cfg-gated records. -->

Five boots used the release `testing,external_test_bins` profile through the
full x86 gate.  The gate caps one invocation at four sequential boots, so the
battery was run as `run-x86-gate.sh 4 full` plus `run-x86-gate.sh 1 full`.
Both invocations reported a clean zero-warning build and a combined 5/5 pass.

<!-- claim-lint:ok: beast artifacts /root/775-equivalence-gate4.txt and /root/775-equivalence-gate1.txt record 4/4 and 1/1 PASS plus "Build clean (0 warnings)". -->

For each captured `serial_user.log` plus `serial_kernel.log`, the old result
was produced by the parent revision of `scripts/x86-strand-census.sh` through
`bash <(git show HEAD^:scripts/x86-strand-census.sh)`.  The migrated result
was produced by the script at `29344251`.  Exit codes and both census fields
were captured independently for each invocation.

## Boot-by-boot comparison

| Boot | Old string census: threads_saved_blocked | Migrated census: threads_saved_blocked | Old string census: stranded | Migrated census: stranded | Old rc | Migrated rc |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 11 | 11 | 0 | 0 | 0 | 0 |
| 2 | 11 | 11 | 0 | 0 | 0 | 0 |
| 3 | 11 | 11 | 0 | 0 | 0 | 0 |
| 4 | 11 | 11 | 0 | 0 | 0 | 0 |
| 5 | 11 | 11 | 0 | 0 | 0 | 0 |

<!-- claim-lint:ok: 5 of 5 paired invocations printed old_saved=11 old_stranded=0 migrated_saved=11 migrated_stranded=0; raw serials are /root/775-equivalence/boot_{1..5}/ on beast. -->

The migrated census agrees with the old string census boot by boot on both
required numbers: 5 of 5 for `threads_saved_blocked` and 5 of 5 for
`stranded`.  Both methods were non-vacuous on the saved-thread population;
each observed 11 distinct saved-blocked TIDs per boot.

<!-- claim-lint:ok: the 5-row table above is the complete battery and every row has 11/11 and 0/0 equality. -->

## Post-removal anti-vacuity

To be appended after the three dispatch records and `quiet_dispatch_log`
feature are removed, using three fresh boots at the new head.
