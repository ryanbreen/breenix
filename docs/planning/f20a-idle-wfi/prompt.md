# F20a Secondary CPU Idle WFI Fix

Frozen contract for this run: fix the ARM64 Parallels idle CPU host-spin issue without changing the timer interrupt path, generic IRQ dispatch, GIC admission, or interrupt-driven scheduler semantics.

Required branch: `fix/f20a-idle-wfi` off `main` at `ba76e841`.

Core goals:
- Identify where secondary CPUs go after PSCI `CPU_ON`.
- Land the minimal correct fix so secondary CPU idle waits with `wfi` instead of hot-spinning.
- Validate with five `./run.sh --parallels --test 45` runs plus host-side Parallels CPU sampling.

Hard prohibitions:
- Do not touch `kernel/src/arch_impl/aarch64/timer_interrupt.rs`.
- Do not touch `kernel/src/arch_impl/aarch64/exception.rs::handle_irq`.
- Do not touch GIC admission code or interrupt priority behavior.
- Do not add polling fallbacks to interrupt-driven wakeups.
- Do not revert fixes from PRs #305, #308, #309, or #312.
- Do not touch files prohibited by project instructions.

Pass criteria for each validation run:
- `[init] Boot script completed` appears in serial.
- `grep -c "T[0-9]" /tmp/breenix-parallels-serial.log` is at least 24.
- Host CPU average is below 150%.
- Host CPU peak is below 400%.

Mandatory outputs:
- `summary.txt` with five-run table.
- `decisions.md` if any validation fails or non-trivial decisions are made.
- `exit.md` with verdict, summary table, before/after host CPU, and self-audit.
