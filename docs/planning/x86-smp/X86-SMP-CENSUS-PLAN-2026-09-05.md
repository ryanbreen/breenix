<!-- Header added when this document was committed with #814 PR-1; the body
     below it is the read-only census pass as it was written, unedited. -->

# Provenance and drift, added at commit time

This document was written as a read-only census of `origin/main` at
`9b3dd4af9dc53d2950688f8094a26351703892cd`, and it is committed here unedited
below this header. Two things a reader needs before trusting its citations:

1. **The head it censused is not the head it landed on.** PR-1 branches from
   `origin/main` at `39169922` — 163 commits later. Source *facts* below were
   re-checked at that base (see the drift table); of the source *line numbers*,
   16 anchor rows were re-derived and 9 of them had moved, while the rest
   were left as citations against the censused head.
2. **PR-1 itself changes what §1.6, §2 and §4 describe.** What it changed, and
   the atlas wording that follows from it, are in the addendum at the end of
   this file (§8), not by editing the body.

## Drift table: anchors re-derived at `39169922`

| the body says | at `39169922` |
|---|---|
| `kernel/src/task/scheduler.rs:1064-1067` — `MAX_CPUS` split | `:1348-1351`, same two-arm shape, same values |
| `kernel/src/task/scheduler.rs:1519-1527` — `online_cpu_count()` | `:1803` |
| `kernel/src/task/scheduler.rs:1504-1513` — `current_cpu_id()` | `:1788` |
| `kernel/src/task/scheduler.rs:5357-5369` — `current_cpu_id_raw()` | `:6230` |
| `kernel/src/test_framework/registry.rs:4118-4126` — census-widen SKIP | `:4170` |
| `docker/qemu/run-x86-boot-tests.sh:415` — `-smp 1` | `:462` |
| `docker/qemu/run-x86-prod-profile-boot-test.sh:1030` — `-smp 1` | `:1053` |
| `docker/qemu/run-x86-tty-oracle-gate.sh:239` — `-smp 1` | `:256` |
| `run.sh:1066` / `:1104` — the `-smp 4` launchers | `:1086` / `:1124` |
| `kernel/src/arch_impl/aarch64/smp.rs:167`, `:170` — `CPUS_ONLINE`, `CPU_ONLINE` | unchanged |
| `kernel/src/arch_impl/x86_64/cpuinfo.rs:229`, `:298` — the two APIC flag strings | unchanged |
| `kernel/src/interrupts.rs:20` — `PICS` | unchanged |
| `kernel/src/per_cpu.rs:356` — `CPU0_DATA` | unchanged |
| `kernel/src/gdt.rs:12-13` — the `TSS`/`GDT` `OnceCell`s | unchanged |
| `docker/qemu/run-boot-parallel.sh:108`, `:130` — `-smp 1` | unchanged |
| `scripts/run-interactive-native.sh:73` — `-smp 2` | unchanged |

Anchors not in that table were not re-derived at `39169922`; read them as
citations against `9b3dd4af`.

---

# x86_64 SMP: read-only census and staged plan

Head censused: `origin/main` at `9b3dd4af9dc53d2950688f8094a26351703892cd`
("Merge pull request #811 from ryanbreen/task/562-slice3c-cpupin-scaffolding").

```
$ git fetch origin && git rev-parse origin/main
9b3dd4af9dc53d2950688f8094a26351703892cd
```

This pass read source only: no build, no boot, and no gate run backs it.
Runtime statements below are marked UNMEASURED where they are inferences from
source rather than readings from a boot.

---

## 1. Census: what x86 has today

### 1.1 The arch directory

```
$ ls kernel/src/arch_impl/x86_64/
constants.rs  cpu.rs  cpuinfo.rs  interrupt_frame.rs  mod.rs
paging.rs     percpu.rs  pic.rs   privilege.rs        timer.rs

$ ls kernel/src/arch_impl/aarch64/ | wc -l
22
```

Ten x86 files against twenty-two aarch64 files. There is no `smp.rs`, no
`gic.rs` analogue, no `boot.S`, and no secondary-entry assembly on the x86 side.

```
$ grep -rn "smp\|SMP\|APIC\|apic\|MADT\|madt\|acpi\|ACPI\|IPI\|ipi" kernel/src/arch_impl/x86_64/
kernel/src/arch_impl/x86_64/percpu.rs:588:    /// on this same CPU. (An SMP reader of another CPU's mark would need a
kernel/src/arch_impl/x86_64/cpuinfo.rs:229:            flags.push("apic");
kernel/src/arch_impl/x86_64/cpuinfo.rs:298:            flags.push("x2apic");
```

Three hits, and 3 of 3 are prose or a CPUID feature-flag string. The two
`cpuinfo.rs` sites push `"apic"` / `"x2apic"` into the `/proc/cpuinfo` flag list
from CPUID; they program no APIC register.

Repo-wide, the same picture:

```
$ grep -rn "SIPI\|0xFEE0\|0xfee0\|LAPIC\|lapic" kernel/src
(no output)

$ grep -rn "shootdown" kernel/src
kernel/src/memory/kernel_page_table.rs:536:        // - TLB: Local invlpg on add, no remote shootdown needed
```

### 1.2 Per-CPU

The access mechanism is genuinely per-CPU-capable: `X86PerCpu::cpu_id()`
(`kernel/src/arch_impl/x86_64/percpu.rs:25-35`) is a
`mov {}, gs:[PERCPU_CPU_ID_OFFSET]`, and the whole `PerCpuOps` surface is
GS-relative.

The *storage* is not. `kernel/src/per_cpu.rs:354-356`:

```rust
/// Static per-CPU data for CPU 0 (BSP)
/// In a real SMP kernel, we'd have an array of these
static mut CPU0_DATA: PerCpuData = PerCpuData::new(0);
```

`per_cpu::init()` (`kernel/src/per_cpu.rs:371`) writes `GS_BASE` and
`KERNEL_GS_BASE` to `&CPU0_DATA` and then panics unless the GS read-back yields
CPU id 0 (`kernel/src/per_cpu.rs:398-403`). There is one instance and one
initializer, and the initializer takes no CPU-id parameter.

Contrast `kernel/src/per_cpu_aarch64.rs:161-163`:

```rust
/// Per-CPU data for all CPUs (up to MAX_CPUS).
static mut ALL_CPU_DATA: [PerCpuData; crate::arch_impl::aarch64::constants::MAX_CPUS] = [
```

with `init_cpu(cpu_id)` at `kernel/src/per_cpu_aarch64.rs:422-425`, called from
`secondary_cpu_entry_rust` (`kernel/src/arch_impl/aarch64/smp.rs`).

### 1.3 GDT / TSS / IDT

Single global instances, one load, one initializer:

- `kernel/src/gdt.rs:12-14` — `static TSS: OnceCell<TaskStateSegment>`,
  `static GDT: OnceCell<(GlobalDescriptorTable, Selectors)>`,
  `static TSS_PTR: AtomicPtr<TaskStateSegment>`.
- `kernel/src/gdt.rs:29` `pub fn init()` builds both and calls `gdt.load()` at
  `kernel/src/gdt.rs:101`. It takes no CPU-id parameter.
- `kernel/src/interrupts.rs:63` `static IDT: Once<InterruptDescriptorTable>`;
  `init_idt()` at `:72` fills it and `idt.load()` at `:225`.

The IDT is legitimately shareable across CPUs (each CPU executes its own `lidt`
against the same table). The TSS is not: RSP0 and the IST entries are per-CPU,
and `per_cpu::update_tss_rsp0(...)` is called on the dispatch path
(`kernel/src/interrupts/context_switch.rs:820`, `:1045`, `:1158`, `:1496`,
`:1633`) against the single `TSS_PTR`.

### 1.4 Timer

PIT, not LAPIC.

- `kernel/src/time/timer.rs:12-18` — `PIT_INPUT_FREQ_HZ = 1_193_182`,
  `PIT_HZ = 200` (the comment reads `200 Hz => 5 ms per tick`), command port
  `0x43`, channel-0 port `0x40`.
