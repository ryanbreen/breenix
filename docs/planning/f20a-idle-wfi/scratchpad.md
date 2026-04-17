# F20a Scratchpad

## 2026-04-17

Starting milestone 1: trace secondary CPU bring-up and idle execution. I will inspect `smp.rs`, `context_switch.rs`, `scheduler.rs`, and the prior ARM64 CPU0 SMP investigation notes, with the explicit constraint that timer, IRQ dispatch, and GIC admission files are read-only for this run.

Milestone 1 findings:
- `secondary_cpu_entry_rust()` registers the per-CPU idle thread, enables interrupts, marks the CPU online, then loops in `wfi`.
- `create_and_register_idle_thread()` points the scheduler's secondary idle context at `idle_loop_arm64`.
- `idle_loop_arm64()` also waits with `wfi`, but all current ARM64 idle waits are bare `wfi`.
- Linux ARM64's idle primitive does `dsb(sy); wfi();` in `arch/arm64/kernel/process.c`.

Starting milestone 2: add a shared ARM64 `arch_cpu_idle()` helper using `dsb sy; wfi`, then replace the secondary idle waits and the scheduler idle-loop wait with that helper. This keeps the timer interrupt, generic IRQ dispatch, and GIC paths unchanged.

Milestone 2 implementation:
- Added `arch_cpu_idle()` in `kernel/src/arch_impl/aarch64/cpu.rs`.
- Swapped the raw `wfi` waits in `kernel/src/arch_impl/aarch64/smp.rs` for the helper.
- Kept `idle_loop_arm64()`'s interrupt unmasking but moved its wait into the helper.
- `cargo fmt` failed on pre-existing trailing whitespace in `tests/shared_qemu.rs`; I reverted the formatter's unrelated edits and kept only the intended files above.

Starting milestone 2 validation: run the aarch64 kernel build and check the compile output for any warnings or errors.

Milestone 2 validation result:
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` completed successfully.
- `rg '^(warning|error)' logs/f20a/aarch64-build.log` produced no output.

Starting milestone 3: run five counted Parallels 45-second sweeps. The first attempted sweep was discarded because the sampler selected unrelated existing Parallels helper processes; counted runs will sample the active `prl_vm_app` VM process directly.

Milestone 3 failure:
- Counted run 1: `exit_status=1`, `boot_script_completed=0`, `timer_tick_count=11`, `host_cpu_avg=105.2%`, `host_cpu_peak=106.6%`.
- Serial reached `[init] Breenix init starting (PID 1)` and then stopped after `F123456789SCT9T0`.
- Correction before rerun: keep the `dsb sy; wfi` helper scoped to actual idle loops. Reverted the generic `CpuOps::halt()` / `halt_with_interrupts()` routing because that path is used by blocking syscall waits and is not part of the secondary idle fix.
- Rerun after restoring generic syscall waits still failed at the same point (`boot_script_completed=0`, `timer_tick_count=12`, host CPU avg `105.1%`).
- Next correction: restore `idle_loop_arm64()` to its prior bare `wfi` sequence and apply `arch_cpu_idle()` only to secondary CPU bring-up idle loops in `smp.rs`.
- Secondary-only run completed init but failed CPU criteria: `host_cpu_avg=595.0%`, `host_cpu_peak=697.4%`.
- Root-cause refinement: the spin is in `idle_loop_arm64()`. The previous low-CPU variant split interrupt unmasking and WFI with a Rust function call, likely creating a lost-wake window. Next attempt keeps one inline assembly block: `msr daifclr, #0xf; dsb sy; wfi`.
- Inline `daifclr; dsb sy; wfi` also stalled init (`boot_script_completed=0`, host CPU avg `105.7%`). Next attempt moves the barrier before `daifclr`, preserving the original adjacent `daifclr; wfi` wake behavior.
- `dsb sy; daifclr; wfi` still stalled init (`boot_script_completed=0`, timer ticks `13`, host CPU avg `105.6%`). Next attempt uses `daifclr; isb; wfi` to synchronize interrupt unmasking without the DSB placement that consistently causes init stalls.
