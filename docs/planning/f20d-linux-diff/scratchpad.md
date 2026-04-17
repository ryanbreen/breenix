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

Starting Phase 2 capture:

- Added F20d one-shot idle audit rows in `kernel/src/arch_impl/aarch64/context_switch.rs`.
- Added F20d delayed end-of-boot per-CPU counter dump in `kernel/src/main_aarch64.rs`.
- Did not edit `timer_interrupt.rs`, `exception.rs`, `syscall_entry.rs`, or `gic.rs`.
- Aarch64 build command completed cleanly with no warning/error lines.

Phase 2 capture result:

- `logs/breenix-parallels-cpu0/f20d/breenix-boot.log`: copied from `/tmp/breenix-parallels-serial.log` after `./run.sh --parallels --test 45`.
- Explicit `PER_CPU_IDLE_AUDIT` rows: CPU2/3/7 pre_wfi only; CPU0 explicit pre_wfi=0, post_wfi=0.
- Delayed end-of-boot dump: `tick_count=[29,24170,24166,24164,24167,24162,24167,24163]`, `idle_arm_tick=[29,24162,24166,24156,24159,24155,24159,24163]`, `post_wfi_count=[80,59,57,79,69,72,50,69]`.
- Temporary Parallels VM `breenix-1776443643` was stopped and removed; QEMU cleanup reported all QEMU processes killed.
- `cargo fmt --check` failed due pre-existing rustfmt drift/trailing whitespace outside the F20d change set, notably `tests/shared_qemu.rs`; no repo-wide formatting was applied to avoid unrelated churn.
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: clean, no warning/error lines after adding the missing aarch64 cfg guards around AHCI-only trace calls.
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`: clean, no warning/error lines.

Starting Phase 3 divergence table:

- First non-logging divergence identified from captured artifacts: Linux CPU 0 receives `arch_timer` interrupts while idle (134 over 5s), Breenix CPU 0 has `tick_count[0]=29` at idle-arm and end audit (`delta=0`) while CPUs 1-7 advance to ~24.16k ticks.
- Wrote `docs/planning/f20d-linux-diff/divergence-table.md`.