- `kernel/src/time/timer.rs:33-46` `pub fn init()` writes mode 3 and the divisor.
- `kernel/src/time/mod.rs:25-31` — `tsc::calibrate()` first (PIT channel 2), then
  `timer::init()` (channel 0).
- Boot order in `kernel/src/main.rs`: `interrupts::init_pic()` at `:377`, then
  `time::init()` at `:383`.

The tick handler is `kernel/src/interrupts/timer.rs:41`
`timer_interrupt_handler`. Its quantum counter is a single global, not a
per-CPU field (`kernel/src/interrupts/timer.rs:35-36`):

```rust
/// Current thread's remaining time quantum
static mut CURRENT_QUANTUM: u32 = TIME_QUANTUM;
```

That file carries the Tier-1 prohibited-modifications banner at lines 1-8.

EOI is PIC EOI, sent from `send_timer_eoi()`
(`kernel/src/interrupts/timer.rs:130`), called from
`kernel/src/interrupts/timer_entry.asm:327` and `:364`. The fallback arm writes
`0x20` to ports `0x20` / `0xA0` directly.

### 1.5 Interrupt controller

8259A PIC only.

- `kernel/src/interrupts.rs:20-21` — `pub static PICS: spin::Mutex<ChainedPics>`,
  offsets 32 and 40 (`:17-18`).
- `kernel/src/interrupts.rs:30-38` — the vectors the kernel uses:
  `Timer = 32`, `Keyboard = 33`, `Serial = 36` (IRQ4/COM1), `Irq10 = 42`,
  `Irq11 = 43`.
- `kernel/src/interrupts.rs:229-241` `init_pic()` calls
  `PICS.lock().initialize()` and clears mask bits 0, 1 and 4 on PIC1 data port
  `0x21`.
- `kernel/src/arch_impl/x86_64/pic.rs` (52 lines) is the `InterruptController`
  HAL impl and delegates to the same `PICS` mutex.

There is no IOAPIC code, no interrupt-source-override handling, and no MADT
parse.

### 1.6 ACPI / MADT

The pinned bootloader already hands the RSDP to the kernel:

```
$ grep -n "rsdp_addr" ~/.cargo/git/checkouts/bootloader-73cd38fff6654e68/707db11/api/src/info.rs
52:    pub rsdp_addr: Optional<u64>,
```

(`kernel/Cargo.toml:141` pins that rev.) The kernel does not read it:

```
$ grep -rn "rsdp" kernel/src
kernel/src/platform_config.rs:553:    pub rsdp_addr: u64,
```

1 of 1 hit, and it is a field of the aarch64-only Parallels `HardwareConfig`
struct (`kernel/src/platform_config.rs:530-559`, inside a
`#[cfg(target_arch = "aarch64")]` block). `kernel_main`'s reads of `boot_info`
are the framebuffer (`kernel/src/main.rs:142`), `physical_memory_offset`
(`:177`) and `memory_regions` (`:187`).

### 1.7 IPIs and TLB shootdown

x86 has neither.

- `kernel/src/memory/tlb.rs` exposes `flush_page` (`:21`), `flush_all` (`:31`)
  and `flush_after_page_table_switch` (`:42`) — local `invlpg` / CR3 reload.
- `kernel/src/memory/kernel_page_table.rs:536` states the current model in a
  comment: `// - TLB: Local invlpg on add, no remote shootdown needed`.
- The reschedule IPI is aarch64-only. `Scheduler::send_resched_ipi`
  (`kernel/src/task/scheduler.rs:3068-3090`) and `send_resched_ipi_to_cpu`
  (`:3093-3116`) are both `#[cfg(target_arch = "aarch64")]`, and the x86 arms are
  discards: `kernel/src/task/scheduler.rs:3593-3594` and `:3612-3613` read
  `#[cfg(not(target_arch = "aarch64"))] let _ = wake.resched_target();`.
  `send_exit_expedite_sgi` (`:3118`) ends the same way at `:3155-3156`.

### 1.8 How the scheduler and dispatch paths assume one CPU on x86

`kernel/src/task/scheduler.rs:1064-1067`:

```rust
#[cfg(target_arch = "aarch64")]
pub(crate) const MAX_CPUS: usize = 8;
#[cfg(not(target_arch = "aarch64"))]
pub(crate) const MAX_CPUS: usize = 1;
```

