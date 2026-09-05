# Slice 3d — captured runs

The 14 artifacts behind
`docs/planning/green-program/aarch64-testing/SLICE3D-2026-09-05.md` (PR #824),
recorded on `sched/562-slice3d-pinned-wake-liveness`.

`01-strict-boot1-serial.txt` has been re-recorded in place, for the same
structural reason `docs/planning/green-program/aarch64-testing/serials/asid-ratchet/03-strict-boot1-serial.txt`
was during the `#796` and `#812` landings: `tests/loopback_pump_structure.rs`
and `tests/ttbr0_shadow_reconciliation_structure.rs` each replay this file
through `docker/qemu/run-aarch64-boot-test-strict.sh`'s scoring-only mode, and
`fix/812-try-manager-masked` adds a required `IRQ_HOLD_ORACLE` line to that
scorer that PR #824's original capture (`BUILD_ID 006a9c05f41695`) predates —
the merge of the two branches (`p812-land3`) therefore scored the old content
`SCORE: FAIL - IRQ-hold oracle marker missing or failed`. The replacement is a
strict-gate boot at the merged head `9ff5c392` (`BUILD_ID 006a9c528e2cdf`),
carrying `[IRQ_HOLD_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=11:hold_us=12034:netrx_pending_at_release=1:received=11:stalled=0:hold_done=1:joined=1:PASS]`
alongside the pre-existing `FCNTL_PM_CONTENTION_ORACLE` PASS line, the
all-zero `PINNED_HOME_CPU_UNAVAILABLE` census and 13 `TTBR0_ASID_CENSUS` lines
all reading `untagged=0`; the strict scorer accepts it (`SCORE: PASS`).

`02-prod-boot1-serial.txt` was checked against the merged head's production
scorer directly (`BREENIX_PROD_SCORE_ONLY=... run-aarch64-prod-profile-boot-test.sh`)
and still scores `PASS` — the production scorer only asserts the
`IRQ_HOLD_ORACLE` marker's *absence* (`count=0`), which the R157 branch does
not change, so it needed no re-record.

The first capture attempt of `01-strict-boot1-serial.txt` at this landing was
discarded before it reached this file: the aarch64 strict and
production-profile gates write to a fixed `/tmp/breenix_aarch64_strict_N` path
that no `BREENIX_GATE_TMP` override reaches (disclosed the same way in
`docs/planning/green-program/irq-locks/serials/812/README.md`), and a
concurrent lane's boot landed in that path mid-capture — its serial carried a
`FCNTL_PM_CONTENTION_ORACLE` line with fields (`arm_wait_us`, `acquired`,
`hold_safety`) this branch's kernel binary does not emit (`strings` on the
compiled ELF shows only the `attempts=` form). Caught by checking the captured
serial's oracle-line shape against the built binary's own `strings` output
before adopting it; the second capture, taken with `pgrep -fl
'qemu-system-aarch64 -M'` reading 0 immediately before and after launch,
matched.

`01-strict-boot1-serial.txt` was re-recorded a second time during the
`fix/819-fcntl-oracle-arming-rendezvous` landing (`ld-fcntl-arm`), for the
same structural reason as the paragraph above: `#819` rewrites the
`FCNTL_PM_CONTENTION_ORACLE` marker itself, from a boolean-attempt shape
(`attempts=1:armed=1:...`) to a 7-named-arm rendezvous shape
(`arm_wait_us=...:armed=1:acquired=1:...:hold_safety=0:...`), and changed
`docker/qemu/run-aarch64-boot-test-strict.sh`'s required pattern to match.
Merging `origin/main` (carrying the `#812` re-record above, with the old
`attempts=` shape) into `fix/819-fcntl-oracle-arming-rendezvous` (carrying the
new `arm_wait_us=` shape but no `IRQ_HOLD_ORACLE` line, since it branched
before `#812`) left no single side's copy able to satisfy both scorer
requirements at once, and the merge's file-level conflict resolution (taking
`origin/main`'s copy outright) scored
`SCORE: FAIL - fcntl process-manager contention oracle marker missing or failed`
in both replay tests. The replacement is a strict-gate boot at the merged
head (`BUILD_ID 006a9c732f0d64`), carrying
`[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=93:armed=1:acquired=1:holder_cpu=1:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=8107:hold_safety=0:hold_done=1:joined=1:PASS]`
alongside an `IRQ_HOLD_ORACLE` PASS line, the all-zero
`PINNED_HOME_CPU_UNAVAILABLE` census, and 18851 `TTBR0_ASID_CENSUS` tagged
entries all reading `untagged=0`; both replay tests pass again
(`both_aarch64_gates_fail_on_a_pinned_placement_refusal`,
`both_aarch64_gates_fail_on_an_untagged_publish`). Checked against the built
kernel's own `strings` output (the `arm_wait_us=` format string) before
adopting it, and taken with the host's aarch64 QEMU count read as 0
immediately before launch (`docker/qemu/run-aarch64-boot-test-strict.sh`
itself now honors `BREENIX_GATE_TMP` for its output directory, so this
capture did not need the discard-and-retake step the paragraph above
required).

| file | what it is |
|---|---|
| `01-strict-boot1-serial.txt` | re-recorded twice: during `#812` landing (a strict-gate boot at merged head `9ff5c392`, `BUILD_ID 006a9c528e2cdf`, adding the `IRQ_HOLD_ORACLE` PASS line), then during `#819` landing (a strict-gate boot at the `ld-fcntl-arm` merged head, `BUILD_ID 006a9c732f0d64`, updating the `FCNTL_PM_CONTENTION_ORACLE` line to the rendezvous marker shape) |
| `01-strict-x3.txt` | PR #824's strict gate, 3 iterations, at its own shipping head: `PASS: 3/3 boots succeeded` |
| `02-prod-boot1-serial.txt` | PR #824's original production-gate boot serial; unchanged — still scores `PASS` against the merged head's production scorer |
| `02-prod-boot1.txt` | PR #824's production-gate run 1 of 3 at its own shipping head |
| `03-prod-boot2.txt` | PR #824's production-gate run 2 of 3 |
| `04-prod-boot3.txt` | PR #824's production-gate run 3 of 3 |
| `05-runtime-anti-vacuity-strict-gate.txt` | PR #824's anti-vacuity leg: the strict gate against a kernel with the pin-hold disposition mutated out — `FAIL: Only 0/1 boots succeeded` |
| `05-runtime-anti-vacuity-strict-serial.txt` | that failing boot's serial |
| `05b-anti-vacuity-discarded-host-load.txt` | a discarded strict-gate run from the same anti-vacuity leg, superseded by `05-runtime-anti-vacuity-strict-gate.txt` |
| `06-x86-prod-gate.txt` | PR #824's x86 production-profile gate on beast (`/root/breenix-slice3d`): `PASS` |
| `07-x86-boot-tests-gate.txt` | PR #824's x86 boot-tests gate on beast: `PASS`, with the full oracle census printed |
| `08-x86-build-testing.txt` | x86 build check, `--features testing,external_test_bins`: clean, 0 warnings/errors |
| `09-x86-build-zero-feature.txt` | x86 build check, no features (the shipped profile): clean, 0 warnings/errors |
