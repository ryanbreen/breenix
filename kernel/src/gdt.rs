use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use conquer_once::spin::OnceCell;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

static TSS: OnceCell<TaskStateSegment> = OnceCell::uninit();
static GDT: OnceCell<(GlobalDescriptorTable, Selectors)> = OnceCell::uninit();

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    data_selector: SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, Segment};
    use x86_64::instructions::tables::load_tss;

    TSS.init_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 8192; // 8KB stack (improved from 4KB)
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            let stack_end = stack_start + STACK_SIZE as u64;
            stack_end
        };
        tss
    });

    GDT.init_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        
        // Kernel segments
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let data_selector = gdt.append(Descriptor::kernel_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS.get().unwrap()));
        
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
                data_selector,
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
    
    log::info!("GDT initialized with kernel segments");
}

#[cfg(feature = "testing")]
pub fn double_fault_stack_top() -> VirtAddr {
    TSS.get()
        .expect("TSS not initialized")
        .interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize]
}

