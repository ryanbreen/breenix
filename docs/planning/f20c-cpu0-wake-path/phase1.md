# F20c Phase 1: CPU 0 WFI Wake-Path Diagnostic

## Diagnostic

Added a one-shot `[END_OF_BOOT_AUDIT]` dump emitted by diagnostic kernel threads
after roughly 40 seconds measured with `CNTVCT_EL0`. The dump does not depend on
CPU 0's global wall-clock tick and does not modify timer, exception, syscall, or
GIC hot paths.

The idle loop also records per-CPU atomic baselines immediately before WFI:

- `idle_arm_tick`: that CPU's `TIMER_TICK_COUNT[cpu]` at WFI entry
- `idle_arm_tsc`: `CNTVCT_EL0` at WFI entry
- `post_wfi_count`: number of times execution resumed at the instruction after WFI
- `post_wfi_tick`: that CPU's `TIMER_TICK_COUNT[cpu]` after post-WFI resume

Interpretation:

- H1 is confirmed if CPU 0's final `tick_count[0]` is greater than
  `idle_arm_tick[0]` while CPU 0 does not advance `post_wfi_count[0]`.
- H2 is confirmed if CPU 0's final `tick_count[0]` is equal to
  `idle_arm_tick[0]` while other CPUs continue advancing.

## Build

Clean:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
result: clean, no warning/error lines

cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
result: clean, no warning/error lines
```

## Boot Output

Command:

```text
./run.sh --parallels --test 45
```

Source log:

```text
.factory-runs/f20c-20260417-115821/phase1-serial-rerun.log
```

The normal `--test 45` boot path did not put CPU 0 through
`idle_loop_arm64` before CPU 0's timer died, so the CPU 0 idle-entry baseline
remained zero:

```text
[ahci]   tick_count=[9,8215,8214,8216,8213,8215,8215,8244]
[ahci]   idle_count=[0,774,763,757,760,752,778,753]
[ahci]   cpu0_timer: cval=56762305 cntvct_at_arm=56738306 cntvct_now=302180869 delta=245418564 (10225ms)
[ahci]   cpu0_idle: iters=0 daif=0xdead pmr=0xdead igrpen1=0xdead cntv_ctl=0xdead
```

The end-of-boot audit, emitted later from CPU 6, showed CPU 0 still frozen at
9 ticks while all other CPUs continued to around 32k ticks:

```text
[END_OF_BOOT_AUDIT] cpu=6 cntvct=1016590360 global_ticks=9 cpus_online=8
[END_OF_BOOT_AUDIT] tick_count=[9,32107,32107,32109,32112,32117,32107,32138]
[END_OF_BOOT_AUDIT] idle_arm_tick=[0,32090,32107,32101,32107,32113,32101,32133]
[END_OF_BOOT_AUDIT] post_wfi_count=[0,293,300,342,301,263,291,262]
[END_OF_BOOT_AUDIT] idle_count=[0,3104,3178,3196,3165,3121,3137,3105]
[END_OF_BOOT_AUDIT] hw_tick_count=[9,32109,32109,32111,32114,32119,32109,32140]
[END_OF_BOOT_AUDIT] sw_to_hw_map=[0,1,2,3,4,5,6,7]
[END_OF_BOOT_AUDIT] timer_ctl=[0x1,0x1,0x1,0x1,0x1,0x1,0x1,0x1]
```

## Verdict

H2 confirmed for the observable main-branch `--test 45` path: CPU 0's PPI27
virtual timer interrupt is not firing after early boot. CPU 0 stayed at
`tick_count[0]=9` from the AHCI timeout through the 40-second audit while other
CPUs advanced from roughly 8.2k to 32.1k ticks.

This run does not reproduce F20b's exact CPU0 `pre_wfi` row because CPU 0 did
not enter `idle_loop_arm64` before the timer froze on `main`; however that makes
H1 less plausible for this branch. A skipped post-WFI dump cannot explain a CPU0
tick counter that remains unchanged for the rest of the boot while PPI27 is
enabled and pending in the AHCI dump.
