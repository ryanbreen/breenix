#![cfg(target_arch = "x86_64")]

use crate::task::scheduler::MAX_CPUS;
use conquer_once::spin::OnceCell;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST_INDEX: u16 = 1;
/// NMI runs on its own IST stack: a TLB-shootdown NMI can arrive between a
/// `syscall` instruction and the entry stub's switch off the user stack.
pub const NMI_IST_INDEX: u16 = 2;

/// One TSS per logical CPU: its own RSP0 and its own IST stacks.
///
/// `TaskStateSegment::new()` leaves `iomap_base` past the segment limit, which
/// disables the I/O permission bitmap. That keeps port I/O from faulting after
/// a CR3 switch to a page table where a bitmap would not be mapped.
static mut TSS: [TaskStateSegment; MAX_CPUS] = [const { TaskStateSegment::new() }; MAX_CPUS];

/// One GDT per logical CPU, each holding that CPU's TSS descriptor.
static mut GDT: [GlobalDescriptorTable; MAX_CPUS] =
    [const { GlobalDescriptorTable::new() }; MAX_CPUS];

/// The selectors every CPU's GDT produces. Each GDT is built by the same
/// sequence of appends, so these are identical across CPUs.
static SELECTORS: OnceCell<Selectors> = OnceCell::uninit();

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

/// Raw pointer to logical CPU `cpu`'s TSS. Panics past `MAX_CPUS`.
pub fn tss_ptr(cpu: usize) -> *mut TaskStateSegment {
    assert!(cpu < MAX_CPUS, "no TSS for CPU {}", cpu);
    unsafe { &raw mut TSS[cpu] }
}

/// Build logical CPU `cpu`'s GDT around its own TSS, load it, reload the
/// kernel segment registers and load the task register.
///
/// Part of the per-CPU init every CPU runs. The GDT is rebuilt on every call,
/// which also writes the TSS descriptor back as available, so a second call on
/// the same CPU (the boot processor loads early, then again from its per-CPU
/// init) does not fault on `ltr` of a busy TSS. The kernel descriptors are
/// rewritten with the bytes they already held.
pub fn load(cpu: usize) {
    use x86_64::instructions::segmentation::{Segment, CS, DS, SS};
    use x86_64::instructions::tables::load_tss;

    let tss: &'static TaskStateSegment = unsafe { &*tss_ptr(cpu) };
    let mut gdt = GlobalDescriptorTable::new();
    let code_selector = gdt.append(Descriptor::kernel_code_segment());
    let data_selector = gdt.append(Descriptor::kernel_data_segment());
    let tss_selector = gdt.append(Descriptor::tss_segment(tss));
    let user_data_selector = gdt.append(Descriptor::user_data_segment());
    let user_code_selector = gdt.append(Descriptor::user_code_segment());

    let gdt: &'static GlobalDescriptorTable = unsafe {
        let slot = &raw mut GDT[cpu];
        *slot = gdt;
        &*slot
    };
    gdt.load();

    let selectors = SELECTORS.get_or_init(|| Selectors {
        code_selector,
        tss_selector,
        data_selector,
        user_code_selector,
        user_data_selector,
    });
    assert!(
        selectors.code_selector == code_selector
            && selectors.data_selector == data_selector
            && selectors.tss_selector == tss_selector
            && selectors.user_code_selector == user_code_selector
            && selectors.user_data_selector == user_data_selector,
        "CPU {} GDT selectors differ from the boot processor's",
        cpu
    );

    unsafe {
        CS::set_reg(code_selector);
        DS::set_reg(data_selector);
        SS::set_reg(data_selector);
        load_tss(tss_selector);
    }
}