That split dates to `25afe0e1` ("feat: add ARM64 SMP support — boot and schedule
across 4 CPUs", Sat Feb 7 2026); `d7d08595` only widened the visibility to
`pub(crate)`.

```
$ git log --format='%h %ad %s' -L 1064,1068:kernel/src/task/scheduler.rs
d7d08595 Thu Aug 13 00:38:04 2026 -0400 fix(x86): deferred-retirement custody for process page-table frames (#470 PR-3)
25afe0e1 Sat Feb  7 06:36:59 2026 -0500 feat: add ARM64 SMP support — boot and schedule across 4 CPUs
```

Because `MAX_CPUS == 1` on x86, `Scheduler::per_cpu_queues: [VecDeque<u64>; MAX_CPUS]`
and `Scheduler::cpu_state: [CpuSchedulerState; MAX_CPUS]` are one-element
arrays.

Four more hardcodes:

- `Scheduler::current_cpu_id()` (`kernel/src/task/scheduler.rs:1504-1513`)
  returns `Aarch64PerCpu::cpu_id()` on aarch64 and the literal `0` on everything
  else, even though `X86PerCpu::cpu_id()` exists and reads GS
  (`kernel/src/arch_impl/x86_64/percpu.rs:25`).
- `current_cpu_id_raw()` (`kernel/src/task/scheduler.rs:5357-5369`) does the same
  with MPIDR_EL1 vs `0`.
- `is_cpu_idle_raw()` (`kernel/src/task/scheduler.rs:5375-5384`) returns `false`
  on x86 unconditionally.
- `arch_can_dispatch_here()` (`kernel/src/task/scheduler.rs:316-324`) is
  `can_dispatch_here()` on aarch64 and `true` on x86.

Emergency / IST stacks are sized for one CPU:

```
$ sed -n 150,155p kernel/src/memory/mod.rs
    // For now, assume single CPU. In SMP systems, this would be the actual CPU count
    let _emergency_stacks =
        per_cpu_stack::init_per_cpu_stacks(1).expect("Failed to initialize per-CPU stacks");
```

and the two consumers hardcode the index
(`kernel/src/memory/per_cpu_stack.rs:93-94` and `:104-105`):

```rust
    // TODO: Get actual CPU ID from APIC
    let cpu_id = 0; // For now, assume CPU 0
```

The master kernel PML4 read on the dispatch path is already lock-free
(`kernel/src/memory/kernel_page_table.rs:53`
`static MASTER_KERNEL_PML4_PHYS: AtomicU64`, accessor `master_kernel_pml4()`),
which is the #791 repair. Its doc comment
(`kernel/src/memory/kernel_page_table.rs:34-42`) argues from a single-CPU
premise: "Behind a spin lock those two readers deadlock a single-CPU machine".
The atomic cell stays correct under SMP; the sentence's reasoning becomes
narrower than the situation.

The x86 dispatch model itself differs from aarch64's. `schedule()` on x86
(`kernel/src/task/scheduler.rs:4676`) returns `Option<(u64, u64)>` for the
interrupt-return path to act on; on aarch64 (`:4640`) it calls
`context_switch::schedule_from_kernel()` inline. The x86 idle loop is
`kernel/src/interrupts/context_switch.rs:1813 idle_loop()`, a single
`enable_and_hlt()` loop plus housekeeping — one instance, entered by whichever
CPU runs it.

### 1.9 What `-smp 2` does today

The kernel contains no INIT/SIPI sender, no AP entry symbol, and no LAPIC ICR
write (the §1.1 greps). The bootloader is the `rust-osdev/bootloader` UEFI stage
(`Cargo.toml:73`), whose handoff hands over one execution context.

So the mechanism is: application processors stay wherever the UEFI firmware left
them, and the kernel does not address them. Marked UNMEASURED — no boot was run
for this document.

Two interactive launchers already pass more than one vCPU today, which is
consistent with that reading:

```
$ grep -rn '\-smp' run.sh scripts/run-interactive-native.sh
run.sh:1066:        -M virt,gic-version=3 -cpu max -smp 4 \
run.sh:1104:        -smp 4 \
scripts/run-interactive-native.sh:73:    -smp 2
```

`run.sh:1104` is inside the `qemu-system-x86_64` invocation beginning at
`run.sh:1094`; `scripts/run-interactive-native.sh:62-84` is likewise
`qemu-system-x86_64`, with `-accel tcg,thread=multi,tb-size=512` at line 65.

The gates, by contrast, pin one CPU:

```
$ grep -rn '\-smp 1' docker/qemu/run-x86-boot-tests.sh docker/qemu/run-x86-prod-profile-boot-test.sh docker/qemu/run-boot-parallel.sh docker/qemu/run-x86-tty-oracle-gate.sh
docker/qemu/run-x86-boot-tests.sh:415:        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
docker/qemu/run-x86-prod-profile-boot-test.sh:1030:    -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
docker/qemu/run-boot-parallel.sh:108:            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
docker/qemu/run-boot-parallel.sh:130:                -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
docker/qemu/run-x86-tty-oracle-gate.sh:239:        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
```

### 1.10 What #629 says, and what the tree says

`gh issue view 629` (state OPEN) quotes `online_cpu_count()` and reads:

> aarch64 asks SMP how many CPUs actually came up; x86 asserts eight. On a
> `-smp 1` boot ... CPUs 1..7 therefore look **online** ...

On this head, `MAX_CPUS` is 1 on x86 (`kernel/src/task/scheduler.rs:1067`, split
introduced by `25afe0e1` in February, i.e. before the August filing), so
`online_cpu_count()` returns 1 and the seven-phantom-CPU consequence the issue
body describes is not the current shape. What the body's "Suggested first move"
asks for is still outstanding, and is what PR-1 below does:

> expose the same `cpus_online()` shape it exposes on aarch64 and clamp to
> `MAX_CPUS`.

The current x86 answer is a compile-time constant that happens to match
`-smp 1` and would misreport the moment a second CPU exists. #629 remains open,
and the honest restatement is: *x86 reports its CPU count from a constant rather
than from an enumeration*.

The residue the issue left behind is still live in the tree.
`kernel/src/test_framework/registry.rs:4118-4126` emits

```
[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:baseline_reported={}:axes={}:SKIP]
```

carrying this comment verbatim:

```
// x86 computes both disarmed census passes, but it does not
// prove the widening: aarch64's real-thread arm does. SKIP discloses that
// platform limitation; it is deliberately not a passing result.
```

The gate pins that marker string literally at
`docker/qemu/run-x86-boot-tests.sh:322`, and
`tests/strand_handoff_structure.rs:2082` and `:2165` pin both halves.

---

## 2. aarch64 to x86 parity map

| Capability | aarch64 (present) | x86_64 (today) |
|---|---|---|
| CPU enumeration | GICR-region probe + PSCI probe loop, `kernel/src/main_aarch64.rs:997-1034`; `smp::MAX_CPUS = 8` (`kernel/src/arch_impl/aarch64/smp.rs:16`) | absent; `MAX_CPUS` is a constant 1 (`kernel/src/task/scheduler.rs:1067`) |
| Secondary start | PSCI `CPU_ON` HVC, `smp::release_cpu()` (`kernel/src/arch_impl/aarch64/smp.rs:381`), entry `secondary_cpu_entry` (`kernel/src/arch_impl/aarch64/boot.S:879`) | absent; no trampoline, no INIT-SIPI-SIPI |
| Online count | `smp::cpus_online()` backed by `CPUS_ONLINE: AtomicU64` (`kernel/src/arch_impl/aarch64/smp.rs:167`) + `CPU_ONLINE[MAX_CPUS]` (`:170`) | `online_cpu_count()` returns `MAX_CPUS` (`kernel/src/task/scheduler.rs:1519-1527`) |
| Bring-up observability | staged `CPU_BRINGUP_STAGE[]` + `bringup_stage_name()` (`kernel/src/arch_impl/aarch64/smp.rs:231-272`), PSCI code retention (`:32`) | absent |
| Per-CPU storage | `ALL_CPU_DATA: [PerCpuData; MAX_CPUS]` + `init_cpu(cpu_id)` (`kernel/src/per_cpu_aarch64.rs:163`, `:422`) | one `CPU0_DATA` + parameterless `init()` (`kernel/src/per_cpu.rs:356`, `:371`) |
| Per-CPU descriptor tables | banked system registers; no per-CPU copy required | one `GDT`/`TSS` `OnceCell` (`kernel/src/gdt.rs:12-13`) — a per-CPU TSS is required for RSP0/IST |
| Per-CPU stacks | `percpu_kernel_stack_top(cpu_id)` (`kernel/src/arch_impl/aarch64/constants.rs`), base stored into the `.bss.boot` variables (`kernel/src/main_aarch64.rs:973`) | `init_per_cpu_stacks(1)` (`kernel/src/memory/mod.rs:154`) with `let cpu_id = 0` at both consumers (`kernel/src/memory/per_cpu_stack.rs:94`, `:105`) |
| Interrupt controller | GICv2/v3 with a per-CPU interface, `gic::init_cpu_interface_secondary()` (`kernel/src/arch_impl/aarch64/gic.rs:460`), redistributor map (`:1445`) | 8259A PIC, one instance (`kernel/src/interrupts.rs:20`) |
| Per-CPU timer | virtual timer PPI, `timer_interrupt::init_secondary()` called from `secondary_cpu_entry_rust` (`kernel/src/arch_impl/aarch64/smp.rs`) | PIT channel 0, machine-wide (`kernel/src/time/timer.rs:33`) |
| IPIs | SGIs 0 and 1 (`kernel/src/arch_impl/aarch64/constants.rs:80`, `:85`), sender `gic::send_sgi()` (`kernel/src/arch_impl/aarch64/gic.rs:831`), receiver `exception.rs:2053-2062` | absent |
| Reschedule IPI in the scheduler | `send_resched_ipi()` / `send_resched_ipi_to_cpu()` / `send_exit_expedite_sgi()` (`kernel/src/task/scheduler.rs:3069`, `:3093`, `:3118`) | the three x86 arms are `let _ = ...` discards (`:3593`, `:3612`, `:3155`) |
| TLB maintenance across CPUs | `tlbi` broadcast within the inner-shareable domain (architectural) | local `invlpg` / CR3 only (`kernel/src/memory/tlb.rs`), comment at `kernel/src/memory/kernel_page_table.rs:536` |
| Idle per CPU | `create_and_register_idle_thread(cpu_id)` then a `wfi` loop in `secondary_cpu_entry_rust` (`kernel/src/arch_impl/aarch64/smp.rs`) | one `idle_loop()` (`kernel/src/interrupts/context_switch.rs:1813`); idle thread registered once (`kernel/src/main.rs:569`) |
| Per-CPU run queues | `per_cpu_queues: [VecDeque<u64>; 8]`, work-stealing, `reclaim_unschedulable_cpu_queues()` (`kernel/src/task/scheduler.rs:1556`) | the same source compiled with `MAX_CPUS = 1`, so the arrays have length 1 |
| CPU identity on the scheduler path | `Aarch64PerCpu::cpu_id()` / MPIDR (`kernel/src/task/scheduler.rs:1506`, `:5360`) | literal `0` (`:1511`, `:5367`) although `X86PerCpu::cpu_id()` exists |

The scheduler core is arch-neutral in the sense that the per-CPU queue,
work-stealing, custody and census machinery is shared source. What is
arch-specific and missing on x86 is: CPU identity, the online count, the
reschedule IPI, and per-CPU idle registration.

---

## 3. Userspace and gate consequences

### 3.1 x86 oracles and gate pins that read on one CPU

| Site | What it assumes |
|---|---|
| `kernel/src/test_framework/registry.rs:4118-4126` | the census-widen oracle emits `arm=none:reason=uniprocessor_no_dispatching_peer:...:SKIP` on x86 because there is no peer CPU to force-place a thread onto (claim-lint:ok: marker quoted from kernel/src/test_framework/registry.rs) |
| `docker/qemu/run-x86-boot-tests.sh:322` | pins that SKIP string as an exact literal |
| `tests/strand_handoff_structure.rs:2082`, `:2165` | pin the same reason token and the gate's literal |
| `docker/qemu/run-x86-boot-tests.sh:321` (`SCHED_STRAND_ORACLE_PATTERN`) | includes `queued_on_nondispatching_cpu`, `worst_queued_nondispatch_ms`, `worst_cpu_scheduler_silence_ms`, `worst_silence_cpu` — axes whose meaning changes once a second CPU can be silent |
| `docker/qemu/run-x86-boot-tests.sh:286-296` | the `TOMBSTONE_QUIESCE` `pending` rationale: "a page-table root cannot be retired while it is the root the CPU currently has installed — and after the last userspace thread exits, nothing on this uniprocessor profile ever loads another one" (claim-lint:ok: quoted from docker/qemu/run-x86-boot-tests.sh) |
| `docker/qemu/run-ext2-lock-race-gate.sh:489` | "by x86 running `-smp 1` (nothing else can contend the lock concurrently during that window)" (claim-lint:ok: quoted from docker/qemu/run-ext2-lock-race-gate.sh) |
| `tests/green_program_envelope_structure.rs:347-360` `x86_gate_scripts_boot_smp_1` | asserts the `-smp N` token equals 1 on the `-machine pc,accel=tcg` line of `docker/qemu/run-x86-tty-oracle-gate.sh` and `docker/qemu/run-x86-prod-profile-boot-test.sh` (list at `:68-71`) |
| `docs/planning/green-program/WORKLOAD-ENVELOPES.md:179-182` | records "**1 virtual CPU**, TCG" for the x86 cells |
| `kernel/src/interrupts/context_switch.rs:1836-1846` | `x86_settled_tombstone_census()` runs from the idle loop, "the only such context on x86" |
| #766, #764 | the measured x86 wake-to-dispatch overruns (p90 2592 ms, max 10318 ms over 324 trials, per #766's title) were taken at `MAX_CPUS=1`; the atlas blended cell says so verbatim: "an x86-only mechanism gated by MAX_CPUS=1" |

`docker/qemu/run-x86-boot-tests.sh` is *not* in `X86_GATE_SCRIPTS`
(`tests/green_program_envelope_structure.rs:68-71` lists two scripts), so the
`-smp` ratchet does not currently watch the boot-tests gate.

### 3.2 What the atlas says

Grepped in
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/atlas/atlas-data.json`:

```
/subsystems[0]/x86/text
  "there is no secondary-CPU bring-up code at all - no smp.rs, no AP
   trampoline, apic present only as a CPUID string
   (arch_impl/x86_64/cpuinfo.rs:229) - so x86 boots uniprocessor, which is
   also why #629 (online_cpu_count() returns MAX_CPUS regardless of -smp,
   reporting 7 phantom CPUs) is a live wrong-answer bug."

