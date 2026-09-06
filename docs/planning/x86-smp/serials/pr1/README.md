# #814 PR-1 evidence, 2026-09-05

Branch `x86/smp-pr1-madt-enum`, head `e2717b88`, based on `origin/main`
`39169922`. The narrative that reads these files is
`../../PR1-ENUM-2026-09-05.md`.

x86 evidence was taken on beast (the `breenix-x86` Incus VM) in the clone
`/root/breenix-smp1`, with `BREENIX_GATE_TMP=/root/breenix-smp1-tmp` (R18).
aarch64 evidence was taken on the Mac in the worktree this branch was authored
in. The gate reuses one working directory per leg
(`/root/breenix-smp1-tmp/breenix_x86_smp_enum_<leg>/`) and clears it at the
start of each run, so the container holds only the last run to use each path —
the oracle and mutation runs overwrote two of the green run's three. The
transcripts and excerpts committed here, taken from each run's own files at the
time, are the record.

| file | what it is |
|---|---|
| `final-gate-3legs.txt` | The whole `run-x86-smp-enum-gate.sh 1 2 4` run at the branch head: the build, the booted image sha256, and the three legs' marker lines and verdicts |
| `leg-serial-excerpts.txt` | Per leg: serial line counts, the marker with 2 lines of boot context either side, the marker's occurrence count in each serial file, and the three existing boot_tests pass markers quoted from the serial |
| `oracle-main-smp2.txt` | The oracle's red arm: `origin/main`'s kernel bytes (git status and an empty `git diff origin/main -- kernel/` are in the header) with this branch's gate script, at `-smp 2`. The gate's two marker assertions fail; its three pass-marker checks do not |
| `mut-m1-smp1-smp2.txt` | Mutation M1: `madt_cpus` hardcoded to 1, run on legs 1 and 2. The diff is in the header. Leg 1 stays green and leg 2 reddens, which is the reason the gate runs more than one leg |
| `ratchet-mutations.txt` | Mutations M2 and M3 against `tests/x86_smp_enum_structure.rs`, with the unmutated baseline and the restored tree either side, and each leg's real exit status |
| `x86-boot-tests-x1.txt` | `docker/qemu/run-x86-boot-tests.sh 1` at this head |
| `x86-prod-profile.txt` | `docker/qemu/run-x86-prod-profile-boot-test.sh` at this head, and the enumeration marker as the zero-feature profile prints it |
| `aarch64-strict-x1.txt` | `docker/qemu/run-aarch64-boot-test-strict.sh 1` at this head |
| `resmoke-merged-head.txt` | The re-smoke after `origin/main` was merged in at `98ba6e64`: the zero-warning build, the three-leg gate, the boot-tests gate, the production-profile gate, and the marker in the zero-feature serial |
| `aarch64-strict-merged-head.txt` | `docker/qemu/run-aarch64-boot-test-strict.sh 1` at the merged head |
| `aarch64-section-comparison.txt` | The aarch64 kernel built from this branch against the same kernel built from its base, compared section by section, plus the two same-source builds that establish what a build-to-build difference looks like |
