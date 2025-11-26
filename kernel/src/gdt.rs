use conquer_once::spin::OnceCell;
use core::sync::atomic::{AtomicPtr, Ordering};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::{PrivilegeLevel, VirtAddr};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST_INDEX: u16 = 1;

static TSS: OnceCell<TaskStateSegment> = OnceCell::uninit();
static GDT: OnceCell<(GlobalDescriptorTable, Selectors)> = OnceCell::uninit();
static TSS_PTR: AtomicPtr<TaskStateSegment> = AtomicPtr::new(core::ptr::null_mut());

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

// Export user segment selectors for context switching
// These will be initialized dynamically when GDT is created
pub static mut USER_CODE_SELECTOR: SegmentSelector = SegmentSelector::new(0, PrivilegeLevel::Ring0);
pub static mut USER_DATA_SELECTOR: SegmentSelector = SegmentSelector::new(0, PrivilegeLevel::Ring0);

pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS};
    use x86_64::instructions::tables::load_tss;

    TSS.init_once(|| {
        let mut tss = TaskStateSegment::new();

        // Set up IST stacks using per-CPU emergency stacks
        // These will be properly initialized after memory system is up
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(0);
        tss.interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = VirtAddr::new(0);
        // Note: We'll update these later with update_ist_stacks()

        // CRITICAL FIX: Don't set RSP0 to a bootstrap stack here
        // It will be set to a proper kernel stack from the upper half
        // when the memory system is initialized
        tss.privilege_stack_table[0] = VirtAddr::new(0);
        
        // Note: RSP0 will be updated by update_tss_rsp0() after kernel stack allocation

        // CRITICAL FIX: Disable I/O permission bitmap to prevent GP faults during CR3 switches
        // Setting iomap_base beyond the TSS limit effectively disables per-port I/O checks
        // This prevents GP faults when executing OUT instructions after CR3 switch to user page table
        // where the TSS I/O bitmap might not be mapped
        tss.iomap_base = core::mem::size_of::<TaskStateSegment>() as u16;
        
        log::info!("TSS I/O permission bitmap disabled (iomap_base={})", tss.iomap_base);

        tss
    });

    // Store a pointer to the TSS for later updates
    let tss_ref = TSS.get().unwrap();
    TSS_PTR.store(tss_ref as *const _ as *mut _, Ordering::Release);
    
    // Log TSS address for debugging CR3 switch issues
    let tss_addr = tss_ref as *const _ as u64;
    log::info!("TSS located at {:#x} (PML4 index {})", tss_addr, (tss_addr >> 39) & 0x1FF);

    GDT.init_once(|| {
        let mut gdt = GlobalDescriptorTable::new();

        // Kernel segments
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let data_selector = gdt.append(Descriptor::kernel_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS.get().unwrap()));

        // User segments (Ring 3)
        let user_data_selector = gdt.append(Descriptor::user_data_segment());
        let user_code_selector = gdt.append(Descriptor::user_code_segment());

        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
                data_selector,
                user_code_selector,
                user_data_selector,
            },
        )
    });

    let (gdt, selectors) = GDT.get().unwrap();

    gdt.load();
    
    // Log GDT address for debugging CR3 switch issues
    use x86_64::instructions::tables::sgdt;
    let gdtr = sgdt();
    log::info!("GDT loaded at {:#x} (PML4 index {})", gdtr.base.as_u64(), (gdtr.base.as_u64() >> 39) & 0x1FF);
    unsafe {
        CS::set_reg(selectors.code_selector);
        DS::set_reg(selectors.data_selector);
        load_tss(selectors.tss_selector);
    }

    // Store user segment selectors for context switching
    unsafe {
        USER_CODE_SELECTOR = selectors.user_code_selector;
        USER_DATA_SELECTOR = selectors.user_data_selector;
    }

    log::info!("GDT initialized with kernel and user segments");
    log::debug!("  Kernel code: {:#x}", selectors.code_selector.0);
    log::debug!("  Kernel data: {:#x}", selectors.data_selector.0);
    log::debug!("  TSS: {:#x}", selectors.tss_selector.0);
    log::debug!("  User data: {:#x}", selectors.user_data_selector.0);
    log::debug!("  User code: {:#x}", selectors.user_code_selector.0);
    
    // Dump raw GDT descriptors for debugging
    unsafe {
        let gdtr = x86_64::instructions::tables::sgdt();
        log::debug!("GDT base: {:#x}, limit: {:#x}", gdtr.base.as_u64(), gdtr.limit);
        
        // Dump user segment descriptors
        let gdt_base = gdtr.base.as_ptr::<u64>();
        let user_data_desc = *gdt_base.offset(5);  // Index 5
        let user_code_desc = *gdt_base.offset(6);  // Index 6
        
        log::debug!("Raw user data descriptor (0x2b): {:#018x}", user_data_desc);
        log::debug!("Raw user code descriptor (0x33): {:#018x}", user_code_desc);
        
        // Decode user data descriptor
        let present = (user_data_desc >> 47) & 1;
        let dpl = (user_data_desc >> 45) & 3;
        let s_bit = (user_data_desc >> 44) & 1;
        let type_field = (user_data_desc >> 40) & 0xF;
        log::debug!("  User data: P={} DPL={} S={} Type={:#x}", present, dpl, s_bit, type_field);
        
        // Decode user code descriptor
        let present = (user_code_desc >> 47) & 1;
        let dpl = (user_code_desc >> 45) & 3;
        let s_bit = (user_code_desc >> 44) & 1;
        let type_field = (user_code_desc >> 40) & 0xF;
        let l_bit = (user_code_desc >> 53) & 1;
        let d_bit = (user_code_desc >> 54) & 1;
        log::debug!("  User code: P={} DPL={} S={} Type={:#x} L={} D={}", 
            present, dpl, s_bit, type_field, l_bit, d_bit);
    }

    // Log TSS setup
    let tss = TSS.get().unwrap();
    let rsp0 = tss.privilege_stack_table[0];
    let ist0 = tss.interrupt_stack_table[0];
    log::debug!("  TSS RSP0 (kernel stack): {:#x}", rsp0);
    log::debug!("  TSS IST[0] (double fault stack): {:#x}", ist0);
}

