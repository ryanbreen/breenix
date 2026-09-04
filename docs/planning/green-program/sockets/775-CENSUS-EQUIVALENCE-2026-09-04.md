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

After removing the three dispatch records and the `quiet_dispatch_log`
feature at `e9350b53`, three fresh boots used the same release
`testing,external_test_bins` profile through `run-x86-gate.sh 3 full`.  The
gate reported a clean zero-warning build and a 3/3 pass.

<!-- claim-lint:ok: beast artifact /root/775-post-removal-gate3.txt records head e9350b53, "Build clean (0 warnings) in 15s", and "GATE: PASS (3/3 boot tests passed; mode=full build=15s boot=600s total=626s)". -->

| Boot | Census markers | threads_saved_blocked | stranded | overflow | Census rc | Removed records |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 11 | 0 | 0 | 0 | 0 |
| 2 | 1 | 11 | 0 | 0 | 0 | 0 |
| 3 | 1 | 11 | 0 | 0 | 0 | 0 |

<!-- claim-lint:ok: raw artifacts /root/775-post-removal/boot_{1..3}/ on beast each contain one DISPATCH_STRAND_CENSUS marker with 11 saved-blocked, 0 stranded, and 0 overflow; the migrated script returned rc 0 for each. -->

The removed-record column counts the three former `context_switch.rs`
messages across each captured `serial_kernel.log`: saved kernel context,
restored kernel context, and switching to the process CR3 for a blocked-syscall
kernel return.  Each message's count is zero in 3 of 3 boots, while each
census remains non-vacuous at 11 saved-blocked threads.
No QEMU process associated with `/root/breenix-775`, these artifacts, or the
three gate run directories remained after the gate completed.
<!-- claim-lint:ok: direct fixed-string greps of all three serial_kernel.log files found saved_record_count=0 restored_record_count=0 cr3_record_count=0 for each boot; scoped ps filtering found no matching #775 QEMU process. -->

## Final validation

The final code head validated on beast was `9e64763a`.  Both of the 2 x86
builds exited 0; each output had a warning-line count of 0 and an error-line
count of 0:
<!-- claim-lint:ok: /root/775-build-x86-testing-final.txt and /root/775-build-x86-production-final.txt on beast are the full outputs; both commands exited 0 and grep counted 0 warning and 0 error lines. -->

```text
   Compiling kernel v0.1.0 (/root/breenix-775/kernel)
   Compiling breenix v0.1.0 (/root/breenix-775)
    Finished `release` profile [optimized] target(s) in 17.47s
```

```text
   Compiling kernel v0.1.0 (/root/breenix-775/kernel)
   Compiling breenix v0.1.0 (/root/breenix-775)
    Finished `release` profile [optimized] target(s) in 14.04s
```

The release ARM64 kernel build also exited 0.  Its output contained no warning
attributed to Breenix and one warning attributed to the repository's
pre-existing pinned-nightly
future-incompatibility notice for the toolchain-built `core` package, so its
raw output contains one `warning:` line:
<!-- claim-lint:ok: /root/775-build-aarch64-production-final.txt is the full output; the command exited 0 with 1 warning line naming only toolchain core and 0 error lines. The same established exception is documented in docs/planning/t3g-prb/PRB-STAGE3-GATE-RESULTS.md. -->

```text
    Finished `release` profile [optimized] target(s) in 0.61s
warning: the following packages contain code that will be rejected by a future version of Rust: core v0.0.0 (/root/.rustup/toolchains/nightly-2025-06-24-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core)
note: to see what the problems were, use the option `--future-incompat-report`, or run `cargo report future-incompatibilities --id 1`
```

The single Cargo invocation discovered and ran 26 of 26 files matching
`tests/*_structure.rs`: it produced 26 successful target summaries totaling
502 passed tests, with zero failed summaries, warnings, or errors.

<!-- claim-lint:ok: /root/775-structure-tests-final.txt on beast is the complete aggregate output; derived counts are result_count=26, passed_tests=502, failed_result_count=0, warning_count=0, error_count=0. -->
