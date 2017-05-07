mod gdt;
mod syscall;

use constants::keyboard::KEYBOARD_INTERRUPT;
use constants::serial::SERIAL_INTERRUPT;
use constants::syscall::SYSCALL_INTERRUPT;
use constants::timer::TIMER_INTERRUPT;

use core::fmt;

use io::{keyboard, serial, timer, ChainedPics};
use memory;
use memory::MemoryController;

use spin::Mutex;

use x86::shared::irq;

use spin::Once;

use x86_64::structures::idt::{Idt, ExceptionStackFrame, PageFaultErrorCode};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtualAddress;

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<gdt::Gdt> = Once::new();

const DOUBLE_FAULT_IST_INDEX: usize = 0;

static mut test_passed: bool = false;

#[repr(C, packed)]
struct InterruptContext {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
 }

impl fmt::Debug for InterruptContext {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {

        write!(f, "\tr15: {} 0x{:x}\n", self.r15, self.r15);
        write!(f, "\tr14: {} 0x{:x}\n", self.r14, self.r14);
        write!(f, "\tr13: {} 0x{:x}\n", self.r13, self.r13);
        write!(f, "\tr12: {} 0x{:x}\n", self.r12, self.r12);
        write!(f, "\tr11: {} 0x{:x}\n", self.r11, self.r11);
        write!(f, "\trbx: {} 0x{:x}\n", self.rbx, self.rbx);
        write!(f, "\trcx: {} 0x{:x}\n", self.rcx, self.rcx);

        write!(f, "\trax: {} 0x{:x}\n", self.rax, self.rax);
        write!(f, "\trdi: {} 0x{:x}\n", self.rdi, self.rdi);
        write!(f, "\trsi: {} 0x{:x}\n", self.rsi, self.rsi);
        write!(f, "\trdx: {} 0x{:x}\n", self.rdi, self.rdi);
        write!(f, "\tr10: {} 0x{:x}\n", self.r10, self.r10);
        write!(f, "\tr8: {} 0x{:x}\n", self.r8, self.r8);
        write!(f, "\tr9: {} 0x{:x}\n", self.r9, self.r9)
    }
}


pub unsafe fn test_interrupt() {
    use util::syscall;
    let res = syscall::syscall6(16, 32, 64, 128, 256, 512, 1024);
    println!("Syscall result is {}", res);
    test_passed = res == 2016;
    if !test_passed {
        panic!("test SYSCALL failed");
    }
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
pub fn init() {

    use x86_64::structures::gdt::SegmentSelector;
    use x86_64::instructions::segmentation::set_cs;
    use x86_64::instructions::tables::load_tss;

    let double_fault_stack = memory::memory_controller().alloc_stack(1)
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
            irq::enable();
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
    println!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: &mut ExceptionStackFrame)
{
    unsafe {
        println!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
            stack_frame.instruction_pointer, stack_frame);
        loop {}
    }
}

extern "x86-interrupt" fn syscall_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("SYSCALL:\n{:#?}", stack_frame);

    ::state().interrupt_count[SYSCALL_INTERRUPT as usize] += 1;

    // Write output to register rax
    unsafe {
        let sp = stack_frame.stack_pointer.0 - 160;

        ::state().scheduler.update_trap_frame(sp);
        //println!("Syscall rsp is {:x}", sp);

        let ref ic:InterruptContext = *(sp as * const InterruptContext);
        //println!("Syscall IC at offset is\n{:?}", ic);

        let num = ic.rax;
        let a = ic.rdi;
        let b = ic.rsi;
        let c = ic.rdx;
        let d = ic.r10;
        let e = ic.r8;
        let f = ic.r9;

        println!("syscall params {} {} {} {} {} {} {}", num, a, b, c, d, e, f);

        let res = syscall::handle(num, a, b, c, d, e, f);

        PICS.lock().notify_end_of_interrupt(SYSCALL_INTERRUPT);

        asm!("movq %rsp, %rcx
              movq $0, %rsp
              movq $1, %rax
              push %rax
              movq %rcx, %rsp" : /* no outputs */ : "r"(sp + 8), "r"(res) : "rax", "rcx");
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

    let sp = stack_frame.stack_pointer.0 - 160;
    ::state().scheduler.update_trap_frame(sp);

    unsafe {
        PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT);
    }

    ::state().scheduler.schedule();
}

extern "x86-interrupt" fn serial_handler(stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[SERIAL_INTERRUPT as usize] += 1;

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