/subsystems[5]/x86/summary  (and the same opening in /subsystems[5]/x86/text)
  "the scheduler compiles with MAX_CPUS-wide per-CPU arrays
   (task/scheduler.rs:328,407) but no second CPU is ever started, so all the
   cross-CPU machinery is unexercised and #629 makes the CPU-count query
   return the wrong answer outright."

/subsystems[5]/blended/text
  "MAX_CPUS=1 plus a tail-only re-enqueue on a passed sleep deadline means a
   woken thread waits out the rest of the round-robin, min 47 / p50 420 /
   p90 2592 / max 10318 ms over 368 trials"

/summary/x86/headline
  "x86_64 is a uniprocessor kernel with a broad userspace test surface ..."
```

The first two cells and the third disagree about the CPU count: the #629
parenthetical says "reporting 7 phantom CPUs" while the blended cell says
`MAX_CPUS=1`. The tree agrees with the blended cell
(`kernel/src/task/scheduler.rs:1067`). Proposed wording is in §6.

---

## 4. Plan

Constraint set applied: one capability per PR (R157), <= ~300 non-test lines
each, and each PR carries an oracle with a stated red arm plus a named gate
delta. Line figures are estimates derived from the parity map, not measurements.

The dependency chain is linear: 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7, with 3
optionally deferrable past 4 (see PR-3's note).

### PR-1 — MADT/CPUID enumeration and a count that comes from the hardware

**Files (est. 236 non-test lines)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/arch_impl/x86_64/acpi.rs` (new) | 130 | RSDP validation, RSDT/XSDT walk, MADT (`APIC`) signature match, checksum, iterate type-0 Local APIC and type-1 IOAPIC entries, record LAPIC ids and the enabled / online-capable flag bits |
| `kernel/src/arch_impl/x86_64/smp.rs` (new) | 70 | `MAX_CPUS`, `CPU_PRESENT[]`, `CPU_ONLINE[]`, `CPUS_ONLINE: AtomicU64` seeded at 1, `cpus_online()`, `cpus_present()`, `set_online(cpu)` — the same shape as `kernel/src/arch_impl/aarch64/smp.rs:167-176` |
| `kernel/src/arch_impl/x86_64/mod.rs` | 4 | `pub mod acpi; pub mod smp;` |
| `kernel/src/main.rs` | 20 | pass `boot_info.rsdp_addr` into `acpi::init()` after `memory::init()`; emit the enumeration marker |
| `kernel/src/task/scheduler.rs` | 12 | `online_cpu_count()`'s non-aarch64 arm reads `x86_64::smp::cpus_online()` clamped to `MAX_CPUS`; `MAX_CPUS` on x86 stays 1 until PR-6 |

**Oracle** — new marker `[X86_SMP_ENUM:present=N:enabled=M:online=1:src=madt]`,
emitted once from `kernel_main`. A wrapper runs the same kernel image at
`-smp 1`, `-smp 2` and `-smp 4` and asserts `present` equals the `-smp` value on
each leg. Red arm: hardcoding `present = 1` in `acpi.rs` reddens the `-smp 2` and
`-smp 4` legs and leaves the `-smp 1` leg green — which doubles as the
anti-vacuity check, since a single-leg gate could not tell the two apart.

**Gate delta** — new `docker/qemu/run-x86-smp-enum-gate.sh`, three boots, not in
`X86_GATE_SCRIPTS`. No change to any existing gate's QEMU line.

**Risk** — low. Report-only: the count feeds `online_cpu_count()`, which is
clamped by `MAX_CPUS = 1`, so placement behaviour does not move. The live hazard
is the RSDP mapping — `acpi::init()` has to translate the physical RSDP address
through `physical_memory_offset` and must not fault if the firmware region falls
outside the mapped window. Whether `-cpu qemu64` exposes the x2APIC CPUID bit is
not established here and is not needed by this PR.

**Deps** — no predecessor. Discharges the "suggested first move" recorded in
#629.

---

### PR-2 — LAPIC init on the BSP and IOAPIC routing for the five IRQs in use

