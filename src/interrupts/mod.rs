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

macro_rules! handler {
    ($name: ident) => {{
        #[naked]
        extern "C" fn wrapper() -> ! {
            unsafe {
                asm!("mov rdi, rsp
                      sub rsp, 8 // align the stack pointer
                      call $0"
                      :: "i"($name as extern "C" fn(
                          *const ExceptionStackFrame) -> !)
                      : "rdi" : "intel");
                ::core::intrinsics::unreachable();
            }
        }
        wrapper
    }}
}

macro_rules! handler_with_error_code {
    ($name: ident) => {{
        #[naked]
        extern "C" fn wrapper() -> ! {
            unsafe {
                asm!("pop rsi // pop error code into rsi
                      mov rdi, rsp
                      sub rsp, 8 // align the stack pointer
                      call $0"
                      :: "i"($name as extern "C" fn(
                          *const ExceptionStackFrame, u64) -> !)
                      : "rdi" : "intel");
                ::core::intrinsics::unreachable();
            }
        }
        wrapper
    }}
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

        idt.set_handler(0, handler!(divide_by_zero_handler));
        idt.set_handler(6, handler!(invalid_opcode_handler));
        idt.set_handler(14, handler_with_error_code!(page_fault_handler));

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

extern "C" fn divide_by_zero_handler(stack_frame: *const ExceptionStackFrame) -> ! {
    unsafe {
        print_error(format_args!("EXCEPTION: DIVIDE BY ZERO\n{:#?}",
            *stack_frame));
    };
    loop {}
}

extern "C" fn invalid_opcode_handler(stack_frame: *const ExceptionStackFrame)
    -> !
{
    unsafe {
        print_error(format_args!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
            (*stack_frame).instruction_pointer, *stack_frame));
    }
    loop {}
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

extern "C" fn page_fault_handler(stack_frame: *const ExceptionStackFrame,
                                 error_code: u64) -> !
{
    use x86::controlregs;
    unsafe {
        print_error(format_args!(
            "EXCEPTION: PAGE FAULT while accessing {:#x}\
            \nerror code: {:?}\n{:#?}",
            controlregs::cr2(),
            PageFaultErrorCode::from_bits(error_code).unwrap(),
            *stack_frame));
    }
    loop {}
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });
