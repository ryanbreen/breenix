Phase 1 SSH preflight:

```
Warning: Permanently added '10.211.55.3' (ED25519) to the list of known hosts.
6.8.0-107-generic
probe
4
SSH_OK
```

Starting Phase 1 capture: collect Linux ftrace idle/IRQ/timer/sched events, /proc/interrupts delta, and cpuidle state statistics from linux-probe at 10.211.55.3 before any Breenix experiments.

Phase 1 capture result:

- `logs/linux-probe-cpu0/f20d/cpu0-idle-trace.txt`: 2,215 lines, ftrace captured successfully after rerunning privileged tracefs setup with `sudo -S`.
- `logs/linux-probe-cpu0/f20d/interrupts-pre` and `interrupts-post`: captured across a 5 second window.
- `logs/linux-probe-cpu0/f20d/cpu-idle-states.txt`: captured, but per-CPU `cpuidle/state*` rows are absent on this probe.
- `logs/linux-probe-cpu0/f20d/phase1-summary.md`: CPU 0 entered idle 187 times and exited idle 187 times; wake distribution is virtio2-virtqueues 93, arch_timer 48, IPI 34, AHCI 12.
