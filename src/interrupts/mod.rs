mod gdt;

use buffers::print_error;

use constants::keyboard::KEYBOARD_INTERRUPT;
use constants::serial::SERIAL_INTERRUPT;
use constants::syscall::SYSCALL_INTERRUPT;
use constants::timer::TIMER_INTERRUPT;
use io::{keyboard, serial, timer, ChainedPics};
use memory::MemoryController;

use spin::Mutex;

use x86::shared::irq;

use spin::Once;

use util::syscall::syscall0;

use x86_64::structures::idt::ExceptionStackFrame;
use x86_64::structures::idt::Idt;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtualAddress;

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<gdt::Gdt> = Once::new();

const DOUBLE_FAULT_IST_INDEX: usize = 0;

static mut test_passed: bool = false;

/*
extern "C" fn interrupt_handler(stack_frame: &mut ExceptionStackFrame, int_id: u64) {
    ::state().interrupt_count[int_id as usize] += 1;

    match int_id as u8 {
        //0x00...0x0F => println!("error: {}", int_id),
        TIMER_INTERRUPT => {
            timer::timer_interrupt();
        }
        KEYBOARD_INTERRUPT => {
            keyboard::read();
        }
        SERIAL_INTERRUPT => {
            println!("serial read\n{:#?}", stack_frame);
            serial::read();
        }
        // On Linux, this is used for syscalls.  Good enough for me.
        SYSCALL_INTERRUPT => {
            println!("Got syscall\n{:#?}", stack_frame);
            syscall_handler(stack_frame);
            // Handle syscall
        }
        _ => {
            ::state().interrupt_count[0 as usize] += 1;
        }
    }

    unsafe {
        PICS.lock().notify_end_of_interrupt(int_id as u8);
    }
}
*/

pub unsafe fn test_interrupt() {
    int!(SYSCALL_INTERRUPT);
    test_passed = true;
}

lazy_static! {
    static ref IDT: Idt = {
        let mut idt = Idt::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX as u16);
        }
        idt.interrupts[(SYSCALL_INTERRUPT - 32) as usize].set_handler_fn(syscall_handler);

        idt
    };
}

#[allow(dead_code)]
pub fn init(memory_controller: &mut MemoryController) {

    use x86_64::structures::gdt::SegmentSelector;
    use x86_64::instructions::segmentation::set_cs;
    use x86_64::instructions::tables::load_tss;

    let double_fault_stack = memory_controller.alloc_stack(1)
        .expect("could not allocate double fault stack");

    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX] = VirtualAddress(
            double_fault_stack.top());
        tss
    });

    let mut code_selector = SegmentSelector(0);
    let mut tss_selector = SegmentSelector(0);
    let gdt = GDT.call_once(|| {
        let mut gdt = gdt::Gdt::new();
        code_selector = gdt.add_entry(gdt::Descriptor::kernel_code_segment());
        tss_selector = gdt.add_entry(gdt::Descriptor::tss_segment(&tss));
        gdt
    });
    gdt.load();

     unsafe {
        // reload code segment register
        set_cs(code_selector);
        // load TSS
        load_tss(tss_selector);
    }

    IDT.load();

    unsafe {

        // PICS.lock().initialize();

        test_interrupt();

        if test_passed {

            println!("Enabling irqs");
            irq::enable();
            println!("Enabled irqs");

            //let ret:u64 = syscall!(69, 12, 13);
            //println!("Got return value {}", ret);
        }
    }
}

extern "x86-interrupt" fn dummy_error_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("\nEXCEPTION: UNHANDLED at {:#x}\n{:#?}",
        stack_frame.instruction_pointer, stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: &mut ExceptionStackFrame) {
    println!("\nEXCEPTION: BREAKPOINT at {:#x}\n{:#?}",
             stack_frame.instruction_pointer,
             stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: &mut ExceptionStackFrame,
    _error_code: u64)
{
    println!("\nEXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn syscall_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("SYSCALL:\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: &mut ExceptionStackFrame)
{
    unsafe {
        print_error(format_args!("EXCEPTION: DIVIDE BY ZERO\n{:#?}",
            stack_frame));
        loop {}
    };
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: &mut ExceptionStackFrame)
{
    unsafe {
        print_error(format_args!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
            stack_frame.instruction_pointer, stack_frame));
        loop {}
    }
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: &mut ExceptionStackFrame,
                                 error_code: u64)
{
    use x86::shared::control_regs;
    unsafe {
        print_error(format_args!(
            "EXCEPTION: PAGE FAULT while accessing {:#x}\
            \nerror code: {:?}\n{:#?}",
            control_regs::cr2(),
            PageFaultErrorCode::from_bits(error_code).unwrap(),
            stack_frame));
    }
}

bitflags! {
    flags PageFaultErrorCode: u64 {
        const PROTECTION_VIOLATION = 1 << 0,
        const CAUSED_BY_WRITE = 1 << 1,
        const USER_MODE = 1 << 2,
        const MALFORMED_TABLE = 1 << 3,
        const INSTRUCTION_FETCH = 1 << 4,
    }
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });
