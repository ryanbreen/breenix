# Turn 10 Runner Comparison

Directive 10A asked for the Turn 8 stress runner verbatim:

```sh
bash turn8-artifacts/run_20boot_scheduler_gate.sh 2>&1 | tee turn10-artifacts/turn8-runner-rerun.log
```

I did start that exact command. The foreground tool session reached two completed boots before the command runner itself was killed by the session lifetime while boot 3 was in progress. Both completed boots failed with the current CPU0-init-stall signature:

```text
boot-1: run_status=0 status=fail reason=failures=13;xhci_msi_irq=0/0 activity=yes max_uptime_ms=40299 cpu=5 msi=0 irq=0 irq_delta=0 lock=0 stale_not_ready=0 stale_current=0 stale_deferred=0 failures=13 data_abort=0 pid1=yes vm=breenix-1779298806
boot-2: run_status=0 status=fail reason=failures=12;xhci_msi_irq=0/0 activity=yes max_uptime_ms=35300 cpu=6 msi=0 irq=0 irq_delta=0 lock=0 stale_not_ready=0 stale_current=0 stale_deferred=0 failures=12 data_abort=0 pid1=yes vm=breenix-1779298924
```

A later attempt to rerun the same harness in the background overwrote the durable `turn10-artifacts/turn8-runner-rerun.log` and died before producing per-boot results. A final two-boot foreground attempt also died before boot 1 completed, leaving only the preserved partial tree under `turn10-artifacts/turn8-runner-partial/`.

Because the verbatim Turn 8 runner no longer passes even at 2/2 observed boots, Turn 10 skipped directive 10B's "runner passes, compare against Turn 9 path" branch and followed 10C.

The old committed Turn 8 stress artifacts still show 20/20 pass with CPU0 tick counts in the tens of thousands. Current fresh deploys fail before `/bin/xhci_counters` can run, so harness fields derived from userspace xHCI counters are zero in the current failures.
