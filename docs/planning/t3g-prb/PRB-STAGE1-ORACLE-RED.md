# PR-B stage 1 — per-CPU exception-stack ownership probes, red on main

Branch `fix/prb-producer-custody`, based on `main @ c4399234`.

Four test-only probes measure one invariant: **when a stack top is written into a
CPU's per-CPU data, the address must belong to that CPU's own slot in the
per-CPU stack region.** Nothing in the tree checks this today. This stage adds
only the measurement; the repair (ownership sentinels plus a refusing setter) is
a later stage.

Everything is behind the new cargo feature `percpu_stack_custody_oracle`
(implies `boot_tests`). With the feature off the probe module compiles to
nothing and both in-path hooks are empty inline no-ops.

- Probes: `kernel/src/task/percpu_stack_oracle.rs`
- Leg D's scheduler read: `kernel::task::scheduler::zero_tid_idle_probe`
- Gate: `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh`
- Serial for the run below: `docs/planning/t3g-prb/serials/prb-stage1-oracles-red-20260823T094928Z.txt`

## Run

```
./docker/qemu/run-aarch64-percpu-stack-custody-gate.sh
```

QEMU `-M virt,gic-version=3 -cpu max -m 512 -smp 4`, kernel built from
`aarch64-breenix-kernel.json` with `--features boot_tests,percpu_stack_custody_oracle`.
Gate result: **FAILED**, exit 1, first on leg A's `:PASS]` requirement.

## Did the boot run to completion?

**Yes.** `[BOOT_TESTS:PASS]` is present (serial lines 583 and 590), the four
probe reports follow at lines 624-627, and the boot kept heart-beating
(`uptime_ms=187061`) until the gate's cleanup killed QEMU. No `KERNEL PANIC`, no
`DATA_ABORT`, no `INSTRUCTION_ABORT`, no `Unhandled sync exception`, no
`soft lockup detected` anywhere in the serial. There is therefore no first fault
line to record.

## Probe A — a cross-CPU stack top is accepted, and a save frame lands on it

The first recorded run of this probe used an overlay predicate that asked for a
frame taken from user mode (`spsr & 0xF == 0`). That predicate was wrong, not
the observation, and it has since been corrected; both readings are kept below
because the raw fields did not change.

### As first recorded (wrong predicate)

```
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=2:arm_verified=1:stimuli=1:accepted=1:overwritten=33:elr_slot=0xffff0000404f76b0:spsr_slot=0x5:overlay=0:FAIL]
```

`accepted=1` carries the failure: CPU 2 installed CPU 7's exception-stack top
through the ordinary public setters and the write was taken verbatim.
`overwritten=33` is the consequence — 33 of the 34 planted words in
`[F-272, F)` were replaced. 33 is exactly the number of words an AArch64
register-save sequence writes (x0-x29, x30, ELR, SPSR); the 34th word is the
frame's 8-byte tail pad, which the vector never stores to, and it survived
intact. `arm_verified=1` says the borrowed slot really was mapped and idle
before the stimulus, so the overwrite is the stimulus's doing.

`overlay=0` was *not* an exoneration. What actually landed is `spsr_slot=0x5`
(EL1h) with `elr_slot=0xffff0000404f76b0`, a kernel text address — an exception
taken while the CPU was already in EL1 with SP_EL1 pointing at the borrowed
stack. Same class of damage, different entry level.

### Corrected predicate, re-run before any repair

Serial: `docs/planning/t3g-prb/serials/prb-stage1b-overlay-red-20260823T105620Z.txt`

```
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=3:arm_verified=1:stimuli=1:accepted=1:overwritten=33:pad_intact=1:elr_slot=0xffff0000404c2cf8:spsr_slot=0x5:overlay=1:FAIL]
```

`overlay` is now `1` when `overwritten > 0` and the two frame slots read back as
a genuine register-save frame:

