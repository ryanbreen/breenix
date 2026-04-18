# F29 Exit - Re-enable SMP on Parallels

## Outcome

PASS. Re-enabling ARM64 PSCI secondary CPU bring-up on Parallels no longer
reproduces the F21 pre-userspace fault. A clean validation boot reached:

```text
[smp] 8 CPUs online
[init] Boot script completed
[init] bounce started (PID 5)
```

The bounce/bwm compositor continued rendering through the 120 second validation
window, and the conservative following-tick frame estimate was 160.87 Hz.

## What Changed

- `kernel/src/platform_config.rs`
  - Added `is_parallels()` for the UEFI/ACPI ARM64 platform with discovered
    GICv3 redistributors and zero VMware RAM offset.
- `kernel/src/main_aarch64.rs`
  - Included Parallels in the ARM64 PSCI secondary CPU bring-up gate.
  - Removed the single-CPU Parallels skip path.

## Original Ask

F29 needed to test whether post-F21 changes made Parallels SMP safe again:
remove the QEMU/VMware-only PSCI gate, build cleanly, boot Parallels with 8
vCPUs, verify userspace and bounce rendering, and either merge the passing
enablement or honestly document a fault if the F21 issue remained.

## Phase Results

### Phase 1 - Gate Removal And Build

Implemented.

Validation:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
grep -E '^(warning|error)(\[|:)' /tmp/f29-aarch64-build.log
```

Result: build passed, and the warning/error grep produced no output.

### Phase 2 - Parallels Boot, Render, And FPS

Implemented.

Validation was run from a detached clean worktree at pre-trailer-rewrite commit
`93e430b6`, whose source diff is identical to final code commit `66ecc316`.
Because other factory VMs were concurrently using
`/tmp/breenix-parallels-serial.log`, the validation copy of `run.sh` was
temporarily pointed at F29-specific host paths and VM prefix:

```text
SERIAL_LOG=/tmp/f29-smp-parallels-serial.log
SCREENSHOT=/tmp/f29-smp-screenshot.png
VM=f29smp-1776510041
```

Command:

```bash
./run.sh --parallels --test 120 --no-build
```

Serial evidence:

```text
[smp] CPU 0 MPIDR=0x80000000, stack_base=0x43000000
[smp] GICR covers 8 redistributors, probing CPUs 1..8
[smp] Probing secondary CPUs via PSCI...
[smp] 8 CPUs online
[init] Boot script completed
[init] bsshd started (PID 4)
[init] bounce started (PID 5)
[bounce] Window mode: id=1 400x300 [boot_id=0000000071e87cf6]
```

Fault-marker grep:

```bash
rg -n "SOFT_LOCKUP|SOFT LOCKUP|DATA_ABORT|UNHANDLED_EC|TIMEOUT|panic|PANIC|VCPU|Exception" \
  /tmp/f29-smp-parallels-serial.log
```

Result: no output.

Render verdicts:

```text
scripts/f23-render-verdict.sh /tmp/f29-smp-screenshot.png
VERDICT=PASS

scripts/f24-render-verdict.sh /tmp/f29-smp-screenshot.png
VERDICT=PASS
```

FPS estimate:

```text
Frame #500 near ticks=5000
Frame #19000 near ticks=120000
(19000 - 500) / ((120000 - 5000) / 1000) = 160.87 Hz
```

### Phase 3 - Failure Classification

Not needed. The F21 fault signature did not reproduce.

### Phase 4 - Minimal Fix

Not needed. No new Parallels SMP fault was observed.

## Known Risks And Gaps

- The PSCI success log lines are interleaved with secondary CPU raw UART output,
  so not every CPU's success line is cleanly visible. The final kernel marker
  still reports `[smp] 8 CPUs online`.
- The final validation used `--no-build` after the clean F29 build to avoid
  repeating the full artifact build. The built kernel binary at
  `/tmp/breenix-f29-validate/target/aarch64-breenix/release/kernel-aarch64`
  did not contain the old single-CPU skip string.
- Logs and screenshots are local validation artifacts and are not committed.

## PR

https://github.com/ryanbreen/breenix/pull/321
