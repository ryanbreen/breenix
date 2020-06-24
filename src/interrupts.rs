use crate::println;
use crate::constants::keyboard::KEYBOARD_INTERRUPT;
use crate::constants::interrupts::DOUBLE_FAULT_IST_INDEX;
use crate::constants::serial::SERIAL_INTERRUPT;
use crate::constants::syscall::SYSCALL_INTERRUPT;
use crate::constants::timer::TIMER_INTERRUPT;

use core::fmt;

//use crate::io::{keyboard, serial, timer};
//use crate::io::pic::ChainedPics;
//use crate::memory;

use lazy_static::lazy_static;

use spin::Mutex;
use spin::Once;

pub mod gdt;

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
/*

use x86_64::structures::tss::TaskStateSegment;
static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<gdt::Gdt> = Once::new();
*/

pub static mut TEST_PASSED: bool = false;

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

#[allow(unused_must_use)]
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

/*
pub unsafe fn test_interrupt() {
    use libbreenix;
    let res = libbreenix::sys_test();
    println!("Syscall result is {}", res);
    TEST_PASSED = res == 2016;
    if !TEST_PASSED {
        panic!("test SYSCALL failed");
    }
}
*/

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {

        let mut idt = InterruptDescriptorTable::new();
        //idt.divide_by_zero.set_handler_fn(divide_by_zero_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        //idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        //idt.page_fault.set_handler_fn(page_fault_handler);
        
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX as u16);
        }
        /*
        for i in 0..256-32 {
            idt.interrupts[i].set_handler_fn(dummy_error_handler);
        }
        */
    
        /*
        idt.interrupts[(SERIAL_INTERRUPT - 32) as usize].set_handler_fn(serial_handler);
        idt.interrupts[(SYSCALL_INTERRUPT - 32) as usize].set_handler_fn(syscall_handler);
        idt.interrupts[(TIMER_INTERRUPT - 32) as usize].set_handler_fn(timer_handler);
        idt.interrupts[(KEYBOARD_INTERRUPT - 32) as usize].set_handler_fn(keyboard_handler);
        
        idt.interrupts[11].set_handler_fn(nic_interrupt_handler);
        idt.interrupts[15].set_handler_fn(nic_interrupt_handler);
        **/

        idt
    };
}

#[allow(dead_code)]
pub fn initialize() {

    gdt::init();
    IDT.load();

    /*
    unsafe {

        PICS.lock().initialize();

        test_interrupt();

        if TEST_PASSED {
            println!("Test passed");

            let irq_base = (0x20 as usize + 16) & 0xfffffff0;
            println!("irq: {:x}", irq_base);
        }
    }
    */
}


extern "x86-interrupt" fn dummy_error_handler(stack_frame: &mut InterruptStackFrame)
{
    println!("EXCEPTION: UNHANDLED\n{:#?}", stack_frame);
}

#[test_case]
fn test_breakpoint_exception() {
    // invoke a breakpoint exception
    x86_64::instructions::interrupts::int3();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}
/*
extern "x86-interrupt" fn nic_interrupt_handler(_stack_frame: &mut ExceptionStackFrame) {
    println!("Packet received!!!");
}
*/
extern "x86-interrupt" fn double_fault_handler(stack_frame: &mut InterruptStackFrame,
    _error_code: u64) -> !
{
    println!("\nEXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    loop {}
}
/*
extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: &mut ExceptionStackFrame)
{
    println!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
            stack_frame.instruction_pointer, stack_frame);
    loop {}
}

extern "x86-interrupt" fn syscall_handler(_stack_frame: &mut ExceptionStackFrame)
{
    unsafe {
        asm!("cli");

        let mut my_sp:usize;
        asm!("" : "={rbp}"(my_sp));
        // x86-interrupt pushes 14 u64s to the stack, the last of which is RAX, but since we
        // plan to return rax from this function, we will set that register directly rather
        // than pop it from the stack.
        my_sp -= 8 * 13;

        //println!("SYSCALL:\n{:#?}", stack_frame);

        ::state().interrupt_count[SYSCALL_INTERRUPT as usize] += 1;

        let sp = my_sp + 0x18;

        let ref ic:InterruptContext = *(sp as * const InterruptContext);
        let num = ic.rax;
        let a = ic.rdi;
        let b = ic.rsi;
        let c = ic.rdx;
        let d = ic.r10;
        let e = ic.r8;
        let f = ic.r9;

        let res = syscall::handle(num, a, b, c, d, e, f);

        PICS.lock().notify_end_of_interrupt(SYSCALL_INTERRUPT);

        asm!(  "movq $0, %rsp
                movq $1, %rax
                pop    %rbx
                pop    %rcx
                pop    %rdx
                pop    %rsi
                pop    %rdi
                pop    %r8
                pop    %r9
                pop    %r10
                pop    %r11
                pop    %r12
                pop    %r13
                pop    %r14
                pop    %r15
                pop    %rbp
                sti
                iretq" :  : "r"(my_sp), "r"(res) : );
            
    }
}

extern "x86-interrupt" fn timer_handler(_stack_frame: &mut ExceptionStackFrame)
{
    unsafe {
        asm!("cli");

        let mut my_sp:usize;
        asm!("" : "={rbp}"(my_sp));

        // x86-interrupt pushes 11 u64s to the stack, the last of which is RAX, so we want
        // our stack pointer at the end of this function to point to where RAX lives on the
        // stack.
        my_sp -= 8 * 10;

        ::state().interrupt_count[TIMER_INTERRUPT as usize] += 1;

        timer::timer_interrupt();

        ::state().scheduler.update_trap_frame(my_sp);
        PICS.lock().notify_end_of_interrupt(TIMER_INTERRUPT);

        ::state().scheduler.schedule();
    }
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[KEYBOARD_INTERRUPT as usize] += 1;

    keyboard::read();

    unsafe {
        PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT);
    }
}

extern "x86-interrupt" fn serial_handler(_stack_frame: &mut ExceptionStackFrame)
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
*/