**Files (est. 320 non-test lines — see the split note)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/arch_impl/x86_64/lapic.rs` (new) | 150 | xAPIC MMIO window (0xFEE00000) mapped uncacheable, `IA32_APIC_BASE` read, SVR enable plus spurious vector, TPR = 0, mask LVT LINT0/LINT1/PERF/THERMAL, `id()`, `eoi()`. x2APIC reported from CPUID and left unused |
| `kernel/src/arch_impl/x86_64/ioapic.rs` (new) | 110 | IOAPIC MMIO window from the MADT type-1 entry, redirection-table read/write, MADT type-2 interrupt-source-override application (the ISA IRQ to GSI remap), per-IRQ unmask to the BSP LAPIC id |
| `kernel/src/interrupts.rs` | 35 | an `Eoi` indirection so acknowledgement goes to the LAPIC when it is enabled; PIC full-mask (`0xFF` to `0x21` and `0xA1`) after the redirection entries are programmed; `init_pic()` becomes `init_irq_controller()` with the PIC path retained behind a boot flag |
| `kernel/src/arch_impl/x86_64/acpi.rs` | 25 | expose the type-1 / type-2 entries collected in PR-1 |

**Split note** — at ~320 lines this is marginally over the budget. The natural
seam is 2a (LAPIC enable + EOI indirection, IRQs still PIC-delivered, est. 185)
and 2b (IOAPIC routing + PIC full-mask, est. 135), each with its own oracle:
2a's is that the LAPIC reports its own id and the spurious vector is installed
while ticks keep arriving; 2b's is the routing marker below.

**Oracle** — new marker
`[X86_IRQ_ROUTE:src=ioapic:pic_mask1=0xff:pic_mask2=0xff:timer=N:kbd=N:serial=N]`
read after the userspace phase, alongside the existing tick-advance and console
liveness checks. Red arms, run singly: (a) leave the timer's redirection entry
masked -> the tick counter does not advance and the boot fails; (b) skip the PIC
full-mask -> `pic_mask1` reads other than `0xff` and the marker fails its
pattern.

**Gate delta** — `docker/qemu/run-x86-boot-tests.sh` adds the new marker to its
pinned set, still at `-smp 1`.

**Risk** — high, and the highest of the seven. This changes delivery for the
timer, keyboard, COM1 and the two VirtIO lines
(`kernel/src/interrupts.rs:30-38`). The five IRQ handlers each call
`PICS.lock().notify_end_of_interrupt(...)` today (`kernel/src/interrupts.rs:525`,
`:570`, `:594`, `:619`, `:1524`) and each has to be re-pointed. `send_timer_eoi()`
lives in the Tier-1 `kernel/src/interrupts/timer.rs:130` and its direct-port
fallback arm (`:139-158`) hardcodes the 8259 command ports — so this PR touches a
Tier-1 file and needs the operator sign-off CLAUDE.md requires. Any existing x86
gate can redden here, since a lost IRQ class is a boot failure. Interaction with
#567: this shifts the timing of the pre-userspace window in which #567's
corrupted kernel-thread resume appears, so a change in #567's recurrence rate is
expected and has to be attributed rather than absorbed.

**Deps** — PR-1 (needs the MADT walk).

---

### PR-3 — LAPIC timer as the scheduler tick, replacing the PIT

**Files (est. 190 non-test lines)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/arch_impl/x86_64/lapic.rs` | 80 | LVT timer entry, divide-configuration register, periodic mode, one-shot calibration against PIT channel 2 (the mechanism `kernel/src/time/tsc.rs:30-36` already uses), `arm_periodic(hz)` |
| `kernel/src/time/timer.rs` | 45 | tick-source selection; `PIT_HZ` retained as the calibration reference; `init()` arms the LAPIC timer when it is available and the PIT otherwise |
| `kernel/src/interrupts.rs` | 20 | a dedicated LAPIC-timer vector distinct from PIC vector 32 |
| `kernel/src/interrupts/timer.rs` (**Tier-1**) | 25 | EOI target becomes the LAPIC; `static mut CURRENT_QUANTUM` (`:35-36`) becomes a per-CPU field so a second CPU does not share one decrementing counter |
| `kernel/src/interrupts/timer_entry.asm` (**Tier-1**) | 20 | the `call send_timer_eoi` sites at `:327` and `:364` follow the new target |

**Tier-1 disclosure** — `kernel/src/interrupts/timer.rs` and
`kernel/src/interrupts/timer_entry.asm` are both on the Tier-1
prohibited-modifications list (CLAUDE.md, "Absolutely Forbidden (ask before ANY
change)"). This PR cannot start without explicit operator approval, and it adds
no logging to either file.

**Oracle** — `[X86_TICK_SOURCE:lapic:hz=200:calibrated_ticks=N]` plus a tick-rate
sample: ticks observed over a wall-clock window from
`clock_gettime(CLOCK_MONOTONIC)` within a stated tolerance of 200 Hz, and the
existing monotonicity assertions in `kernel/src/clock_gettime_test.rs` and
`kernel/src/time_test.rs` held unchanged. Red arm: mis-set the divide
configuration by one power of two -> the sampled rate lands outside tolerance and
the marker's `hz` field disagrees with the measurement.

**Gate delta** — the new marker joins `docker/qemu/run-x86-boot-tests.sh`'s
pinned set; `docker/qemu/run-x86-prod-profile-boot-test.sh` gains the tick-rate
sample alongside its existing console-prompt liveness sample.

**Risk** — high. The 200 Hz PIT tick is the reference the x86 timing surface is
calibrated against, and #766/#764's latency distributions were measured against
it, so those envelopes need re-deriving after this lands. #700 (x86 futex timeout
not returning `ETIMEDOUT`) sits directly downstream of tick arithmetic and could
change rate in either direction.

**Note on ordering** — PR-3 is a prerequisite for per-CPU ticks (PR-6) but not for
AP bring-up (PR-4). It can be deferred to sit between PR-4 and PR-5 if the
operator prefers to get APs parked before touching Tier-1 timer files.

**Deps** — PR-2 (LAPIC has to be enabled and the EOI path re-pointed).

---

### PR-4 — AP trampoline, per-CPU descriptor tables and state, APs parked in `hlt`

**Files (est. 340 non-test lines — over the ~300 budget; split proposed)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/arch_impl/x86_64/ap_trampoline.S` (new) | 110 | 16-bit entry at a page-aligned address below 1 MiB, load GDT, enter protected then long mode against the BSP's CR3, read this AP's stack pointer and CPU id from a parameter block, jump to `ap_entry_rust` |
| `kernel/src/arch_impl/x86_64/smp.rs` | 110 | copy the trampoline into its low frame, INIT-assert / INIT-deassert / SIPI / SIPI via the LAPIC ICR with the standard 10 ms and 200 us waits, per-CPU bring-up stage array mirroring `kernel/src/arch_impl/aarch64/smp.rs:231-272`, bounded wait for `CPUS_ONLINE` to reach the expected value |
| `kernel/src/per_cpu.rs` | 55 | `ALL_CPU_DATA: [PerCpuData; MAX_CPUS]` replacing `CPU0_DATA` (`:356`), `init_cpu(cpu_id)` replacing the parameterless `init()` (`:371`), read-back check against `cpu_id` rather than the literal 0 (`:398-403`) |
| `kernel/src/gdt.rs` | 45 | per-CPU `GDT`/`TSS` array, `init_cpu(cpu_id)`, `get_tss_ptr()` returns this CPU's TSS |
| `kernel/src/memory/per_cpu_stack.rs` | 15 | `init_per_cpu_stacks(n)` driven by the enumerated count; the two `let cpu_id = 0` sites (`:94`, `:105`) read `X86PerCpu::cpu_id()` |
| `kernel/src/memory/kernel_page_table.rs` | 5 | identity-map the trampoline frame in the master PML4 |

**Split proposal** — PR-4a: trampoline + ICR sequence, APs reach long mode and
`hlt` on a statically allocated stack, no per-CPU kernel state (est. 190 lines).
PR-4b: per-CPU `PerCpuData`, GDT/TSS, IST and emergency stacks (est. 150 lines).
4a's oracle is the online count; 4b's is the per-CPU heartbeat.

**Oracle** — per-AP marker `[X86_AP:cpu=N:stage=online:lapic_id=M]` and a summary
`[X86_SMP_ONLINE:expected=N:online=N]`, plus a heartbeat
`[X86_AP_HEARTBEAT:cpu=N:seq=S]` emitted from the AP park loop under a rate
limit, so a CPU that reaches `online` and then stops is distinguishable from one
that keeps running. Red arms, run singly: (a) drop the second SIPI -> `online`
falls short of `expected`; (b) point two APs at the same stack -> the heartbeat
sequence from one of them stops advancing.

**Gate delta** — new `docker/qemu/run-x86-smp-gate.sh` at `-smp 2` and `-smp 4`.
Existing gates stay at `-smp 1` and gain no new marker, because with APs parked
in `hlt` and unregistered with the scheduler, a `-smp 1` boot's behaviour is
unchanged.

**Risk** — high. Three specific hazards: (i) the low-memory trampoline frame has
to be reserved out of the bootloader's memory map before the frame allocator
hands it to something else; (ii) `gdt::init()`'s `OnceCell` shape
(`kernel/src/gdt.rs:12-13`) means the single-load pattern has to become an array
without breaking `TSS_PTR` (`:14`), which the dispatch path reads on each switch;
(iii) #567 is an open corruption of kernel-thread resume in the pre-userspace
window, and this PR adds new pre-userspace execution contexts. The `-smp 1` legs
of existing gates ought to be unaffected — that is the property the round has to
demonstrate, not assume.

**Deps** — PR-1 (enumeration), PR-2 (LAPIC ICR).

---

### PR-5 — IPIs and TLB shootdown

**Files (est. 220 non-test lines)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/arch_impl/x86_64/lapic.rs` | 45 | `send_ipi(target_lapic_id, vector)`, `send_ipi_all_but_self(vector)`, ICR delivery-status wait |
| `kernel/src/arch_impl/x86_64/smp.rs` | 45 | two vectors — `VEC_RESCHEDULE` and `VEC_TLB_SHOOTDOWN` — plus their handler bodies and an ack counter per request |
| `kernel/src/interrupts.rs` | 30 | IDT entries for the two vectors, LAPIC EOI on both |
| `kernel/src/memory/tlb.rs` | 60 | a shootdown request record (address range plus generation), broadcast, and a bounded ack rendezvous; `flush_page` keeps its local fast path while one CPU is online |
| `kernel/src/memory/process_memory.rs`, `kernel/src/memory/kernel_page_table.rs` | 40 | unmap and permission-change sites request the shootdown; the `no remote shootdown needed` comment at `kernel_page_table.rs:536` is re-derived |

