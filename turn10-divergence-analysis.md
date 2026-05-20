# Turn 10 Divergence Analysis

The first reproducible source boundary is between `18c88a01` and `cb73f6e3`.

Passing fresh deploys:

```text
18c88a01 boot-1: xhci post-activation MSI_EVENT_COUNT=1
18c88a01 first freeze-watch: timer_ticks_cpu0=281, peers about 420
18c88a01 final freeze-watch: timer_ticks_cpu0=63067, peers about 78k-80k
```

`045dcd04` has the same no-CPU0-regression behavior. Boot 1 ran cleanly through init, xhci_counters, bwm, telnetd, bsshd, bounce, and heartbeat. Boot 2 later hit the older Turn 7 DATA_ABORT/soft-lockup path, but CPU0 continued ticking into the tens of thousands; it is not the current CPU0-at-5 init stall.

Failing fresh deploy:

```text
cb73f6e3 boot-1: xhci post-activation MSI_EVENT_COUNT=60
cb73f6e3 first freeze-watch: timer_ticks_cpu0=5, peers about 400
cb73f6e3 final freeze-watch: timer_ticks_cpu0=5, peers about 27k
cb73f6e3 panic: CPU0 timer regression, peer max=30000
```

The earliest runtime divergence is before userland service spawning:

- Passing boots print `[spawn] path='/bin/xhci_counters'`, `xhci-counters`, `bwm`, `TELNETD_LISTENING`, and heartbeat output before the first freeze-watch line.
- The failing `cb73f6e3` boot prints `[init] Breenix init starting (PID 1)` and then only freeze-watch lines; no service spawn completes.
- In the failing boot, ready queues are empty, `cur_cpu0=11`, `total_threads=11`, and the reported scheduler locks are `ok`, while CPU0 tick count is frozen at 5.

That makes the regression distinct from the older DATA_ABORT issue and from a late userspace service failure. The system reaches EL0 and PID1 starts, then CPU0 stops receiving/progressing timer ticks while the other CPUs continue to service their timers.
