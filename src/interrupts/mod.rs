use crate::{print,println};
use crate::constants::keyboard::KEYBOARD_INTERRUPT;
use crate::constants::interrupts::{PIC_1_OFFSET, PIC_2_OFFSET, DOUBLE_FAULT_IST_INDEX};
use crate::constants::serial::SERIAL_INTERRUPT;
use crate::constants::syscall::SYSCALL_INTERRUPT;
use crate::constants::timer::TIMER_INTERRUPT;
use crate::state;

use core::fmt;

use crate::io::keyboard;
use crate::io::timer;
//use crate::memory;

use lazy_static::lazy_static;
use pic8259_simple::ChainedPics;
use spin;

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

pub mod gdt;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

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
        idt.divide_error.set_handler_fn(divide_by_zero_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX as u16);
        }

        for i in 32..256 {
            idt[i].set_handler_fn(dummy_error_handler);
        }
    
        /*
        idt.interrupts[(SERIAL_INTERRUPT - 32) as usize].set_handler_fn(serial_handler);
        idt.interrupts[(SYSCALL_INTERRUPT - 32) as usize].set_handler_fn(syscall_handler);
        */
        idt[TIMER_INTERRUPT as usize].set_handler_fn(timer_handler);
        
        idt[KEYBOARD_INTERRUPT as usize].set_handler_fn(keyboard_handler);
        /*
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

    unsafe { PICS.lock().initialize() };

    x86_64::instructions::interrupts::enable();

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

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: &mut InterruptStackFrame)
{
    println!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: &mut InterruptStackFrame)
{
    println!("EXCEPTION: INVALID OPCODE at {:?}\n{:#?}",
            stack_frame.instruction_pointer, stack_frame);
    loop {}
}

/*
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
*/

extern "x86-interrupt" fn timer_handler(_stack_frame: &mut InterruptStackFrame)
{
    use x86_64::instructions::interrupts;
    state::increment_interrupt_count(TIMER_INTERRUPT as usize);
    timer::timer_interrupt();

    unsafe {
        PICS.lock().notify_end_of_interrupt(TIMER_INTERRUPT);
    }
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: &mut InterruptStackFrame)
{
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        state::increment_interrupt_count(KEYBOARD_INTERRUPT as usize);

        use x86_64::instructions::port::Port;

        let mut port = Port::new(0x60);
        let scancode: u8 = unsafe { port.read() };
        crate::io::keyboard::add_scancode(scancode);
        // keyboard::read();

        unsafe {
            PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT);
        }
    });
}
/*
extern "x86-interrupt" fn serial_handler(_stack_frame: &mut ExceptionStackFrame)
{
    ::state().interrupt_count[SERIAL_INTERRUPT as usize] += 1;

    serial::read();

    unsafe {
        PICS.lock().notify_end_of_interrupt(SERIAL_INTERRUPT);
    }
}
*/

use x86_64::structures::idt::PageFaultErrorCode;
use crate::hlt_loop;

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: &mut InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    println!("EXCEPTION: PAGE FAULT");
    println!("Accessed Address: {:?}", Cr2::read());
    println!("Error Code: {:?}", error_code);
    println!("{:#?}", stack_frame);
    hlt_loop();
}