**Oracle** — a boot test: the BSP maps a page with a known value, an AP reads it
(caching the translation), the BSP unmaps it and requests the shootdown, the AP
re-reads and reports a fault. Marker
`[X86_TLB_SHOOTDOWN:reqs=N:acks=N:peers=M:stale_reads=0]` with
`acks == reqs * peers`. Red arm: make the shootdown local-only -> the AP's re-read
succeeds against the stale translation and `stale_reads` rises above 0.

**Gate delta** — the shootdown marker joins `docker/qemu/run-x86-smp-gate.sh`
(`-smp 2` and `-smp 4`). The `-smp 1` legs assert `peers=0` and the local fast
path.

**Risk** — medium-high. A shootdown rendezvous with interrupts disabled on the
requester and a peer that is itself spinning is the classic deadlock shape; the
ack wait needs a bound and a fail-loud path. Interaction with #791's repair: the
master-PML4 read is an `AtomicU64` (`kernel/src/memory/kernel_page_table.rs:53`)
and stays lock-free, but this PR introduces a *new* IF=0 cross-CPU wait on the
same path, and `scripts/check-x86-dispatch-no-alloc.sh` plus
`tests/dispatch_path_lock_free_structure.rs` both police that path — both have to
stay green, and neither is currently written to see a rendezvous spin.

**Deps** — PR-4 (needs a second CPU that can acknowledge).

---

### PR-6 — Scheduler dispatch on the APs

**Files (est. 280 non-test lines)**

| file | est. lines | what |
|---|---|---|
| `kernel/src/task/scheduler.rs` | 120 | `MAX_CPUS` on x86 raised from 1 (`:1067`); `Scheduler::current_cpu_id()` (`:1509-1512`) and `current_cpu_id_raw()` (`:5366-5368`) read `X86PerCpu::cpu_id()`; `is_cpu_idle_raw()` (`:5380-5383`) gets a real body; the three `let _ = wake.resched_target()` discards (`:3593`, `:3612`, `:3155`) become IPI sends; `send_resched_ipi` / `send_resched_ipi_to_cpu` gain x86 arms |
| `kernel/src/arch_impl/x86_64/smp.rs` | 50 | the AP entry registers an idle thread for its CPU and enters the dispatch loop instead of the park loop — the shape of `secondary_cpu_entry_rust`'s `create_and_register_idle_thread(cpu_id)` (`kernel/src/arch_impl/aarch64/smp.rs`) |
| `kernel/src/main.rs` | 30 | idle-thread creation becomes per CPU (today `per_cpu::set_idle_thread(...)` once at `:569`) |
| `kernel/src/interrupts/context_switch.rs` (**Tier-2**) | 60 | `idle_loop()` (`:1813`) becomes per-CPU-safe: `x86_settled_tombstone_census()` (`:1846`) and `report_dispatch_strand_census_heartbeat()` (`:1836`) are rate-limited singletons today and would be entered by N idle loops |
| `kernel/src/interrupts/timer.rs` (**Tier-1**) | 20 | per-CPU quantum, if PR-3 has not already done it |

**Which aarch64 ratchets apply** — named rather than assumed:

