# F20a Plan

## Milestone 1: Trace Idle Path

Validation command:
`rg -n "secondary_cpu_entry_rust|idle_loop_arm64|wfi|register_cpu_idle_thread|isr_unblock_for_io" kernel/src/arch_impl/aarch64 kernel/src/task`

Expected outcome: know whether secondary CPUs reach a WFI-capable idle loop or hot-spin elsewhere.

## Milestone 2: Minimal Fix

Validation command:
`cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | tee logs/f20a/build.log; ! rg "^(warning|error)" logs/f20a/build.log`

Expected outcome: clean aarch64 build with no warnings or errors.

## Milestone 3: Parallels Sweep

Validation command:
`./run.sh --parallels --test 45`

Expected outcome: five runs pass the F20a boot-script, timer-breadcrumb, and host-CPU criteria.

## Milestone 4: Ship

Validation command:
`git diff --name-only main...HEAD`

Expected outcome: prohibited files untouched, no polling fallback added, docs complete, PR merged if validation passes.
