# Turn 6: Five-boot gate and strict polling split

**Status: INCONCLUSIVE.**

The Turn 5 IRQ-tail fix continues to look healthy: every serial log reached the
userspace syscall marker, no boot showed AHCI timeout or panic markers, and all
valid GDB endpoint captures showed CPU0 AHCI interrupts and healthy CPU0 timer
ticks. The strict zero-polling proof failed, though:

- boot 1: `ahci_polled_post_registration_count=69`
- boot 4: `ahci_polled_post_registration_count=70`
- boot 5: `ahci_polled_post_registration_count=70`

Boots 2 and 3 reached userspace in serial, but Parallels `guest-debugger` did
not open before the VM was gone, so their GDB endpoint values are unavailable.
This does not affect the conclusion: the valid endpoint boots already fail the
strict post-registration polling criterion.

## A. Counter split diff

Source commit:

- `97fdfd2f feat(ahci): split polling counter pre/post IRQ registration`

The existing total counter was widened to `AtomicU64`, and a second exported
counter was added:

```rust
#[export_name = "ahci_polled_completion_count"]
pub static AHCI_POLLED_COMPLETION_COUNT: AtomicU64 = AtomicU64::new(0);

#[export_name = "ahci_polled_post_registration_count"]
pub static AHCI_POLLED_POST_REGISTRATION_COUNT: AtomicU64 = AtomicU64::new(0);
```

The polling branch now accounts by IRQ-registration state:

```rust
if AHCI_IRQ.load(Ordering::Relaxed) == 0 {
    AHCI_POLLED_COMPLETION_COUNT.fetch_add(1, Ordering::Relaxed);
} else {
    AHCI_POLLED_POST_REGISTRATION_COUNT.fetch_add(1, Ordering::Relaxed);
    AHCI_POLLED_COMPLETION_COUNT.fetch_add(1, Ordering::Relaxed);
}
```

No behavior was changed. This turn only added memory-only instrumentation.

## B. Build result

Command:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Result: clean. Exit 0, zero compiler warnings.

`git diff --check` was clean before the source commit.

## C. Five-boot aggregate result

Harness:

- `turn6-artifacts/run_5boot_gate.sh`
- Fresh `./run.sh --parallels` VM per boot
- 90-second wait window per boot
- Per-boot serial copied to `turn6-artifacts/boot-N/serial.log`
- Per-boot GDB state copied to `turn6-artifacts/boot-N/gdb-state.log`
- Cleanup after every boot; final `prlctl list --all` showed only `linux-probe`
- Required QEMU cleanup reported `All QEMU processes killed`

Aggregate:

```text
boot-1: fail reason=post_poll=69
boot-2: fail reason=ahci_irq=missing;ahci_isr_count=missing;ahci_cpu=missing;post_poll=missing;gdb_rc=guestdebugger-port-timeout;cpu0_pct=0.00
boot-3: fail reason=ahci_irq=missing;ahci_isr_count=missing;ahci_cpu=missing;post_poll=missing;gdb_rc=guestdebugger-port-timeout;cpu0_pct=0.00
boot-4: fail reason=post_poll=70
boot-5: fail reason=post_poll=70
overall: fail
ahci_isr_count: min=0, max=1124, mean=674.20
cpu0_pct_of_max: min=0.00, max=100.00, mean=59.97
ahci_polled_post_registration_count: max across all boots = 70
```

The aggregate min/mean include boots 2 and 3 as zero because their GDB captures
timed out. Looking only at valid endpoint captures:

| Boot | AHCI ISR count | CPU0 / peer max | Total polls | Post-registration polls |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1123 | 99.86% | 71 | 69 |
| 4 | 1124 | 100.00% | 72 | 70 |
| 5 | 1124 | 99.99% | 72 | 70 |

## D. Strict zero-polling proof

Strict proof failed.

The new post-registration counter is nonzero in every valid GDB capture:

```text
boot-1 ahci_polled_post_registration_count=69
boot-4 ahci_polled_post_registration_count=70
boot-5 ahci_polled_post_registration_count=70
```

This means the Turn 5 caveat was real: most of the remaining polling occurs
after `AHCI_IRQ` is registered. The polling branch is still the
`!scheduler_running` path in `wait_cmd_slot0()`, so the likely window is
post-IRQ-registration but pre-scheduler. That is not acceptable under the
strict Turn 6 bar as written.

## E. Headline health numbers

The IRQ-tail fix remains healthy in the valid endpoint captures:

- AHCI IRQ is always `34`.
- AHCI ISR always runs on CPU0 (`ahci_isr_last_mpidr_aff0=0`).
- AHCI ISR counts are stable: `1123`, `1124`, `1124`.
- CPU0 timer is at full pace: `99.86%`, `100.00%`, `99.99%` of peer max.
- Serial logs for all five boots reached `[ OK ] syscall path verified`.
- Serial logs for all five boots had zero AHCI timeout markers.
- Serial logs for all five boots had zero panic, data abort, synchronous
  exception, or CPU0 regression alarm markers.

Boots 2 and 3 are harness/GDB capture failures, not serial health failures:
their serial logs reached userspace and had no timeout/panic markers.

## F. Status and Turn 7 scope

**Status: INCONCLUSIVE.**

Turn 6 proves the Turn 5 fix is stable enough to keep CPU0 healthy, but it does
not prove strict zero polling after IRQ registration.

Named Turn 7 scope:

1. Trace the post-registration polling commands in `wait_cmd_slot0()`.
2. Split the post-registration counter by scheduler state or call phase:
   `post_registration_pre_scheduler` vs `post_registration_scheduler_running`.
3. If the nonzero count is entirely post-registration/pre-scheduler, decide
   whether to either:
   - move AHCI IRQ registration later so early probe/setup commands remain
     classified pre-registration; or
   - make the pre-scheduler path wait on ISR-completed command state instead
     of polling `PORT_CI`.
4. If any polling occurs after scheduler start, investigate that exact command
   path as a real interrupt-path regression.

Do not weaken the test. The next proof needs to explain or remove the
`69..70` post-registration polls.

Artifacts:

- `turn6-artifacts/run_5boot_gate.sh`
- `turn6-artifacts/aggregate-result.txt`
- `turn6-artifacts/metrics.tsv`
- `turn6-artifacts/boot-1/`
- `turn6-artifacts/boot-2/`
- `turn6-artifacts/boot-3/`
- `turn6-artifacts/boot-4/`
- `turn6-artifacts/boot-5/`