/// Load the boot processor's GDT and TSS, before memory is up.
pub fn init() {
    load(0);

    let tss_addr = tss_ptr(0) as u64;
    log::info!(
        "TSS I/O permission bitmap disabled (iomap_base={})",
        unsafe { (*tss_ptr(0)).iomap_base }
    );
    log::info!(
        "TSS located at {:#x} (PML4 index {})",
        tss_addr,
        (tss_addr >> 39) & 0x1FF
    );

    let selectors = SELECTORS.get().expect("GDT selectors recorded by load()");

    // Log GDT address for debugging CR3 switch issues
    use x86_64::instructions::tables::sgdt;
    let gdtr = sgdt();
    log::info!(
        "GDT loaded at {:#x} (PML4 index {})",
        gdtr.base.as_u64(),
        (gdtr.base.as_u64() >> 39) & 0x1FF
    );

    log::info!("GDT initialized with kernel and user segments");
    log::debug!("  Kernel code: {:#x}", selectors.code_selector.0);
    log::debug!("  Kernel data: {:#x}", selectors.data_selector.0);
    log::debug!("  TSS: {:#x}", selectors.tss_selector.0);
    log::debug!("  User data: {:#x}", selectors.user_data_selector.0);
    log::debug!("  User code: {:#x}", selectors.user_code_selector.0);

    // Dump raw GDT descriptors for debugging
    unsafe {
        let gdtr = x86_64::instructions::tables::sgdt();
        log::debug!(
            "GDT base: {:#x}, limit: {:#x}",
            gdtr.base.as_u64(),
            gdtr.limit
        );

        // Dump user segment descriptors
        let gdt_base = gdtr.base.as_ptr::<u64>();
        let user_data_desc = *gdt_base.offset(5); // Index 5
        let user_code_desc = *gdt_base.offset(6); // Index 6

        log::debug!("Raw user data descriptor (0x2b): {:#018x}", user_data_desc);
        log::debug!("Raw user code descriptor (0x33): {:#018x}", user_code_desc);

        // Decode user data descriptor
        let present = (user_data_desc >> 47) & 1;
        let dpl = (user_data_desc >> 45) & 3;
        let s_bit = (user_data_desc >> 44) & 1;
        let type_field = (user_data_desc >> 40) & 0xF;
        log::debug!(
            "  User data: P={} DPL={} S={} Type={:#x}",
            present,
            dpl,
            s_bit,
            type_field
        );

        // Decode user code descriptor
        let present = (user_code_desc >> 47) & 1;
        let dpl = (user_code_desc >> 45) & 3;
        let s_bit = (user_code_desc >> 44) & 1;
        let type_field = (user_code_desc >> 40) & 0xF;
        let l_bit = (user_code_desc >> 53) & 1;
        let d_bit = (user_code_desc >> 54) & 1;
        log::debug!(
            "  User code: P={} DPL={} S={} Type={:#x} L={} D={}",
            present,
            dpl,
            s_bit,
            type_field,
            l_bit,
            d_bit
        );
    }

    // Log TSS setup
    let tss = unsafe { &*tss_ptr(0) };
    let rsp0 = tss.privilege_stack_table[0];
    let ist0 = tss.interrupt_stack_table[0];
    log::debug!("  TSS RSP0 (kernel stack): {:#x}", rsp0);
    log::debug!("  TSS IST[0] (double fault stack): {:#x}", ist0);
}

pub fn user_code_selector() -> SegmentSelector {
    SELECTORS
        .get()
        .expect("GDT not initialized")
        .user_code_selector
}

pub fn user_data_selector() -> SegmentSelector {
    SELECTORS
        .get()
        .expect("GDT not initialized")
        .user_data_selector
}

pub fn kernel_code_selector() -> SegmentSelector {
    SELECTORS.get().expect("GDT not initialized").code_selector
}

pub fn kernel_data_selector() -> SegmentSelector {
    SELECTORS.get().expect("GDT not initialized").data_selector
}

/// The executing CPU's TSS.
///
/// Read from the TSS pointer in this CPU's per-CPU data, which its per-CPU
/// init stores. Before per-CPU data exists only the boot processor runs, so
/// its slot answers.
pub fn get_tss_ptr() -> *mut TaskStateSegment {
    if crate::per_cpu::is_initialized() {
        let tss = crate::per_cpu::tss_ptr();
        if !tss.is_null() {
            return tss;
        }
    }
    tss_ptr(0)
}

#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn double_fault_stack_top() -> VirtAddr {
    unsafe { (*get_tss_ptr()).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] }
}

/// Point logical CPU `cpu`'s IST entries at its own emergency stacks.
///
/// Part of the per-CPU init every CPU runs, after `memory::init` has mapped the
/// per-CPU stack region.
pub fn install_ist_stacks(cpu: usize) {
    use crate::memory::per_cpu_stack;

    let tss = tss_ptr(cpu);
    unsafe {
        (*tss).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            per_cpu_stack::emergency_stack(cpu);
        (*tss).interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] =
            per_cpu_stack::page_fault_stack(cpu);
        (*tss).interrupt_stack_table[NMI_IST_INDEX as usize] = per_cpu_stack::nmi_stack(cpu);
    }
}

/// Get the executing CPU's TSS RSP0 value for debugging
pub fn get_tss_rsp0() -> u64 {
    unsafe { (*get_tss_ptr()).privilege_stack_table[0].as_u64() }
}

/// Set the executing CPU's TSS.RSP0.
///
/// Called on the syscall path, so it reads the TSS pointer straight from this
/// CPU's per-CPU data: one GS-relative load, no lock.
pub fn set_tss_rsp0(kernel_stack_top: VirtAddr) {
    let tss = crate::per_cpu::tss_ptr();
    if !tss.is_null() {
        unsafe {
            (*tss).privilege_stack_table[0] = kernel_stack_top;
        }
    }
}

/// Get GDT base and limit for logging
pub fn get_gdt_info() -> (u64, u16) {
    let gdtr = x86_64::instructions::tables::sgdt();
    (gdtr.base.as_u64(), gdtr.limit)
}

/// Get the executing CPU's TSS base address and RSP0 for logging
pub fn get_tss_info() -> (u64, u64) {
    let tss_ptr = get_tss_ptr();
    let rsp0 = unsafe { (*tss_ptr).privilege_stack_table[0].as_u64() };
    (tss_ptr as u64, rsp0)
}
