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
    rax: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
}

macro_rules! save_scratch_registers {
    () => {
        asm!("push rax
              push rcx
              push rdx
              push rsi
              push rdi
              push r8
              push r9
              push r10
              push r11
        " :::: "intel", "volatile");
    }
}

macro_rules! restore_scratch_registers {
    () => {
        asm!("pop r11
              pop r10
              pop r9
              pop r8
              pop rdi
              pop rsi
              pop rdx
              pop rcx
              pop rax
            " :::: "intel", "volatile");
    }
}

macro_rules! handler {
    ($name: ident) => {{
        #[naked]
        extern "C" fn wrapper() -> ! {
            unsafe {
                save_scratch_registers!();

                asm!("mov rdi, rsp
                      add rdi, 9*8 // calculate exception stack frame pointer
                      call $0"
                      :: "i"($name as extern "C" fn(
                          &ExceptionStackFrame))
                      : "rdi" : "intel");

                restore_scratch_registers!();
                asm!("iretq":::: "intel", "volatile");
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
                save_scratch_registers!();
                asm!("mov rsi, [rsp + 9*8] // load error code into rsi
                      mov rdi, rsp
                      add rdi, 10*8 // calculate exception stack frame pointer
                      sub rsp, 8 // align the stack pointer
                      call $0
                      add rsp, 8 // undo stack pointer alignment"
                      :: "i"($name as extern "C" fn(
                          &ExceptionStackFrame, u64))
                      : "rdi" : "intel");
                restore_scratch_registers!();
                asm!("add rsp, 8 // pop error code
                      iretq" :::: "intel", "volatile");
                ::core::intrinsics::unreachable();
            }
        }
        wrapper
    }}
}

macro_rules! interrupt {
    ($id: ident, $name: ident) => {{
        #[naked]
        extern "C" fn wrapper() -> ! {
            unsafe {
                save_scratch_registers!();
                asm!("mov rsi, $0
                      mov rdi, rsp
                      add rdi, 8*8 // calculate exception stack frame pointer
                      call $1"
                      :: "i"($id as u64), "i"($name as extern "C" fn(&ExceptionStackFrame, u64))
                      : "rdi" : "intel");

                restore_scratch_registers!();
                asm!("iretq" :::: "intel", "volatile");
                ::core::intrinsics::unreachable();
            }
        }
        wrapper
    }}
}
// Was 0x1c57c8 and then i set it to 0x1c5758
static mut test_passed: bool = false;

extern "C" fn interrupt_handler(stack_frame: &ExceptionStackFrame, int_id: u64) {
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
            syscall_handler(stack_frame, stack_frame.rax);
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
            // Install a dummy handler if we don't care about the interrupt
            idt.set_handler(i, handler!(dummy_error_handler));
        }

        idt.set_handler(0, handler!(divide_by_zero_handler));
        idt.set_handler(1, handler!(dummy_error_handler));
        idt.set_handler(3, handler!(breakpoint_handler));
        idt.set_handler(6, handler!(invalid_opcode_handler));
        idt.set_handler(14, handler_with_error_code!(page_fault_handler));
        idt.set_handler(TIMER_INTERRUPT as u8, interrupt!(TIMER_INTERRUPT, interrupt_handler));
        idt.set_handler(KEYBOARD_INTERRUPT as u8, interrupt!(KEYBOARD_INTERRUPT, interrupt_handler));
        idt.set_handler(SERIAL_INTERRUPT as u8, interrupt!(SERIAL_INTERRUPT, interrupt_handler));
        idt.set_handler(SYSCALL_INTERRUPT as u8, interrupt!(SYSCALL_INTERRUPT, interrupt_handler));

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

        test_interrupt();

        if test_passed {
            x86::irq::enable();

            let ret:u64 = syscall!(69, 12, 13);
            println!("Got return value {}", ret);
        }
    }
}

extern "C" fn dummy_error_handler(stack_frame: &ExceptionStackFrame)
{
    let stack_frame = unsafe { &*stack_frame };
    println!("\nEXCEPTION: UNHANDLED at {:#x}\n{:#?}",
        stack_frame.instruction_pointer, stack_frame);
}

extern "C" fn breakpoint_handler(stack_frame: &ExceptionStackFrame)
{
    let stack_frame = unsafe { &*stack_frame };
    println!("\nEXCEPTION: BREAKPOINT at {:#x}\n{:#?}",
        stack_frame.instruction_pointer, stack_frame);
}

extern "C" fn where_we_at(addr: u64, addr2: u64) {
    println!("We are at {:x} {:x}", addr, addr2);
}

extern "C" fn syscall_handler(stack_frame: &ExceptionStackFrame,
    syscall_id: u64)
{
    let stack_frame = unsafe { &*stack_frame };
    println!("\nEXCEPTION: SYSCALL at {:#x}\n{:#?}",
        stack_frame.instruction_pointer, stack_frame);

    let mut blah:u64 = 0xdeadbeef;
    blah += 8;

    unsafe {
        asm!("mov rdi, rip
              mov rsi, 0
              call $0"
              :: "i"(where_we_at as extern "C" fn(u64, u64))
              : "rip", "rdi" : "intel", "volatile");
    }
}

extern "C" fn divide_by_zero_handler(stack_frame: &ExceptionStackFrame)
{
    unsafe {
        print_error(format_args!("EXCEPTION: DIVIDE BY ZERO\n{:#?}",
            *stack_frame));
        loop {}
    };
}

extern "C" fn invalid_opcode_handler(stack_frame: &ExceptionStackFrame)
{
    unsafe {
        print_error(format_args!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
            (*stack_frame).instruction_pointer, *stack_frame));
        loop {}
    }
}

extern "C" fn page_fault_handler(stack_frame: &ExceptionStackFrame,
                                 error_code: u64)
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
