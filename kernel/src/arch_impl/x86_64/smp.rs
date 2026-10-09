//! x86_64 processor enumeration, and the CPU counts answered from it.
//!
//! ## What this module does, and what it does not
//!
//! It records what the firmware's MADT reports about the processors on this
//! machine (via `super::acpi`), cross-checks that against CPUID, and answers
//! `cpus_online()` / `cpus_present()` / `is_cpu_online()` from atomics instead
//! of from a compile-time constant.
//!
//! The boot processor is online from the first instruction. Each application
//! processor is started by `super::ap_start` and marks itself online with
//! `mark_online` once it has run `super::cpu_init::init_cpu` and registered
//! its idle thread with the scheduler.
//!
//! ## Logical CPU numbers
//!
//! A logical CPU number indexes every per-CPU array (`PerCpuData`, the GDT and
//! TSS, the IST stacks, the scheduler's run queues). The boot processor is 0;
//! secondaries are numbered in the order they come online, so the online CPUs
//! are always `0..cpus_online()`. `cpu_apic_id()` maps a logical number to the
//! local APIC id an IPI is addressed to, as recorded by that CPU's own per-CPU
//! init.
//!
//! ## Two different numbers
//!
//! `madt_cpu_count()` is what the firmware reports — it tracks `-smp N`.
//! `cpus_present()` is that count clamped into what this kernel's per-CPU
//! state can address, `crate::task::scheduler::MAX_CPUS`. Neither is the
//! online count: placement and every per-CPU loop that must see only running
//! CPUs read `cpus_online()` or `is_cpu_online()`.
//!
//! The aarch64 counterpart is `crate::arch_impl::aarch64::smp`, whose
//! `CPUS_ONLINE`/`CPU_ONLINE`/`cpus_online()`/`is_cpu_online()` shape this
//! mirrors. See #814 for the staged plan and #629 for the count-from-a-constant
//! defect.

use core::arch::x86_64::{__cpuid, __cpuid_count};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::acpi::{self, MAX_ENUMERATED_CPUS};
use crate::task::scheduler::MAX_CPUS;

/// Where the enumeration came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnumerationSource {
    /// `init()` has not run yet.
    NotRun,
    /// The firmware's MADT was read.
    Madt,
    /// The MADT walk refused; CPUID answered instead.
    CpuidFallback,
}

const SOURCE_NOT_RUN: u32 = 0;
const SOURCE_MADT: u32 = 1;
const SOURCE_CPUID_FALLBACK: u32 = 2;

/// Processors currently online, in the sense the scheduler means: entered and
/// able to dispatch. Seeded at 1 for the boot processor; each application
/// processor raises it in `mark_online`.
static CPUS_ONLINE: AtomicU64 = AtomicU64::new(1);

/// Per-CPU online flags, indexed by logical CPU number. The boot processor,
/// CPU 0, is online from the first instruction the kernel runs.
static CPU_ONLINE: [AtomicBool; MAX_CPUS] = {
    let mut flags = [const { AtomicBool::new(false) }; MAX_CPUS];
    flags[0] = AtomicBool::new(true);
    flags
};

/// Local APIC id of each logical CPU, recorded by that CPU's own per-CPU init.
/// `NO_APIC_ID` until then.
static CPU_APIC_ID: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(NO_APIC_ID) }; MAX_CPUS];

/// `CPU_APIC_ID` value for a CPU whose per-CPU init has not recorded one.
const NO_APIC_ID: u32 = u32::MAX;

/// Processors this kernel's per-CPU state can address: the MADT count clamped
/// into `[1, MAX_CPUS]`.
static CPUS_PRESENT: AtomicU64 = AtomicU64::new(1);

/// Processor entries the MADT reported, unclamped. 0 until `init()` runs, and
/// 0 afterwards when the walk refused.
static MADT_CPUS: AtomicU32 = AtomicU32::new(0);

/// Of those, the ones flagged enabled.
static MADT_ENABLED: AtomicU32 = AtomicU32::new(0);

/// Of those, the ones that arrived as type-9 (x2APIC) entries.
static MADT_X2APIC: AtomicU32 = AtomicU32::new(0);

/// APIC ids in MADT order, for the entries a census recorded.
static APIC_IDS: [AtomicU32; MAX_ENUMERATED_CPUS] =
    [const { AtomicU32::new(0) }; MAX_ENUMERATED_CPUS];

