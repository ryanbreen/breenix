# F20d Phase 1 Linux Probe Ground Truth

Probe: `wrb@10.211.55.3`

Preflight:

```text
6.8.0-107-generic
probe
4
SSH_OK
```

Artifacts:

- `cpu0-idle-trace.txt`: ftrace capture over a 10 second idle window.
- `interrupts-pre`: `/proc/interrupts` before a 5 second delta window.
- `interrupts-post`: `/proc/interrupts` after a 5 second delta window.
- `cpu-idle-states.txt`: per-CPU cpuidle sysfs state capture.

## CPU 0 idle transitions

From `cpu0-idle-trace.txt`, counting CPU 0 `cpu_idle` events:

- CPU 0 enters idle (`state=1 cpu_id=0`) 187 times.
- CPU 0 exits idle (`state=4294967295 cpu_id=0`) 187 times.
- Every CPU 0 idle exit is immediately followed by a CPU 0 `irq_handler_entry` event in the next CPU 0 ftrace event.

Examples:

- First observed CPU 0 idle exit: `cpu0-idle-trace.txt:22`.
- First observed CPU 0 wake IRQ: `cpu0-idle-trace.txt:23` (`irq=2 name=IPI`).
- First observed CPU 0 idle re-entry: `cpu0-idle-trace.txt:26`.
- First observed CPU 0 arch timer wake: `cpu0-idle-trace.txt:63` -> `cpu0-idle-trace.txt:64`.

## CPU 0 wake-source distribution

Wake IRQ names where a CPU 0 `irq_handler_entry` immediately follows a CPU 0 `cpu_idle` exit:

| IRQ | Name | Count |
| --- | --- | ---: |
| 28 | `virtio2-virtqueues` | 93 |
| 10 | `arch_timer` | 48 |
| 2 | `IPI` | 34 |
| 15 | `ahci[PRL4010:00]` | 12 |

Total wake IRQs counted: 187.

## `/proc/interrupts` CPU 0 delta

The Linux probe reports the timer as IRQ line `10`, `GICv3 27 Level arch_timer`, not as `30 Edge`.

From `interrupts-pre:2` to `interrupts-post:2`:

- CPU 0 `arch_timer` pre: 15,545.
- CPU 0 `arch_timer` post: 15,679.
- Delta: 134 interrupts over 5 seconds.
- Rate: 26.8 CPU 0 arch timer interrupts per second.

## CPU 0 idle-state sysfs stats

`cpu-idle-states.txt` contains CPU headings but no `cpuidle/state*/name`, `usage`, or `time` rows:

```text
=== cpu0 ===
=== cpu1 ===
=== cpu2 ===
=== cpu3 ===
```

Observed result from the requested artifact:

- CPU 0 per-state cpuidle usage/time rows exposed through `/sys/devices/system/cpu/cpu0/cpuidle/state*`: 0.
- CPU 0 per-state time spent reported by sysfs: unavailable from this probe artifact.
- CPU 0 ftrace-observed idle state: `state=1`, with 187 entries and 187 exits.

Linux-only follow-up check showed `/sys/devices/system/cpu/cpu0/cpuidle` is absent, while global `/sys/devices/system/cpu/cpuidle` exists. Therefore the probe provides ftrace idle state transitions but not per-CPU cpuidle usage/time counters.