- `elr_slot` is a canonical kernel high-half address (`elr_slot >> 48 == 0xffff`);
- `spsr_slot & 0xF` is a well-formed exception-return mode — `SPSR_MODE_EL0T`
  (`0b0000`) or `SPSR_MODE_EL1H` (`0b0101`). Both are legitimate: the vectors
  carve their 272-byte frame off whatever SP is current, so a userspace thread
  whose SP_EL1 is a borrowed stack top and a CPU already in EL1 running on a
  borrowed exception stack both build their frame there. EL1h is the mode the
  filed defect shows;
- `spsr_slot` has no bits set above the 32-bit PSTATE image (NZCV, bits 31:28,
  is the highest-numbered SPSR field), so an arbitrary word cannot be read as
  processor state.

The new `pad_intact` field reports the 34th word — the frame's 8-byte tail pad
at `top - 8`, which the vector's 33 stores never touch. `overwritten == 33 &&
pad_intact == 1` is the save frame's own fingerprint, and this run shows exactly
that.

Probe A's PASS condition is unchanged: `target_cpu != none && arm_verified == 1
&& stimuli > 0 && accepted == 0 && overwritten == 0 && overlay == 0`.
`pad_intact` is reported, not gated on.

### The fingerprint varies with which CPU borrows the slot

Four consecutive runs of the corrected probe:

| stimulus_cpu | overwritten | pad_intact | foreign_occupancy |
|---|---|---|---|
| 0 | 34 | 0 | 5 |
| 1 | 33 | 1 | 2 |
| 0 | 34 | 0 | 3 |
| 3 | 33 | 1 | 2 |

`accepted=1` and `overlay=1` in all four. The clean `33/1` fingerprint appears
when the stimulus lands on a secondary CPU; when CPU 0 borrows the slot it keeps
running on it long enough for an ordinary `stp x29, x30, [sp, #-16]!` prologue to
reach the tail pad as well, giving `34/0`. Probe C's count rises above the
stimulus's own 2 for the same reason: once `kernel_stack_top()` holds the
borrowed address, the dispatch path's `set_user_rsp_scratch(kernel_stack_top())`
and `ensure_user_rsp_scratch_for_el0()` re-install it, so the extra observations
are downstream propagation of the single stimulus, not independent foreign
installs.

## Probe B — the control arm, shipped in the same build

```
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=2:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
```

No failing field. This is the arm that must keep passing after the repair: it
proves a CPU's own slot top and an ordinary heap-backed thread kernel stack are
both still accepted, so a future probe-A pass cannot be produced by a blanket
refusal. `target_image_disturbed=0` confirms probe B itself does not touch
probe A's image (it runs before the stimulus is armed).

## Probe C — the passive occupancy census

```
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=19569:foreign_occupancy=2:max_concurrent=1:worst_slot=7:worst_cpu=2:FAIL]
```

(This is the first recorded run; the corrected-predicate re-run reports the same
`foreign_occupancy=2` with `worst_cpu=3`.)

`foreign_occupancy=2` carries the failure: two stack-top installs named a slot
whose owner was not the installing CPU. Both are probe A's stimulus
(`worst_slot=7`, `worst_cpu=2` matches leg A's `target_cpu=7`,
`stimulus_cpu=2`; the stimulus calls `set_kernel_stack_top` and
`set_user_rsp_scratch`, hence exactly 2). `observations=19569` is the
anti-vacuity half — the census really did watch the dispatch path. Outside the
deliberate stimulus this boot showed no spontaneous foreign occupancy, and
`max_concurrent=1` says no slot was ever named by two CPUs at once.

## Probe D — thread id 0 is a live thread id

```
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=0:zero_resolves=1:FAIL]
```

Both fields fail. `swapper_tid=0` is the `idle_task.id = 0;` overwrite in
`kernel/src/main_aarch64.rs`: CPU 0's idle thread carries thread id 0, which is
also the "no thread" sentinel everywhere else. `zero_resolves=1` is the
consequence at the lookup: resolving tid 0 through the scheduler's own
`registered_idle_cpu` helper succeeds, so a zero tid is indistinguishable from a
real registration. Leg D calls that helper rather than copying it, so when a
later stage simplifies the `(cpu_id == 0 || cpu.idle_thread != 0)` special case
this probe sees the change.

## What the gate additionally demands

The gate requires at least one serial line containing the literal
`[PERCPU_STACK_ALIEN:` — the record the future refusing setter will emit when it
declines a stack top belonging to another CPU. That literal does not exist
anywhere in the tree today (0 occurrences in this serial), so the gate fails on
it as well, deliberately. Leg A is reached first in the gate's check order, so
that is the condition the run reports.

## Deviation from the stage-1 brief

The brief specified probe C's slot attribution as: outside
`[base, base + PERCPU_STACK_REGION_SIZE)` means "not in the region", otherwise
`slot = (value - base) / PERCPU_STACK_STRIDE`. Implemented literally that is
wrong for the addresses actually in play, because per-CPU stack tops are
*exclusive* upper bounds: `percpu_kernel_stack_top(cpu) == base + (cpu + 1) *
PERCPU_STACK_STRIDE`. The literal formula attributes every legitimate own-slot
top to the NEXT slot (CPU 0's own top would read as slot 1), and puts the last
slot's top — which is exactly the address probe A borrows — outside the region
altogether, so probe A's stimulus would not have been counted at all. Probe C
would then have been incapable of ever reporting `foreign_occupancy == 0`,
including after the repair.

`slot_of_stack_top` therefore treats the region as `(base, base + size]` and
attributes by the last addressable byte below the top:
`slot = (value - 1 - base) / PERCPU_STACK_STRIDE`. This maps
`percpu_kernel_stack_top(cpu)` to slot `cpu` and any address strictly inside a
slot to that slot. Legitimate installs observed this boot (CPU 0's boot stack
`percpu_kernel_stack_top(0)`, secondaries' `percpu_kernel_stack_top(cpu)` from
`smp.rs`, the idle fallbacks in `context_switch.rs`) all attribute to their own
slot under this rule, which is why `foreign_occupancy` is exactly the 2 the
stimulus caused.

## No behaviour change in this stage

- `set_kernel_stack_top` / `set_user_rsp_scratch` bodies are unchanged apart
  from an appended call to the census recorder (atomics only, no locks, no
  formatting — it runs on the dispatch path).
- The userspace-dispatch site in `context_switch.rs` gains one appended call
  after the existing `set_user_rsp_scratch(kernel_stack_top())` statement; its
  condition is untouched.
- `idle_task.id`, the `(cpu_id == 0 || cpu.idle_thread != 0)` expression and
  `reclaim_terminated_threads` are untouched.
- Ordinary profiles verified clean after the change: `--features boot_tests` and
  `--features testing,external_test_bins` both build for
  `aarch64-breenix-kernel.json` with 0 warnings, and
  `docker/qemu/run-aarch64-boot-test-native.sh` passes on the first attempt.

One thing worth recording, because it was tripped over on the way here:
`run-aarch64-boot-test-native.sh` boots whatever kernel is sitting at
`target/aarch64-breenix-kernel/release/kernel-aarch64` and requires
`boot_tests`-only markers (`[BLOCK_EINTR_ORACLE:`, `[INIT_GROUP_REFUSAL_ORACLE:`),
so it must be run against a `boot_tests` build. Run against a
`testing,external_test_bins` kernel it panics at
`kernel/src/task/softirq_tests.rs:228:5` ("ksoftirqd should have processed
deferred softirqs"). That panic is **pre-existing on `main @ c4399234`**: an
unmodified main checkout built with the same profile and booted with the same
QEMU command panics identically at the same line. It is unrelated to this
change, which compiles to nothing in that profile.