1. The scheduler-lock IRQ-safety property. On x86 the lock is taken inside
   `without_interrupts` (`kernel/src/task/scheduler.rs:4681`, and
   `tests/dispatch_path_lock_free_structure.rs`'s module doc states the
   invariant: `with_thread_mut` "holds it inside `without_interrupts`, so no
   holder can be preempted while holding it"). With APs live, that property stops
   being sufficient on its own — a peer CPU can hold the lock while this CPU
   spins with IF=0 — so the aarch64 lock-order and irqsave-typed-lock discipline
   (#609's repair) becomes an x86 requirement too.
2. `scripts/check-x86-dispatch-no-alloc.sh` and
   `tests/dispatch_path_lock_free_structure.rs` — the dispatch path stays
   allocation-free and free of locks that ordinary thread context holds with
   interrupts enabled.
3. `scripts/check-critical-path-violations.sh` — no logging added to the
   interrupt or syscall paths.
4. The strand census's per-CPU axes (`SCHED_STRAND_ORACLE_PATTERN`,
   `docker/qemu/run-x86-boot-tests.sh:321`) start carrying real values on x86
   rather than degenerate ones.

The aarch64 ASID invariants have no direct x86 analogue in this plan: x86's
equivalent is PCID/INVPCID, and PCID is out of scope (§5). The x86 model stays
"CR3 reload flushes the local TLB" plus PR-5's explicit shootdown.

**Oracle** — a boot test that places a kernel thread on CPU 1 and asserts it runs
there, reported as `[X86_DISPATCH_CPU:tid=T:cpu=1:iters=N]`, plus retirement of
the `arm=none:reason=uniprocessor_no_dispatching_peer` SKIP at
`kernel/src/test_framework/registry.rs:4123` in favour of the armed pass the
aarch64 leg produces. Red arm: force `current_cpu_id()` back to the literal `0`
-> the thread reports `cpu=0` and the marker fails.
claim-lint:ok: the retired marker text is quoted from
kernel/src/test_framework/registry.rs

**Gate delta** — `docker/qemu/run-x86-smp-gate.sh` gains the dispatch marker;
`docker/qemu/run-x86-boot-tests.sh:322`'s `CENSUS_WIDEN_ORACLE_LITERAL` and the
two pins in `tests/strand_handoff_structure.rs` (`:2082`, `:2165`) change from
the SKIP literal to the armed form.

**Risk** — the largest blast radius of the seven, because it is the first PR in
which x86 behaviour at `-smp 1` can change: raising `MAX_CPUS` above 1 re-opens
exactly the "CPU index above the online count" shape #629's body describes, and
`offline_queue_occupancy()` (`kernel/src/task/scheduler.rs:5261-5268`) and
`reclaim_unschedulable_cpu_queues()` (`:1556`) start having a non-empty range to
walk. #766/#764's latency envelopes were measured at `MAX_CPUS=1` and stop
applying. #812's contrast paragraph — which says x86 avoids the NetRx/PM
self-deadlock "by accident of a different gate", `kernel/src/per_cpu.rs:716-724`
running `do_softirq()` only at `preempt_count() == 0` — stays true per CPU, but
the *cross-CPU* variant (CPU A holds `PROCESS_MANAGER`, CPU B's softirq spins on
the same `spin::Mutex`, `kernel/src/process/mod.rs:146`) becomes reachable for
the first time on x86.

**Deps** — PR-4 and PR-5 (a dispatching AP needs the reschedule IPI; a dispatched
process needs the shootdown).

---

### PR-7 — Gates at `-smp 2` and `-smp 4` as new profiles

**Files (est. 180 non-test lines, mostly shell)**

| file | est. lines | what |
|---|---|---|
| `docker/qemu/run-x86-smp-gate.sh` (new, carried forward from PR-4) | 90 | the standing SMP gate: `-smp 2` and `-smp 4` profiles, marker set from PRs 1/4/5/6 |
| `docker/qemu/run-x86-boot-tests.sh` | 35 | a `BREENIX_X86_SMP` variable defaulting to 1, so the standing profile is unchanged and the SMP legs are additional runs |
| `docker/qemu/run-x86-prod-profile-boot-test.sh` | 25 | the same, subject to its verdict-path discipline (`tests/teardown_structure.rs`'s `x86_production_profile_gate_verdict_discipline_holds`: no `exit` may pre-empt the verdict — see `docs/planning/green-program/gates/GATE-PREFLIGHT-VERDICT-802-2026-09-05.md`) |
| `tests/green_program_envelope_structure.rs` | 20 | `x86_gate_scripts_boot_smp_1` (`:347-360`) re-derived — see the risk note |
| `docs/planning/green-program/WORKLOAD-ENVELOPES.md` | 10 | §2's "**1 virtual CPU**, TCG" entry (`:179-182`) restated per profile |

**Oracle** — the gate's own verdict on each new profile, with the PR-1/4/5/6
markers pinned per leg. Anti-vacuity: the `-smp 4` leg has to fail if the kernel
reports `online=1`, which the PR-1 enumeration marker already distinguishes.

**Gate delta** — this *is* the gate delta: two new x86 profiles, with `-smp 1`
retained as the standing profile until the SMP legs are green over a stated
number of boots.

**Risk** — `x86_gate_scripts_boot_smp_1`
(`tests/green_program_envelope_structure.rs:347-360`) is an equality assertion
against the `-smp` token on the `-machine pc,accel=tcg` line of the two watched
scripts (`:68-71`). It reddens the moment either watched script's anchor line
carries anything but `1`. Two honest routes, and the operator picks (§7 Q4): keep
the anchor lines at `-smp 1` and put the SMP legs in a separate, unwatched
script; or widen the ratchet and re-derive the WORKLOAD-ENVELOPES §2 entry in the
same PR, which is what the ratchet's own failure message asks for ("re-derive
TTY-x86's envelope before trusting it under the new CPU count").

**Deps** — PR-6.

---

## 5. Not in the plan

Named so the boundary is explicit rather than implied:

- **NUMA** — node discovery (SRAT/SLIT), node-local allocation, node-aware
  placement.
- **CPU hotplug / offline** — `CPU_OFF`, offlining a running CPU, migrating its
  queue on the way out. PR-4's bring-up is one-shot at boot.
- **x2APIC-only systems** — the plan uses xAPIC MMIO. x2APIC is detected and
  reported (`kernel/src/arch_impl/x86_64/cpuinfo.rs:298` already surfaces the
  CPUID bit) but the MSR access path is not implemented, so a machine that comes
  up with x2APIC latched and xAPIC unavailable is out of scope.
- **Real x86 hardware** — the acceptance target is QEMU. Parallels has no x86
  path in this repo (`run.sh --parallels` and `scripts/parallels/` are aarch64),
  and VMware's x86 path likewise does not exist here.
- **PCID / INVPCID** and any ASID-equivalent tagging. The TLB model stays
  "CR3 reload flushes local" plus PR-5's explicit shootdown.
- **Tickless and high-resolution per-CPU timers**, TSC-deadline mode, and
  clocksource selection beyond the LAPIC-vs-PIT choice in PR-3.
- **SMP load balancing** beyond the existing arch-neutral work-stealing plus the
  reschedule IPI — no periodic balancer, no scheduling domains, no affinity API.
- **MSI/MSI-X on x86** (the aarch64 side has GICv2m); x86 device interrupts stay
  on the IOAPIC lines already in use.
- **An AML interpreter** — PR-1 parses fixed ACPI tables only.
- **The BIOS boot path** — `Cargo.toml:73` limits the bootloader to UEFI.
- **#567, #700, #766, #764, #812** — pre-existing defects this plan interacts
  with (the §4 risk rows) but does not undertake to repair.

---

## 6. Atlas wording

Two atlas cells carry a #629 parenthetical the tree contradicts, and a third
already states the tree's number. Proposed replacements, both keeping the
capability-gap verdict unchanged.

**`/subsystems[0]/x86/text` — current:**

```
Status is partial for one concrete reason: there is no secondary-CPU bring-up
code at all — no smp.rs, no AP trampoline, apic present only as a CPUID string
(arch_impl/x86_64/cpuinfo.rs:229) — so x86 boots uniprocessor, which is also
why #629 (online_cpu_count() returns MAX_CPUS regardless of -smp, reporting 7
phantom CPUs) is a live wrong-answer bug.
```

**Proposed:**

```
Status is partial for one concrete reason: there is no secondary-CPU bring-up
code at all — no smp.rs, no AP trampoline, apic present only as a CPUID string
(arch_impl/x86_64/cpuinfo.rs:229) — so x86 boots uniprocessor. #629 is the
reporting half of that gap, and its body is now stale in one detail: MAX_CPUS
has been 1 on x86 since 25afe0e1 (task/scheduler.rs:1067), so
online_cpu_count() returns 1 rather than the seven phantom peers the issue
describes. What stands is that x86 answers the CPU-count question from a
compile-time constant instead of from an enumeration — right at -smp 1 by
construction, and wrong the moment a second CPU exists. Its live residue is the
census-widen oracle's x86 arm, which emits
arm=none:reason=uniprocessor_no_dispatching_peer:...:SKIP
(test_framework/registry.rs:4123) because there is no peer CPU to force-place a
thread onto.
```

**`/subsystems[5]/x86/summary` (and the identical opening of
`/subsystems[5]/x86/text`) — current:**

```
Status is partial for the SMP reason above: the scheduler compiles with
MAX_CPUS-wide per-CPU arrays (task/scheduler.rs:328,407) but no second CPU is
ever started, so all the cross-CPU machinery is unexercised and #629 makes the
CPU-count query return the wrong answer outright.
```

**Proposed:**

```
Status is partial for the SMP reason above: the scheduler's per-CPU arrays
compile at MAX_CPUS = 1 on this arch (task/scheduler.rs:1067) and no second CPU
is started, so the cross-CPU machinery — per-CPU queues, work-stealing, the
reschedule IPI, the retirement grace — is carried as source but not exercised.
Scheduler::current_cpu_id() returns the literal 0 here (task/scheduler.rs:1511)
while X86PerCpu::cpu_id() reads GS and would answer correctly, and the three
reschedule-IPI call sites are `let _ =` discards (task/scheduler.rs:3155, 3593,
3612). #629 stands as the count-from-a-constant half of that, restated: not
seven phantom CPUs, one hardcoded answer.
```

`/subsystems[5]/blended/text` already says MAX_CPUS=1 and needs no change; the
two cells above are what disagree with it.

`/summary/x86/headline` ("x86_64 is a uniprocessor kernel ...") stays accurate
until PR-4 lands.

---

## 7. Open questions for the operator

1. **TCG vs KVM at `-smp > 1` on beast.** The x86 gates hardcode `accel=tcg`
   (`docker/qemu/run-x86-boot-tests.sh:415`,
   `docker/qemu/run-x86-prod-profile-boot-test.sh:1030`), while #700 reports
   measurements from "beast KVM x86 boots". Which accelerator do the `-smp 2` and
   `-smp 4` profiles run under, and should MTTCG be requested explicitly
   (`accel=tcg,thread=multi`, as `scripts/run-interactive-native.sh:65` already
   does) rather than left to QEMU's default?
2. **Tier-1 approval.** PR-3 edits `kernel/src/interrupts/timer.rs` and
   `kernel/src/interrupts/timer_entry.asm`; PR-6 edits the per-CPU quantum in the
   same file; PR-2 re-points `send_timer_eoi()` there. 3 of 3 are Tier-1.
   Approve, or should the LAPIC EOI be routed through a non-Tier-1 seam even at
   the cost of an extra indirection on the tick path?
3. **When does x86 `MAX_CPUS` leave 1?** PR-1 as written leaves it at 1 and makes
   only the enumeration honest; PR-6 raises it. Raising it earlier makes the count
   reportable sooner but re-opens the index-above-online-count shape #629's body
   describes, on a kernel with no IPI to rescue a mis-placed thread.
4. **The `-smp` ratchet.** `x86_gate_scripts_boot_smp_1`
   (`tests/green_program_envelope_structure.rs:347`) asserts equality with 1 on
   two watched scripts. Widen it with a re-derived WORKLOAD-ENVELOPES §2 entry, or
   keep those two scripts at `-smp 1` and add a separate unwatched SMP gate?
5. **Sequencing against #567.** #567 is an open corruption of kernel-thread
   resume in the pre-userspace window, and four network tests are deferred for it
   (`docker/qemu/run-x86-boot-tests.sh:424-426`). PR-4 adds new pre-userspace
   execution contexts to exactly that window. Hold x86 SMP behind #567, or land it
   behind a default-off boot flag with the SMP legs as new gates only?
6. **The trampoline's low frame.** The AP trampoline needs a page-aligned frame
   below 1 MiB that the frame allocator does not hand out. Reserve one from the
   bootloader's memory map inside the kernel, or ask the bootloader for it?
7. **Tick frequency across PR-3.** `PIT_HZ = 200`
   (`kernel/src/time/timer.rs:14`) is the reference the x86 timing surface and
   #766/#764's distributions were measured against. Hold the LAPIC timer at 200 Hz
   for comparability, or take the opportunity to change it and re-derive those
   envelopes in the same round?
8. **Acceptance bar.** Confirm QEMU-only acceptance for x86 SMP (§5), given that
   Parallels x86 does not exist in this repo and beast is the sole x86 host.

---

## Claim-lint

```
claim-lint: scripts/claim-lint.py --files /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/x86-smp/X86-SMP-CENSUS-PLAN-2026-09-05.md -> exit 0
```

Output: `claim-lint: clean (1 file(s) checked, whole files).`

Two discharges were needed and are recorded inline rather than by rewording,
because both quote tree text verbatim: the census-widen marker string from
`kernel/src/test_framework/registry.rs` and the two single-CPU rationale
comments from `docker/qemu/run-x86-boot-tests.sh` and
`docker/qemu/run-ext2-lock-race-gate.sh`.

---

## 8. Addendum: what PR-1 changed about this document (added at commit time)

PR-1 landed on branch `x86/smp-pr1-madt-enum`. Its round doc is
`docs/planning/x86-smp/PR1-ENUM-2026-09-05.md`, which carries the marker
semantics, the `-smp 1/2/4` evidence and the mutation table. This section
records only the places where the body above stopped describing the tree.

### 8.1 Statements the body makes that PR-1 moved

| where | what the body says | what PR-1 made true |
|---|---|---|
| §1.6 | "The kernel does not read it", of `boot_info.rsdp_addr` | `kernel/src/main.rs:187` reads it and `:198` hands it to `arch_impl::x86_64::smp::init` |
| §1.6 | `grep -rn "rsdp" kernel/src` → 1 hit, the aarch64 `HardwareConfig` field | 3 hits: that field, plus the two `main.rs` lines above |
| §1.10 | "x86 answers the CPU-count question from a compile-time constant rather than from an enumeration" | `online_cpu_count()`'s x86 arm reads `arch_impl::x86_64::smp::cpus_online()`, an `AtomicU64`. The VALUE it answers is still 1, because `CPUS_ONLINE` is seeded at 1 and PR-1 starts no processor |
| §2, row "CPU enumeration" | x86: "absent" | x86: `arch_impl/x86_64/acpi.rs` (RSDP → RSDT/XSDT → MADT type 0 / type 9) plus a CPUID cross-check |
| §2, row "Online count" | x86: `online_cpu_count()` returns `MAX_CPUS` | x86: `(smp::cpus_online() as usize).clamp(1, MAX_CPUS)`, the same expression the aarch64 arm uses |
| §4, PR-1 "Oracle" row | planned marker `[X86_SMP_ENUM:present=N:enabled=M:online=1:src=madt]`, with `present` tracking `-smp` | the landed marker carries ten fields, and the field that tracks `-smp` is `madt_cpus`, not `present`. `present` is the MADT count clamped to `MAX_CPUS`, so it reads 1 on each leg. The two numbers were split deliberately: see §8.3 |
| §4, PR-1 "Files" row | `CPU_PRESENT[]`, `CPU_ONLINE[]` arrays | the landed `smp.rs` carries `CPUS_ONLINE`/`CPUS_PRESENT` counters plus an `APIC_IDS` array. A per-CPU online bitmap has no reader while one processor runs, so it was left for the PR that starts a second one |

### 8.2 Atlas wording, re-proposed for the post-PR-1 tree

§6 above proposed replacement text for two atlas cells. Both proposals say
x86 "answers the CPU-count question from a compile-time constant", which PR-1
makes stale. These supersede them; the capability-gap verdict is unchanged,
because enumeration is not bring-up.

**`/subsystems[0]/x86/text` — proposed:**

```
Status is partial for one concrete reason: there is no secondary-CPU bring-up
code at all - no AP trampoline, no INIT/SIPI, apic present only as a CPUID
string (arch_impl/x86_64/cpuinfo.rs:229) - so x86 boots uniprocessor. #814 PR-1
closed the reporting half of that gap: arch_impl/x86_64/acpi.rs reads the
firmware's MADT and arch_impl/x86_64/smp.rs answers cpus_online() from an
atomic, so a -smp 4 boot now REPORTS madt_cpus=4 while online stays 1 - the
one-line [X86_SMP_ENUM:...] marker kernel_main emits, scored across -smp 1/2/4
by docker/qemu/run-x86-smp-enum-gate.sh. #629's body is stale in one detail
(MAX_CPUS has been 1 on x86 since 25afe0e1, so online_cpu_count() returned 1,
not the seven phantom peers it describes); what remains open under it is that
the count is still capped at MAX_CPUS = 1, and its live residue is the
census-widen oracle's x86 arm, which emits
arm=none:reason=uniprocessor_no_dispatching_peer:...:SKIP
(test_framework/registry.rs) because there is no peer CPU to force-place a
thread onto.
```

**`/subsystems[5]/x86/summary` (and the identical opening of
`/subsystems[5]/x86/text`) — proposed:**

```
Status is partial for the SMP reason above: the scheduler's per-CPU arrays
compile at MAX_CPUS = 1 on this arch (task/scheduler.rs) and no second CPU is
started, so the cross-CPU machinery - per-CPU queues, work-stealing, the
reschedule IPI, the retirement grace - is carried as source but not exercised.
Scheduler::current_cpu_id() returns the literal 0 here while X86PerCpu::cpu_id()
reads GS and would answer correctly, and the three reschedule-IPI call sites are
`let _ =` discards. online_cpu_count() no longer answers from the bare constant:
since #814 PR-1 it reads arch_impl::x86_64::smp::cpus_online(), clamped to
MAX_CPUS - the same expression aarch64 uses. The value is still 1, because PR-1
enumerates processors without starting one.
```

`/subsystems[5]/blended/text` and `/summary/x86/headline` are unaffected: the
`MAX_CPUS=1` statement and the "uniprocessor kernel" headline both still hold
after PR-1, which starts no processor.

### 8.3 Why `madt_cpus` and `present` are two fields

The plan's single `present=N` would have had to be either the firmware's count
(which exceeds what the scheduler's per-CPU arrays can index, at
`MAX_CPUS = 1`) or the clamped one (which cannot move with `-smp`, so it could
not detect a hardcoded enumeration). Splitting them lets the marker carry both
without either lying: `madt_cpus` is what the firmware reported, and `present`
is what this kernel can address. The gate asserts the first against the leg's
`-smp` value and the second against 1.

### 8.4 What PR-1 leaves for the later rows of §4

PR-1 touches neither the LAPIC nor the IOAPIC nor the PIT, and it adds no AP
entry path, so the §4 rows PR-2 through PR-7 stand as written. The
8 operator questions in §7 are also unanswered by it: 0 of 8 are settled here,
and question 2 (Tier-1 approval) does not arise, since PR-1 edits no Tier-1
file. Question 4 gains one
data point: the new gate is unwatched by
`tests/green_program_envelope_structure.rs`'s `X86_GATE_SCRIPTS` (a two-script
list), so the `-smp 1` ratchet on the two watched gates is untouched by it.
