use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::{VirtAddr, PrivilegeLevel};
use conquer_once::spin::OnceCell;
use core::sync::atomic::{AtomicPtr, Ordering};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const DEBUG_DF_IST_INDEX: u16 = 1;  // For Ring 3 timer debugging

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
    use x86_64::instructions::segmentation::{CS, DS, Segment};
    use x86_64::instructions::tables::load_tss;

    TSS.init_once(|| {
        let mut tss = TaskStateSegment::new();
        
        // Set up double fault stack using per-CPU emergency stack
        // This will be properly initialized after memory system is up
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(0);
        // Note: We'll update this later with update_ist_stack()
        
        // Set up debug double fault stack (IST[1]) with static stack
        tss.interrupt_stack_table[DEBUG_DF_IST_INDEX as usize] = {
            const DEBUG_STACK_SIZE: usize = 4096; // 4KB debug stack
            static mut DEBUG_STACK: [u8; DEBUG_STACK_SIZE] = [0; DEBUG_STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(&raw const DEBUG_STACK);
            stack_start + DEBUG_STACK_SIZE as u64
        };
        
        // Set up privilege level 0 (kernel) stack for syscalls/interrupts from userspace
        // Use the legacy RSP0 field for Ring 3 -> Ring 0 transitions
        tss.privilege_stack_table[0] = {
            const STACK_SIZE: usize = 65536; // 64KB kernel stack (DOUBLED to test stack overflow)
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            let stack_end = stack_start + STACK_SIZE as u64;
            stack_end
        };
        
        tss
    });
    
    // Store a pointer to the TSS for later updates
    let tss_ref = TSS.get().unwrap();
    TSS_PTR.store(tss_ref as *const _ as *mut _, Ordering::Release);

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
    
    // DEBUG: Dump raw GDT entries to verify attributes
    unsafe {
        use x86_64::instructions::tables::sgdt;
        use x86_64::structures::DescriptorTablePointer;
        
        let gdtr: DescriptorTablePointer = sgdt();
        let gdt_ptr = gdtr.base.as_u64() as *const u64;
        
        // Dump user code descriptor (index 6, selector 0x33)
        let user_code_desc = *gdt_ptr.add(6);
        crate::serial_println!("GDT[6] User Code: {:#016x}", user_code_desc);
        
        // Dump user data descriptor (index 5, selector 0x2b)  
        let user_data_desc = *gdt_ptr.add(5);
        crate::serial_println!("GDT[5] User Data: {:#016x}", user_data_desc);
        
        // Decode user code descriptor attributes
        let dpl = (user_code_desc >> 45) & 0x3;
        let p = (user_code_desc >> 47) & 0x1;
        let l = (user_code_desc >> 53) & 0x1;  // 64-bit code flag
        let d = (user_code_desc >> 54) & 0x1;
        crate::serial_println!("  User Code: DPL={} P={} L={} D={}", dpl, p, l, d);
        
        // Decode user data descriptor attributes
        let data_dpl = (user_data_desc >> 45) & 0x3;
        let data_p = (user_data_desc >> 47) & 0x1;
        let data_type = (user_data_desc >> 40) & 0xF;
        crate::serial_println!("  User Data: DPL={} P={} Type={:#x}", data_dpl, data_p, data_type);
    }
    
    // Log TSS setup
    let tss = TSS.get().unwrap();
    let rsp0 = tss.privilege_stack_table[0];
    let ist0 = tss.interrupt_stack_table[0];
    log::debug!("  TSS RSP0 (kernel stack): {:#x}", rsp0);
    log::debug!("  TSS IST[0] (double fault stack): {:#x}", ist0);
}

pub fn user_code_selector() -> SegmentSelector {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .user_code_selector
}

pub fn user_data_selector() -> SegmentSelector {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .user_data_selector
}

pub fn kernel_code_selector() -> SegmentSelector {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .code_selector
}

pub fn kernel_data_selector() -> SegmentSelector {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .data_selector
}

pub fn set_kernel_stack(stack_top: VirtAddr) {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        unsafe {
            let old_stack = (*tss_ptr).privilege_stack_table[0];
            (*tss_ptr).privilege_stack_table[0] = stack_top;
            log::debug!("TSS RSP0 updated: {:#x} -> {:#x}", old_stack.as_u64(), stack_top.as_u64());
        }
    } else {
        panic!("TSS not initialized");
    }
}


/// Update the IST stack with the per-CPU emergency stack
/// This should be called after the memory system is initialized
pub fn update_ist_stack(stack_top: VirtAddr) {
    let tss_ptr = TSS_PTR.load(Ordering::Acquire);
    if !tss_ptr.is_null() {
        unsafe {
            (*tss_ptr).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_top;
            log::info!("Updated IST[0] (double fault stack) to {:#x}", stack_top.as_u64());
        }
    } else {
        panic!("TSS not initialized");
    }
}

