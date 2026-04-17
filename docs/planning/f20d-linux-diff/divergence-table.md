# F20d Linux vs Breenix CPU 0 Idle Divergence Table

Phase 1 Linux probe: `10.211.55.3`, kernel `6.8.0-107-generic`.

Phase 2 Breenix run: `./run.sh --parallels --test 45`.

| Observable | Linux value | Breenix value | Diverges (Y/N) | Evidence file:line |
| --- | --- | --- | --- | --- |
| CPU 0 enters idle/WFI at least once | Yes. CPU 0 enters `cpu_idle state=1` 187 times. | Yes. End audit reports `idle_count[0]=123`. | N | Linux: `logs/linux-probe-cpu0/f20d/phase1-summary.md:25`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:62` |
| CPU 0 exits/wakes from idle/WFI at least once | Yes. CPU 0 exits `cpu_idle` 187 times and every exit is followed by an IRQ entry. | Yes. End audit reports `post_wfi_count[0]=80`. | N | Linux: `logs/linux-probe-cpu0/f20d/phase1-summary.md:26` and `logs/linux-probe-cpu0/f20d/phase1-summary.md:27`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:61` |
| CPU 0 has a timer source enabled/active in the captured state | Yes. `/proc/interrupts` reports `GICv3 27 Level arch_timer` on CPU 0. | Yes. End audit reports `timer_ctl[0]=0x1`. | N | Linux: `logs/linux-probe-cpu0/f20d/phase1-summary.md:51`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:55` and `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:67` |
| CPU 0 timer ticks advance while CPU 0 is in the idle observation window | CPU 0 `arch_timer` advances by 134 interrupts over 5 seconds, 26.8/s. | CPU 0 `tick_count` is 29 at idle-arm capture and 29 at end audit; delta 0. | Y | Linux: `logs/linux-probe-cpu0/f20d/phase1-summary.md:53` through `logs/linux-probe-cpu0/f20d/phase1-summary.md:58`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:63` through `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:65` |
| CPU 0 timer wake source is observed | `arch_timer` immediately follows CPU 0 idle exit 48 times. | No CPU 0 wake INTID is observed in the Phase 2 log. | Y | Linux: `logs/linux-probe-cpu0/f20d/phase1-summary.md:43`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:69` through `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:74` |
| Other CPUs continue receiving timer/tick activity during the observation | CPU 1-3 `arch_timer` counters increase in `/proc/interrupts` pre/post. | CPUs 1-7 reach about 24.16k ticks in the same end audit. | N | Linux: `logs/linux-probe-cpu0/f20d/interrupts-pre:2` and `logs/linux-probe-cpu0/f20d/interrupts-post:2`; Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:83` through `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:89` |
| Per-CPU software-to-hardware tick attribution is identity mapped | Not captured in Phase 1 Linux artifacts. | `sw_to_hw_map=[0,1,2,3,4,5,6,7]`. | N/A | Breenix: `logs/breenix-parallels-cpu0/f20d/phase2-summary.md:54` |

## First Divergence

The first row where `Diverges` is `Y` is:

```text
CPU 0 timer ticks advance while CPU 0 is in the idle observation window
```

Phase 4 target: explain and fix why Breenix CPU 0 has timer enabled and reaches/wakes from WFI, but its CPU 0 timer tick counter does not advance across the idle observation window, while Linux CPU 0 receives `arch_timer` interrupts in the same class of idle flow.
