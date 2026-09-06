//! x86_64 processor enumeration, and the CPU counts answered from it.
//!
//! ## What this module does, and what it does not
//!
//! It records what the firmware's MADT reports about the processors on this
//! machine (via `super::acpi`), cross-checks that against CPUID, and answers
//! `cpus_online()` / `cpus_present()` from atomics instead of from a
//! compile-time constant. That is the whole of it.
//!
//! It starts no processor. No INIT/SIPI sequence is sent, no APIC register is
//! written, no trampoline exists, and `CPUS_ONLINE` is seeded at 1 and moved
//! by nothing in this module. On a `-smp N` boot the other N-1 processors stay
//! where the firmware left them: OVMF starts them during its own init and
//! parks them, and this kernel never addresses them again. `online=1` in the
//! marker below is that fact, reported rather than assumed.
//!
//! ## Two different numbers
//!
//! `madt_cpu_count()` is what the firmware reports — it tracks `-smp N`.
//! `cpus_present()` is that count clamped into what this kernel's per-CPU
//! state can address, which is `crate::task::scheduler::MAX_CPUS` (1 on x86
//! until the scheduler gains per-CPU descriptor tables, stacks and dispatch).
//! Keeping them separate is what lets the enumeration be honest while
//! placement behaviour stays exactly where it was: `online_cpu_count()` reads
//! `cpus_online()`, which is 1, clamped by the same `MAX_CPUS`.
//!
//! The aarch64 counterpart is `crate::arch_impl::aarch64::smp`, whose
//! `CPUS_ONLINE`/`CPU_ONLINE`/`cpus_online()` shape this mirrors. See
//! #814 for the staged plan and #629 for the count-from-a-constant defect
//! this addresses the reporting half of.

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
/// able to dispatch. Seeded at 1 for the boot processor, and moved by nothing
/// in this PR — no AP is started.
static CPUS_ONLINE: AtomicU64 = AtomicU64::new(1);

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

/// How many of `APIC_IDS` are populated.
static APIC_ID_COUNT: AtomicU32 = AtomicU32::new(0);

/// The boot processor's own APIC id, read from CPUID rather than from the
/// MADT: it is the id of the processor executing this code.
static BSP_APIC_ID: AtomicU32 = AtomicU32::new(0);

/// One of the `SOURCE_*` codes above.
static SOURCE: AtomicU32 = AtomicU32::new(SOURCE_NOT_RUN);

/// Set by the first `init()`, so a second call cannot emit a second marker.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Number of processors online. Mirrors
/// `crate::arch_impl::aarch64::smp::cpus_online()`.
#[inline(always)]
pub fn cpus_online() -> u64 {
    CPUS_ONLINE.load(Ordering::Acquire)
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

/// The APIC id recorded at `index`, or `None` past what was recorded.
pub fn apic_id_of(index: usize) -> Option<u32> {
    if index >= enumerated_cpu_count() {
        return None;
    }
    APIC_IDS.get(index).map(|id| id.load(Ordering::Acquire))
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
    // SAFETY: CPUID leaf 0 is architecturally present on every x86_64.
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
/// bootloader's offset window (see `super::acpi`). Nothing here starts a
/// processor.
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