/// Whether each recorded MADT entry carries the Enabled flag.
static APIC_ENABLED: [AtomicBool; MAX_ENUMERATED_CPUS] =
    [const { AtomicBool::new(false) }; MAX_ENUMERATED_CPUS];

/// MADT entries, by index, whose processor was sent a startup sequence and
/// did not come online. Bit `n` is entry `n`.
static UNANSWERED: AtomicU64 = AtomicU64::new(0);

/// How many of `APIC_IDS` are populated.
static APIC_ID_COUNT: AtomicU32 = AtomicU32::new(0);

/// The boot processor's own APIC id, read from CPUID rather than from the
/// MADT: it is the id of the processor executing this code.
static BSP_APIC_ID: AtomicU32 = AtomicU32::new(0);

/// One of the `SOURCE_*` codes above.
static SOURCE: AtomicU32 = AtomicU32::new(SOURCE_NOT_RUN);

/// Set by the first `init()`, so a second call cannot emit a second marker.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Ticks each CPU has been credited with, and the tick each CPU's last credit
/// ran to. Read by /proc/stat beside the idle ticks credited to
/// `IDLE_TICK_TOTAL`.
static ELAPSED_TICKS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static CREDITED_TO_TICK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Credit logical CPU `cpu`, the executing CPU, with the ticks since its
/// previous credit: to its elapsed time, and to its idle time when `idle`, the
/// interrupted thread being its idle thread. Called on every interrupt return
/// through the scheduler, which every tick makes, as aarch64's timer interrupt
/// credits its CPU. No lock.
pub fn credit_ticks(cpu: usize, idle: bool) {
    if cpu >= MAX_CPUS {
        return;
    }
    let now = crate::time::get_ticks();
    let last = CREDITED_TO_TICK[cpu].swap(now, Ordering::Relaxed);
    if last == 0 || now <= last {
        return;
    }
    ELAPSED_TICKS[cpu].fetch_add(now - last, Ordering::Relaxed);
    if idle {
        crate::tracing::providers::counters::IDLE_TICK_TOTAL.add_cpu(cpu, now - last);
    }
}

/// Ticks logical CPU `cpu` has been credited with.
pub fn cpu_elapsed_ticks(cpu: usize) -> u64 {
    ELAPSED_TICKS
        .get(cpu)
        .map_or(0, |ticks| ticks.load(Ordering::Relaxed))
}

/// Number of processors online. Mirrors
/// `crate::arch_impl::aarch64::smp::cpus_online()`.
#[inline(always)]
pub fn cpus_online() -> u64 {
    CPUS_ONLINE.load(Ordering::Acquire)
}

/// Whether logical CPU `cpu` is online.
#[inline(always)]
pub fn is_cpu_online(cpu: usize) -> bool {
    cpu < MAX_CPUS && CPU_ONLINE[cpu].load(Ordering::Acquire)
}

/// Bit `n` set for every online logical CPU `n`.
#[inline]
pub fn online_mask() -> u64 {
    (0..MAX_CPUS)
        .filter(|&cpu| is_cpu_online(cpu))
        .fold(0, |mask, cpu| mask | (1 << cpu))
}

/// Mark logical CPU `cpu` online. Called once by that CPU, after its per-CPU
/// init and idle-thread registration, with interrupts masked; logical numbers
/// are handed out in order, so `cpu` is always the current online count.
pub fn mark_online(cpu: usize) {
    assert!(
        cpu < MAX_CPUS && cpu as u64 == cpus_online(),
        "CPU {} came online out of order ({} online)",
        cpu,
        cpus_online()
    );
    CPU_ONLINE[cpu].store(true, Ordering::Release);
    CPUS_ONLINE.fetch_add(1, Ordering::Release);
}

/// Record that the processor at MADT entry `index` was started and did not
/// come online.
pub fn note_unanswered(index: usize) {
    if index < 64 {
        UNANSWERED.fetch_or(1 << index, Ordering::AcqRel);
    }
}

/// Record the local APIC id of logical CPU `cpu`. Called by that CPU's own
/// per-CPU init, before it is marked online.
pub fn set_cpu_apic_id(cpu: usize, apic_id: u32) {
    if cpu < MAX_CPUS {
        CPU_APIC_ID[cpu].store(apic_id, Ordering::Release);
    }
}

/// The local APIC id logical CPU `cpu` recorded, if it has run its per-CPU init.
#[inline]
pub fn cpu_apic_id(cpu: usize) -> Option<u32> {
    let id = CPU_APIC_ID.get(cpu)?.load(Ordering::Acquire);
    (id != NO_APIC_ID).then_some(id)
}

/// The logical CPU whose per-CPU init recorded `apic_id`.
///
/// Reads only the fixed array, so it is usable where the GS base cannot be
/// trusted, such as an NMI that interrupted an entry stub.
#[inline]
pub fn cpu_of_apic_id(apic_id: u32) -> Option<usize> {
    (0..MAX_CPUS).find(|&cpu| CPU_APIC_ID[cpu].load(Ordering::Acquire) == apic_id)
}

/// Number of processors this kernel can address, MADT count clamped to
/// `MAX_CPUS`.
#[inline(always)]
pub fn cpus_present() -> u64 {
    CPUS_PRESENT.load(Ordering::Acquire)
}

/// Processor entries the firmware's MADT reported, unclamped.
pub fn madt_cpu_count() -> u32 {
    MADT_CPUS.load(Ordering::Acquire)
}

/// MADT processor entries carrying the enabled flag.
pub fn madt_enabled_count() -> u32 {
    MADT_ENABLED.load(Ordering::Acquire)
}

/// MADT processor entries that were type-9 x2APIC structures.
pub fn madt_x2apic_count() -> u32 {
    MADT_X2APIC.load(Ordering::Acquire)
}

/// The boot processor's APIC id, from CPUID.
pub fn bsp_apic_id() -> u32 {
    BSP_APIC_ID.load(Ordering::Acquire)
}

/// How many APIC ids the enumeration recorded.
pub fn enumerated_cpu_count() -> usize {
    APIC_ID_COUNT.load(Ordering::Acquire) as usize
}

/// The APIC id recorded at `index`, if the enumeration recorded that many.
pub fn apic_id_of(index: usize) -> Option<u32> {
    if index >= enumerated_cpu_count() {
        return None;
    }
    APIC_IDS.get(index).map(|id| id.load(Ordering::Acquire))
}

/// Whether the MADT entry at `index` carries the Enabled flag.
pub fn apic_entry_enabled(index: usize) -> bool {
    index < enumerated_cpu_count()
        && APIC_ENABLED
            .get(index)
            .is_some_and(|enabled| enabled.load(Ordering::Acquire))
}

/// Where the enumeration came from.
pub fn enumeration_source() -> EnumerationSource {
    match SOURCE.load(Ordering::Acquire) {
        SOURCE_MADT => EnumerationSource::Madt,
        SOURCE_CPUID_FALLBACK => EnumerationSource::CpuidFallback,
        _ => EnumerationSource::NotRun,
    }
}

/// Highest standard CPUID leaf this processor answers.
fn cpuid_max_leaf() -> u32 {
    // SAFETY: CPUID leaf 0 is architecturally present on x86_64.
    unsafe { __cpuid(0) }.eax
}

/// Logical processors per package, CPUID leaf 1 EBX[23:16].
///
/// This is a cross-check, not the count anything is derived from: on a
/// processor that reports no HTT support the field is architecturally
/// reserved, and QEMU's `qemu64` model is free to leave it at 1 whatever
/// `-smp` says. It is reported so a MADT count can be read next to it.
fn cpuid_logical_processors() -> u32 {
    if cpuid_max_leaf() < 1 {
        return 0;
    }
    // SAFETY: leaf 1 is present, having been bounded by leaf 0's EAX above.
    let leaf1 = unsafe { __cpuid(1) };
    (leaf1.ebx >> 16) & 0xFF
}

/// The executing processor's APIC id.
///
/// Leaf 0xB subleaf 0 EDX is the full x2APIC id and is preferred when the leaf
/// is implemented (a nonzero EBX is the "this subleaf is valid" signal in the
/// Intel SDM's topology-enumeration algorithm); otherwise leaf 1 EBX[31:24],
/// the 8-bit initial APIC id, answers.
fn cpuid_bsp_apic_id() -> u32 {
    let max_leaf = cpuid_max_leaf();
    if max_leaf >= 0xB {
        // SAFETY: leaf 0xB is present, having been bounded by leaf 0's EAX.
        let topology = unsafe { __cpuid_count(0xB, 0) };
        if topology.ebx != 0 {
            return topology.edx;
        }
    }
    if max_leaf < 1 {
        return 0;
    }
    // SAFETY: leaf 1 is present, having been bounded by leaf 0's EAX above.
    let leaf1 = unsafe { __cpuid(1) };
    (leaf1.ebx >> 24) & 0xFF
}

