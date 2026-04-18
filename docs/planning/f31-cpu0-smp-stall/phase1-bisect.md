# F31 Phase 1 - Merge Commit Bisect

## Purpose

Identify which merged PR introduced the Parallels SMP regression where CPU0 stops
taking timer ticks while other CPUs continue, PPI27 remains pending, and AHCI
reads time out.

## Method

For each requested commit, boot Parallels for 60 seconds with:

```bash
./run.sh --parallels --test 60
```

Capture `/tmp/breenix-parallels-serial.log` into the factory run directory and
record:

- Whether CPU0 `tick_count` stays above 100 during the 60s boot.
- Whether AHCI reports `TIMEOUT`.
- Whether userspace lifecycle markers include bsshd and bounce.

## Results

| Commit | Contents | CPU0 tick_count evidence | CPU0 > 100? | AHCI TIMEOUT? | bsshd/bounce lifecycle | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| `c8d6d75c` | current main, PRs #321-#324 | `[ahci] tick_count=[142,8419,8419,8418,8417,8473,8416,8418]`; `cpu0_breadcrumb=107`; `PPI27_pending=1` | Yes, but stalled far below peers | Yes, bsshd and bounce reads timed out | `bsshd` failed; `bounce` failed | Bad |
| `d6aebfa1` | pre-F28, has F27+F29+F30 | No AHCI timeout dump; boot reached bsshd listen within 60s | Yes by lifecycle evidence | No | boot completed; bsshd listened; bounce not reached in captured 60s | Good |
| `25083e23` | pre-F30, has F27+F29 | `[timer] cpu0 ticks=30000` | Yes | No | boot completed; bsshd started; bounce started | Good |
| `e2fd72e2` | pre-F27, has F29 only | No AHCI timeout dump; boot reached bsshd listen within 60s | Yes by lifecycle evidence | No | boot completed; bsshd listened; bounce not reached in captured 60s | Good |
| `ab351efe` | pre-F29, known-working F26 state | `[timer] cpu0 ticks=85000` | Yes | No | boot completed; bsshd started; bounce started | Good |

## Breaker

The requested bisect did **not** confirm PR #321 / F29 as the breaker. The F29
merge commit `e2fd72e2` booted with 8 CPUs online and reached bsshd without AHCI
timeouts. The first bad commit in this requested sequence is current main
`c8d6d75c`, and its parent in the sequence, `d6aebfa1`, was good.

That points to PR #324 / F28 (`eab6455d`, merged by `c8d6d75c`) as the
regressing change, not PR #321.

Raw artifacts are under:

```text
.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase1/
```

Notable bad-current evidence:

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   cpu0_last_timer_elr=0xffff00004011dc48 cpu0_breadcrumb=107 ctl=0x1
[ahci]   CPU0_GICR_ISPENDR0=0x08000001 PPI27_pending=1
[ahci]   tick_count=[142,8419,8419,8418,8417,8473,8416,8418]
[ahci] read_blocks(267612, 2) wait failed: AHCI: command timeout
```
