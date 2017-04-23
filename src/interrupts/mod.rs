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

use x86_64::structures::idt::{Idt, ExceptionStackFrame, PageFaultErrorCode};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtualAddress;

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<gdt::Gdt> = Once::new();

const DOUBLE_FAULT_IST_INDEX: usize = 0;

static mut test_passed: bool = false;

pub unsafe fn test_interrupt() {
    int!(SYSCALL_INTERRUPT);
    test_passed = true;
}

lazy_static! {
    static ref IDT: Idt = {
        let mut idt = Idt::new();
        idt.divide_by_zero.set_handler_fn(divide_by_zero_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX as u16);
        }

        for i in 0..256-32 {
            idt.interrupts[i].set_handler_fn(dummy_error_handler);
        }

        idt.interrupts[(SERIAL_INTERRUPT - 32) as usize].set_handler_fn(serial_handler);
        idt.interrupts[(SYSCALL_INTERRUPT - 32) as usize].set_handler_fn(syscall_handler);
        idt.interrupts[(TIMER_INTERRUPT - 32) as usize].set_handler_fn(timer_handler);
        idt.interrupts[(KEYBOARD_INTERRUPT - 32) as usize].set_handler_fn(keyboard_handler);

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

        PICS.lock().initialize();

        test_interrupt();

        if test_passed {

            println!("Enabling irqs");
            //use x86_64::instructions::interrupts;
            //interrupts::enable();
            irq::enable();
            println!("Enabled irqs");
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

extern "x86-interrupt" fn syscall_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("SYSCALL:\n{:#?}", stack_frame);

    ::state().interrupt_count[SYSCALL_INTERRUPT as usize] += 1;

    unsafe {
        PICS.lock().notify_end_of_interrupt(SYSCALL_INTERRUPT);
    }
}

extern "x86-interrupt" fn timer_handler(stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[TIMER_INTERRUPT as usize] += 1;

    timer::timer_interrupt();

    unsafe {
        PICS.lock().notify_end_of_interrupt(TIMER_INTERRUPT);
    }
}

extern "x86-interrupt" fn keyboard_handler(stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[KEYBOARD_INTERRUPT as usize] += 1;

    keyboard::read();

    unsafe {
        PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT);
    }
}

extern "x86-interrupt" fn serial_handler(stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[SERIAL_INTERRUPT as usize] += 1;

    println!("serial read\n{:#?}", stack_frame);
    serial::read();

    unsafe {
        PICS.lock().notify_end_of_interrupt(SERIAL_INTERRUPT);
    }
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: &mut ExceptionStackFrame, error_code: PageFaultErrorCode) {
    use x86_64::registers::control_regs;
    println!("\nEXCEPTION: PAGE FAULT while accessing {:#x}\nerror code: \
                                  {:?}\n{:#?}",
             control_regs::cr2(),
             error_code,
             stack_frame);
    loop {}
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });
