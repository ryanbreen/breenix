# x86 boot-tests gate, 3 boots at `fix/693-poll-wake-loss` @ `712ae668`

`docker/qemu/run-x86-boot-tests.sh 1`, 3 separate invocations (`set -e`
aborts a multi-boot run on the first red, so each boot ran as its own
process). Fresh scratch clone of `https://github.com/ryanbreen/breenix.git`
inside the `breenix-x86` Incus VM on beast, checked out at
`fix/693-poll-wake-loss` @ `712ae668e966fa858c3a8c6e18d05e21929a3113` (this
slot's merge-forward of the branch onto `origin/main` @ `910deceb`).
`rust-fork` symlinked to the pre-built clone at `/root/breenix/rust-fork-real`
(read-only reference, unmodified); `Cargo.lock` copied from that same
pre-built checkout as a starting lockfile (pins `x86_64 = 0.15.4`, avoiding
the known 0.15.5 nightly-pin break). `userspace/programs/build.sh` (x86_64,
default arch): exit 0, 148 binaries installed, 0 warning/error lines.

## Results

| run | verdict | `PRODUCTION_REAPED_ROWS` (RING3_SMOKE fork census) | `TOMBSTONE_CENSUS` | derives to 5? |
|---|---|---|---|---|
| 1 | PASS | 5 | `resident=0:removed=7` | `0 + (7-2) = 5` |
| 2 | PASS | 5 | `resident=1:removed=6` | `1 + (6-2) = 5` |
| 3 | PASS | 5 | `resident=1:removed=6` | `1 + (6-2) = 5` |

`TOMBSTONE_FIXTURE_REMOVALS=2` (`docker/qemu/run-x86-boot-tests.sh:134`);
the gate's own derivation comment (`:248`) is `resident + (removed -
TOMBSTONE_FIXTURE_REMOVALS) == PRODUCTION_REAPED_ROWS`. All 3 of 3 runs
derive to 5, matching the census-computed `PRODUCTION_REAPED_ROWS` on the
same boot (#697's derivation, `docker/qemu/run-x86-boot-tests.sh:211-232`).
The `resident`/`removed` split differs run to run (0/7 vs 1/6) because it is
a snapshot taken before final reclaim quiesces in some boots and after in
others — a timing race in when the in-kernel census line prints relative to
background reclaim, not a defect; the immediately following
`TOMBSTONE_QUIESCE` line is `resident=0:removed=7` in 3 of 3 runs.

**0 reds across all 3 boots.** No `FAIL` line, no `UNATTRIBUTED` red, in any
of the 3 gate outputs (`grep -n FAIL` on each of the 3 files below matches
only benign counter names — e.g. `EXEC_FAILED_RELEASE_PROD`, whose own
`balance=0` — never a gate verdict). `x86 userspace gate: PASS -
exited=109 expected>=104 nonzero=0 allowlist=0` in 3 of 3 runs; `x86
frame-custody gate run 1: PASS` (the script's own `$i` loop variable is 1 on
every one of these 3 separate single-boot invocations) is the final line of
all 3 gate-output files.

## Files

Per run (`run1`/`run2`/`run3` = independent boots, not the script's own
numbering):

- `x86-boot-tests-run{N}-gate-output.txt` — full stdout+stderr of the
  `run-x86-boot-tests.sh 1` invocation (build, disk creation, boot, the
  script's ~40 marker-count assertions, final verdict).
- `x86-boot-tests-run{N}-serial_kernel.txt` — kernel-side serial log for
  that boot.
- `x86-boot-tests-run{N}-serial_user.txt` — userspace-side serial log for
  that boot.

```
claim-lint: python3 scripts/claim-lint.py --files <this README + N2 issue body> -> exit 0
```