pub fn user_code_selector() -> SegmentSelector {
    GDT.get().expect("GDT not initialized").1.user_code_selector
}

pub fn user_data_selector() -> SegmentSelector {
    GDT.get().expect("GDT not initialized").1.user_data_selector
}

pub fn kernel_code_selector() -> SegmentSelector {
    GDT.get().expect("GDT not initialized").1.code_selector
}

pub fn kernel_data_selector() -> SegmentSelector {
    GDT.get().expect("GDT not initialized").1.data_selector
}

/// Get the TSS pointer for per-CPU data
pub fn get_tss_ptr() -> *mut TaskStateSegment {
    TSS_PTR.load(Ordering::Acquire)
}

pub fn set_kernel_stack(stack_top: VirtAddr) {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        unsafe {
            let old_stack = (*tss_ptr).privilege_stack_table[0];
            (*tss_ptr).privilege_stack_table[0] = stack_top;
            crate::serial_println!(
                "TSS RSP0 updated: {:#x} -> {:#x}",
                old_stack.as_u64(),
                stack_top.as_u64()
            );
        }
    } else {
        panic!("TSS not initialized");
    }
}

#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn double_fault_stack_top() -> VirtAddr {
    TSS.get()
        .expect("TSS not initialized")
        .interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize]
}

/// Update the IST stacks with per-CPU emergency stacks
/// This should be called after the memory system is initialized
pub fn update_ist_stacks() {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        // Get both IST stack addresses
        let emergency_stack = crate::memory::per_cpu_stack::current_cpu_emergency_stack();
        let page_fault_stack = crate::memory::per_cpu_stack::current_cpu_page_fault_stack();
        
        unsafe {
            // Set up double fault IST
            (*tss_ptr).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = emergency_stack;
            log::info!(
                "Updated IST[{}] (double fault stack) to {:#x}",
                DOUBLE_FAULT_IST_INDEX,
                emergency_stack.as_u64()
            );
            
            // Set up page fault IST
            (*tss_ptr).interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = page_fault_stack;
            log::info!(
                "Updated IST[{}] (page fault stack) to {:#x}",
                PAGE_FAULT_IST_INDEX,
                page_fault_stack.as_u64()
            );
        }
    } else {
        panic!("TSS not initialized");
    }
}

/// Legacy function - now calls update_ist_stacks()
#[allow(dead_code)]
pub fn update_ist_stack(stack_top: VirtAddr) {
    let _ = stack_top; // Ignore parameter, use proper per-CPU stacks
    update_ist_stacks();
}

/// Get the current TSS RSP0 value for debugging
pub fn get_tss_rsp0() -> u64 {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        unsafe { (*tss_ptr).privilege_stack_table[0].as_u64() }
    } else {
        0
    }
}

/// Set TSS.RSP0 directly (for testing/debugging)
pub fn set_tss_rsp0(kernel_stack_top: VirtAddr) {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        unsafe {
            (*tss_ptr).privilege_stack_table[0] = kernel_stack_top;
        }
    }
}

/// Get GDT base and limit for logging
pub fn get_gdt_info() -> (u64, u16) {
    let gdtr = x86_64::instructions::tables::sgdt();
    (gdtr.base.as_u64(), gdtr.limit)
}

/// Get TSS base address and RSP0 for logging
pub fn get_tss_info() -> (u64, u64) {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        let base = tss_ptr as u64;
        let rsp0 = unsafe { (*tss_ptr).privilege_stack_table[0].as_u64() };
        (base, rsp0)
    } else {
        (0, 0)
    }
}
