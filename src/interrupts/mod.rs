mod idt;

use buffers::print_error;

use constants::keyboard::KEYBOARD_INTERRUPT;
use constants::serial::SERIAL_INTERRUPT;
use constants::syscall::SYSCALL_INTERRUPT;
use constants::timer::TIMER_INTERRUPT;
use interrupts::idt::HandlerFunc;
use io::{keyboard, serial, timer, ChainedPics};

use spin::Mutex;

use x86;

#[derive(Debug)]
#[repr(C)]
struct ExceptionStackFrame {
    instruction_pointer: u64,
    code_segment: u64,
    cpu_flags: u64,
    stack_pointer: u64,
    stack_segment: u64,
}


static mut test_passed: bool = false;

extern "C" fn interrupt_handler(int_id: u8) {
    ::state().interrupt_count[int_id as usize] += 1;

    match int_id {
        //0x00...0x0F => println!("error: {}", int_id),
        TIMER_INTERRUPT => {
            timer::timer_interrupt();
        }
        KEYBOARD_INTERRUPT => {
            keyboard::read();
        }
        SERIAL_INTERRUPT => {
            serial::read();
        }
        // On Linux, this is used for syscalls.  Good enough for me.
        SYSCALL_INTERRUPT => {
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

lazy_static! {
    static ref IDT: idt::Idt = {
        let mut idt = idt::Idt::new();

        for i in 0..255 {
          if i != SYSCALL_INTERRUPT {
              //idt.set_handler(i as u8, noop_wrapper);
          }
        }

        idt.set_handler(0, divide_by_zero_handler);

        //idt.set_handler(SYSCALL_INTERRUPT as u8, syscall_wrapper);

        idt
    };
}

pub unsafe fn test_interrupt() {
    int!(SYSCALL_INTERRUPT);
    test_passed = true;
}

#[allow(dead_code)]
pub fn init() {
    unsafe {

        PICS.lock().initialize();
        IDT.load();

        //test_interrupt();

        if test_passed {
            x86::irq::enable();
        }
    }
}

extern "C" fn divide_by_zero_handler() -> ! {
    let stack_frame: *const ExceptionStackFrame;
    unsafe {
        asm!("mov $0, rsp" : "=r"(stack_frame) ::: "intel");
        print_error(format_args!("EXCEPTION: DIVIDE BY ZERO\n{:#?}",
            *stack_frame));
    };
    loop {}
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });
