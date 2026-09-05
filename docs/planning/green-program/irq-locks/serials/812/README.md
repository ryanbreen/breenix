# #812 — captured runs

Provenance for the 14 artifacts beside this file, all captured 2026-09-05 on
branch `fix/812-try-manager-masked` off `origin/main` at `be412ee9`. The
narrative that reads them is
`docs/planning/green-program/irq-locks/812-FIX-2026-09-05.md`.
claim-lint:ok: 14 of 14 files beside this README have a row below.

Host: this Mac (aarch64, QEMU HVF), except rows 10-12, which ran in the
`breenix-x86` Incus container on beast with
`BREENIX_GATE_TMP=/root/breenix-p812-tmp` and a private clone at
`/root/breenix-p812`.

The two boot serials (01, 02) are byte-exact QEMU captures and carry no added
header, because both are replayed through the strict gate's scoring-only mode
and an inserted line would be scored. Their provenance is here instead. The
derived artifacts (07, 08, 09) carry their own header lines.

| file | what it is |
|---|---|
| `01-red-unrepaired-serial.txt` | one boot of a kernel built from this branch with `kernel/src/process/mod.rs` reverted to `origin/main` — the oracle present, the repair absent. The oracle prints `masked_in_hold=0:...:stalled=1:...:FAIL` and the boot never reaches `[EXEC_SMOKE:TARGET_OK]`. QEMU command line identical to the strict gate's, output directory private |
| `02-green-repaired-serial.txt` | the same boot with the repair in place: `masked_in_hold=1:sends=12:hold_us=12028:netrx_pending_at_release=1:received=12:stalled=0:...:PASS` |
| `03-strict-x20-run1.txt` | `docker/qemu/run-aarch64-boot-test-strict.sh`, run 1 of 3: 20/20, 226 s |
| `04-strict-x20-run2.txt` | run 2 of 3: 20/20, 244 s |
| `05-strict-x20-run3.txt` | run 3 of 3: 20/20, 238 s |
| `06-aarch64-prod-profile.txt` | `docker/qemu/run-aarch64-prod-profile-boot-test.sh` at the final head of this branch: PASS, `Observed IRQ-hold oracle marker count: 0` |
| `07-score-red-and-green.txt` | the strict gate's scoring-only mode run over the committed copies of 01 and 02, with exit statuses |
| `08-gate-mutations.txt` | the strict gate's scoring-only mode over 4 mutations of the green serial: verdict flipped, line deleted, `masked_in_hold` zeroed, `stalled` raised |
| `09-ratchet-source-mutation.txt` | `cargo test --test teardown_structure try_manager_is_a_masked_acquisition_on_every_arch` with the `msr daifset, #0xf` line deleted from `try_manager()`'s aarch64 arm: exit 101 |
| `10-x86-boot-tests-beast.txt` | `docker/qemu/run-x86-boot-tests.sh 1` on beast: `x86 frame-custody gate run 1: PASS`, `GATE_EXIT=0` |
| `11-x86-prod-profile-beast.txt` | `docker/qemu/run-x86-prod-profile-boot-test.sh` on beast: PASS, `test-only marker '[IRQ_HOLD_ORACLE:': 0`, `PROD_GATE_EXIT=0` |
| `12-x86-build-beast.txt` | the beast x86 build check: `cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi`, `BUILD_EXIT=0`, 0 `^(warning\|error)` lines |
| `13-claim-lint-selftest.txt` | `scripts/test_claim_lint.py`: `Ran 72 tests`, `OK` |
| `14-structure-suites.txt` | each `tests/*_structure.rs` suite run one at a time at the final head: 31 of 31 green, 569 cases |
