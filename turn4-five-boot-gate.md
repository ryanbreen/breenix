# Turn 4 Five-Boot Stress Gate

## A. Harness Script

Harness: `turn4-artifacts/run_5boot_stress.sh`

The harness is prepared but the 5-boot gate was not run to completion in this turn. It launches `./run.sh --parallels --no-build`, detects the fresh epoch-named `breenix-*` VM from `run.out`, uses the `run.out` serial tail as the authoritative serial stream, waits for 220 seconds of active rendering, classifies each boot against the Turn 4 criteria, captures a best-effort `gdb-state.log`, and writes `metrics.tsv` plus `aggregate-result.txt`.

## B. Per-Boot Result Table

| Boot | Result | Reason |
| --- | --- | --- |
| 1 | not run | Aborted during harness shakedown; initial harness version used `/tmp/breenix-parallels-serial.log` as the sole serial source, but Parallels/run.sh can remove that path while continuing to tail serial into `run.out`. |
| 2 | not run | Blocked by Parallels resource contention. |
| 3 | not run | Blocked by Parallels resource contention. |
| 4 | not run | Blocked by Parallels resource contention. |
| 5 | not run | Blocked by Parallels resource contention. |

## C. Aggregate Metrics

No aggregate metrics were generated. The gate requires five valid 220-second active-rendering boots; this turn produced zero valid boots.

## D. Headline

The 5-boot Parallels gate did not run. A sibling AHCI Ralph was concurrently running its own Parallels `breenix-*` 5-boot gate from `/Users/wrb/fun/code/breenix.worktrees/ahci-interrupt-driven`, using the same VM namespace and live `prlctl` operations. Continuing the virtio-gpu gate would risk stopping or deleting the sibling's active VMs, which would violate the AHCI off-limits constraint.

The reusable Turn 4 harness is ready for a retry once the Parallels VM namespace is free.

## E. Status

BLOCKED.

Proposed Turn 5: after the sibling AHCI Parallels gate has exited, rerun `./turn4-artifacts/run_5boot_stress.sh` from this worktree and evaluate all five boots against the Turn 4 criteria. No kernel code changes are needed before the retry.
