# #827 landing-run hard_timeout specimen, 2026-09-05

Native (this Mac) evidence from the `ld-827` landing worktree's re-smoke of
`gates/827-per-boot-host-facts` merged with `origin/main`, kernel built with
`--features boot_tests`. The narrative that reads this file is
`../../GATE-BOOT-FACTS-827-2026-09-05.md`, "Landing re-smoke" section.

| file | what it is |
|---|---|
| `boot2-hard_timeout-serial.txt` | The preserved failure serial for boot 2 of the 20-iteration strict-gate run, the boot whose `[GATE_BOOT_FACTS:...]` line read `ended_by=hard_timeout`. Shows the TTY oracle completing 14/14, a `[heartbeat]` line at `uptime_ms=19133`, `exec_smoke` launched (`[EXEC_SMOKE:LAUNCH]`), and 0 crash markers before the poll loop's 18s bound ran out -- the pre-adjudicated host-wall-clock-budget signature the round doc's own "What is NOT claimed" section named as not yet reproduced by a live specimen. |

The other 19 boots of that run, and the 1 boot of the separate
prod-profile run, each read `ended_by=scored_pass` and left no failure
serial behind (only `report_gate_failure` preserves one, and it runs only
on a boot the gate scores as failed).