/// Read the firmware's processor enumeration, publish it, and report it once.
///
/// Called from `kernel_main` after `memory::init` has installed the master
/// kernel page table, because the MADT walk reads physical memory through the
/// bootloader's offset window (see `super::acpi`). This path starts no
/// processor; `super::ap_start::start_application_processors` does.
pub fn init(rsdp_phys: Option<u64>, physical_memory_offset: u64) {
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }

    let bsp_apic_id = cpuid_bsp_apic_id();
    BSP_APIC_ID.store(bsp_apic_id, Ordering::Release);
    let cpuid_logical = cpuid_logical_processors();

    let (madt_cpus, enabled, x2apic, source, reason) =
        match acpi::read_madt(rsdp_phys, physical_memory_offset) {
            Ok(census) => {
                for index in 0..census.recorded {
                    APIC_IDS[index].store(census.apic_ids[index], Ordering::Release);
                    APIC_ENABLED[index].store(census.apic_enabled[index], Ordering::Release);
                }
                APIC_ID_COUNT.store(census.recorded as u32, Ordering::Release);
                (
                    census.processor_entries,
                    census.enabled_entries,
                    census.x2apic_entries,
                    SOURCE_MADT,
                    "none",
                )
            }
            Err(refusal) => (
                0,
                0,
                0,
                SOURCE_CPUID_FALLBACK,
                acpi::refusal_token(refusal),
            ),
        };

    MADT_CPUS.store(madt_cpus, Ordering::Release);
    MADT_ENABLED.store(enabled, Ordering::Release);
    MADT_X2APIC.store(x2apic, Ordering::Release);
    SOURCE.store(source, Ordering::Release);

    let present = (madt_cpus as usize).clamp(1, MAX_CPUS);
    CPUS_PRESENT.store(present as u64, Ordering::Release);

    // The one emission site for this marker. `tests/x86_smp_enum_structure.rs`
    // pins that there is exactly one, and
    // `docker/qemu/run-x86-smp-enum-gate.sh` pins its shape across
    // `-smp 1`, `-smp 2` and `-smp 4`.
    log::info!(
        "[X86_SMP_ENUM:madt_cpus={}:enabled={}:x2apic={}:bsp_apic_id={}:cpuid_logical={}:present={}:online={}:max_cpus={}:src={}:reason={}]",
        madt_cpus,
        enabled,
        x2apic,
        bsp_apic_id,
        cpuid_logical,
        present,
        cpus_online(),
        MAX_CPUS,
        if source == SOURCE_MADT {
            "madt"
        } else {
            "cpuid_fallback"
        },
        reason,
    );
}

/// Report the SMP measurement once secondary bring-up is complete: how many
/// CPUs the firmware's MADT reports enabled and how many are online, and one
/// line for each started processor that did not come online.
///
/// The second line is the boot-path stage, printed only when more than one
/// CPU is reported and every one of them is online. A refused MADT walk
/// reports 0: the CPUID cross-check is not an enumeration.
pub fn report_bring_up() {
    let online = cpus_online();
    let reported = u64::from(madt_enabled_count());
    let unanswered = UNANSWERED.load(Ordering::Acquire);
    for index in (0..64).filter(|index| unanswered & (1 << index) != 0) {
        log::warn!(
            "[smp] CPU with APIC id {} did not come online",
            apic_id_of(index).unwrap_or(u32::MAX)
        );
    }
    log::info!(
        "[smp] online={} reported={} source={}",
        online,
        reported,
        match enumeration_source() {
            EnumerationSource::Madt => "madt",
            EnumerationSource::CpuidFallback => "none",
            EnumerationSource::NotRun => "not-run",
        }
    );
    if reported > 1 && online == reported {
        log::info!(
            "[smp] every reported CPU is online ({} of {})",
            online,
            reported
        );
    }
}
