# Scratchpad - F32j Idle Sleep Gate + GIC SGI Admission

2026-04-19: Starting M1. Read factory runbook, claimed bead `breenix-eis`, created branch `f32j-idle-sgi-admission`, and cleaned stray QEMU processes. Next step is to make the M2/M3 code edits after confirming Linux cite lines and current Breenix behavior.

2026-04-19: M2/M3 initial code edits made. Idle loop now disables DAIF before the sleep gate, uses ordered need_resched plus ISR wake depth as the WFI gate, and calls schedule_from_kernel before/after WFI when work is visible. GIC redistributor init now enables SGI0 and SGI1 after the blanket SGI/PPI disable. Next: targeted format/build.

2026-04-19: M4/M5 stopped per factory gate. AArch64 and x86 builds were clean, wait_stress passed, and normal Parallels runs 1-2 passed. Run 3 reached bsshd/bounce, frames, render PASS, and no AHCI timeout, but lacked a clean `[bounce] Window mode` line and captured no `cpu0 ticks=` audit lines, so the required 5/5 proof failed. Filed `breenix-k16` for F32k Option 2.
