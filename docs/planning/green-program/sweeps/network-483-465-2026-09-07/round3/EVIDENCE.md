# Round-3 evidence: #483/#465 clause 5 (five-run 120s sweep) + clause 4/3 (wait_stress receipt)

Detached worktree `W` = `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483r3/wt`, created via `git worktree add --detach W origin/main`.

**SHA: `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`** (`git rev-parse HEAD` in `W`; `git status --short` empty). This is later than the requested floor `af5d7a3cd5cbfbe275347f74750cc6456331edb9`: `git merge-base --is-ancestor af5d7a3c HEAD` exits 0, and `git rev-list --count af5d7a3c..HEAD` = 9 (nine commits ahead, including PR #925's own round-2 evidence commit and the #920/#924 merges).

Every one of the 7 boot commands run in this round (1 wait-stress run, 1 clean-rebuild throwaway boot, 5 lifecycle runs) ran with `BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library` exported; `BREENIX_WAIT_STRESS` was unset for all 6 non-wait-stress commands and set to `1` for the 1 wait-stress command. All 7 commands built the plain production kernel (no `--features`; confirmed by `run.sh` never invoking `cargo build` with a `--features` flag anywhere in the 7 preserved `stdout.log` files under `N/`). claim-lint:ok: 7/7 stdout.log files under N/ checked individually, see File index below. Evidence root `N` = `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/v483r3/evidence`.

## Deviation: run 1 was aborted and redone

The first `--test 120` attempt (`N/run1-test120-ABORTED-waitstress-contaminated/`) ran with `--no-build` immediately after the wait-stress build, which reused the wait-stress build's ext2 disk image. `scripts/create_ext2_disk.sh:280,560` writes `/etc/wait_stress.enabled` into the ext2 image whenever `BREENIX_WAIT_STRESS=1` was set at disk-creation time; `--no-build` does not regenerate the ext2 disk (build step `[4/4]` is skipped), so that flag file survived into what was meant to be a plain lifecycle boot. Its `serial.log` shows `WAIT_STRESS_PROGRESS` lines interleaved with the ordinary boot (e.g. line 645), which is not the plain-production-kernel lifecycle boot the task asked for. VM `breenix-1788771062` was stopped (`prlctl stop --kill`), polled to `stopped`, and deleted before proceeding. A full rebuild without `BREENIX_WAIT_STRESS` was then run (`N/rebuild-clean/stdout.log`) and confirmed clean: no `Enabled wait_stress init gate` line appears in that build's output. Runs 1–5 below are all from that clean build, sharing one `target/parallels/*.hdd` pair (`Existing Parallels disk ... already up to date` on every subsequent `--no-build` invocation, confirming no rebuild drift between runs).

## Wait-stress receipt

Command: `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` (VM `breenix-1788770699`, single run). Preserved: `N/waitstress-150/{stdout.log,serial.log,screenshot.png,VM.txt,SHA.txt}`.

- `N/waitstress-150/serial.log:1613`: `WAIT_STRESS_PASS entered=1126355 returned=1126354 wakes=43165784 waiters=0`
- STALL count: `grep -c STALL N/waitstress-150/serial.log` = **0**
- SHA recorded in `N/waitstress-150/SHA.txt`: `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`
- VM `breenix-1788770699` was stopped, polled to `stopped`, and deleted after the run (a subsequent `prlctl list --all` shows exactly 1 VM, `linux-probe`, suspended; 0 `breenix-*` VMs remain). claim-lint:ok: 1/1 VM created for this run, individually stopped/polled/deleted.

## Five-run 120s lifecycle sweep

Each run: fresh epoch-named VM created by `run.sh --parallels --test 120 --no-build`; serial log truncated (`> /tmp/breenix-parallels-serial.log`) immediately before start; first capture is `run.sh`'s own `capture-display.sh` screenshot (`screenshot-1.png`); second capture taken via `prlctl capture <vm> --file screenshot-2.png` before stopping the VM; VM then `prlctl stop --kill`, polled to `stopped`, then `prlctl delete`d. `ELAPSED_TO_FIRST_CAPTURE` is the host wall-clock from immediately before `run.sh` was invoked (`date +%s`) to immediately after it exited (`date +%s`), covering VM creation/config + the 120s sleep + capture-retry time; it is not the boot time alone. Capture-delta is the file-mtime gap between `screenshot-1.png` and `screenshot-2.png`.

| Run | VM | Wall-clock to 1st capture | Capture delta (1→2) | Verdict 1 | Verdict 2 | init | bsshd started | Boot script completed | bounce started | Heartbeats (count / last uptime_ms) | FPS min/max (samples, <160 count) | Fault markers | PARALLELS_CAPTURE |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | breenix-1788771554 | 179s | 15s | **FAIL** (distinct=10, dominant=(0,0,0), coherent_region=None) | **PASS** (distinct=1157, coherent bbox_frac=0.1466) | serial.log:391 | serial.log:591 | serial.log:637 | serial.log:670 | 152 / 149493 | 192/267 (146 samples, 0 <160) | 0 | stdout.log:83 method=prlctl reason=ok |
| 2 | breenix-1788771793 | 220s | 83s (deviation: intended ~15s; a 30s-timeout Bash call cut the capture step short and it was retried in the next call, so the gap is real 83s, not 15s) | **FAIL** (distinct=150, dominant=(0,0,0), dom_frac=0.9500 -- fails the dom_frac<0.90 gate despite a present coherent_region) | **PASS** (distinct=2002, coherent bbox_frac=0.4424) | serial.log:391 | serial.log:592 | serial.log:639 | serial.log:671 | 263 / 260645 | 193/497 (257 samples, 0 <160) | 0 | stdout.log:83 method=prlctl reason=ok |
| 3 | breenix-1788772193 | 206s | 19s | **PASS** (distinct=1840, coherent bbox_frac=0.1477) | **FAIL** (distinct=19, dominant=(0,0,0), coherent_region=None) | serial.log:391 | serial.log:592 | serial.log:643 | serial.log:661 | 171 / 168621 | 182/256 (165 samples, 0 <160) | 0 | stdout.log:83 method=prlctl reason=ok |
| 4 | breenix-1788772443 | 169s | 18s | **FAIL** (distinct=6, dominant=(0,0,0), coherent_region=None) | **FAIL** (distinct=631, dominant=(0,0,0), dom_frac=0.9228 -- fails the dom_frac<0.90 gate despite a present coherent_region at bbox_frac=0.0706) | serial.log:390 | serial.log:591 | serial.log:638 | serial.log:670 | 155 / 152538 | 163/249 (149 samples, 0 <160) | 0 | stdout.log:83 method=prlctl reason=ok |
| 5 | breenix-1788772676 | 160s | 19s | **PASS** (distinct=1900, coherent bbox_frac=0.4424) | **PASS** (distinct=867, coherent bbox_frac=0.1466) | serial.log:391 | serial.log:592 | serial.log:645 | serial.log:671 | 149 / 146594 | 123/262 (143 samples, 13 <160) | 0 | stdout.log:83 method=prlctl reason=ok |

Each of the 10 `Verdict` cells (5 runs x 2 captures) was produced by `bash scripts/f24-render-verdict.sh <run>/screenshot-N.png` run from `W`; PASS = exit 0, FAIL = exit 1 (`VERDICT=FAIL` printed, distinct from the CAPTURE_MISSING exit-2 case, which occurred in 0 of the 10 captures in the table above). claim-lint:ok: 10/10 captures individually scored, see the per-run screenshot-1.png/screenshot-2.png files under N/run{1..5}-test120/. Fault-marker count is `grep -c -E "AHCI.*[Tt]imeout|FATAL|PANIC|DATA_ABORT|SOFT_LOCKUP" <run>/serial.log`; every run returned 0. `init`/`bsshd started`/`Boot script completed`/`bounce started` cells cite the line number of `[init] Breenix init starting (PID 1)`, `[init] bsshd started (PID ...)`, `[init] Boot script completed`, and `[init] bounce started (PID ...)` respectively in that run's `serial.log`. Heartbeat count is `grep -c heartbeat <run>/serial.log`; the paired uptime_ms is from that run's last `[heartbeat]` line — heartbeats continue accumulating past the 120s test window because the VM stays running through both captures before being stopped, so the last heartbeat's `uptime_ms` exceeds 120000. FPS min/max/sample-count/below-160-count come from every `instantaneous_fps=` value inside `[bwm-fps]` lines in that run's `serial.log` (`grep -oE 'fps=[0-9]+' <run>/serial.log | sed 's/fps=//' | sort -n`, confirmed to be exactly the `[bwm-fps]` lines and no other `*fps=` pattern exists in these logs).

### Outcome, stated plainly (no judgement)

- Run 5: both captures PASS.
- Runs 1, 2, 3: exactly one of the two captures PASS (run 1 and run 2 PASS on the second capture only; run 3 PASSes on the first capture only).
- Run 4: both captures FAIL.
- No fault markers (AHCI timeout / FATAL / PANIC / DATA_ABORT / SOFT_LOCKUP) in any of the 5 serial logs.
- No FPS sample below 160 in runs 1–4; run 5 has 13 of 143 samples below 160 (min 123).
- 5 of 5 runs reached the full service lifecycle (init -> bsshd -> boot script completed -> bounce, line numbers cited in the table above) and produced heartbeats past the test window.

## File index

- `N/waitstress-150/{stdout.log,serial.log,screenshot.png,VM.txt,SHA.txt}`
- `N/rebuild-clean/stdout.log` (clean-rebuild confirmation: `grep -c "Enabled wait_stress" N/rebuild-clean/stdout.log` = 0; the throwaway 1s test VM `breenix-1788771375` it booted was stopped/deleted before run 1)
- `N/run1-test120-ABORTED-waitstress-contaminated/{stdout.log,serial.log,screenshot.png,VM.txt,SHA.txt,NOTE.txt}`
- `N/run{1,2,3,4,5}-test120/{stdout.log,serial.log,screenshot-1.png,screenshot-2.png,VM.txt,SHA.txt,TIMING.txt,cap1-epoch.txt,cap2-epoch.txt}` (cap*-epoch.txt present for runs 3–5 only; runs 1–2 predate that instrumentation and are timed via file mtime instead, noted in the table)

All 8 VMs created during this round (`breenix-1788770699`, `breenix-1788771062` [aborted], `breenix-1788771375` [throwaway rebuild-clean boot], `breenix-1788771554`, `breenix-1788771793`, `breenix-1788772193`, `breenix-1788772443`, `breenix-1788772676`) were `prlctl stop --kill`, polled to `stopped`, then `prlctl delete`d -- 8 of 8, verified individually after each stop/delete pair. `prlctl list --all` at the end of this round shows exactly 1 VM, the pre-existing `linux-probe` (suspended, untouched); 0 `breenix-*` VMs remain.
