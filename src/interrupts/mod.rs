mod idt;

use buffers::print_error;

use constants::keyboard::KEYBOARD_INTERRUPT;
use constants::serial::SERIAL_INTERRUPT;
use constants::syscall::SYSCALL_INTERRUPT;
use constants::timer::TIMER_INTERRUPT;
use io::{keyboard, serial, timer, ChainedPics};

use spin::Mutex;

use x86;

macro_rules! caller_save {
    ( $( $x:expr ),* ) => {
        {
          // We have rax copied to IC, so we use rax to pop the error_code
          // off the stack.
          //asm!("pop %rax":::"memory" "{rax}");
          asm!("push %rcx":::"memory" "{rcx}");
          asm!("push %rdx":::"memory" "{rdx}");
          asm!("push %r8":::"memory" "{r8}");
          asm!("push %r9":::"memory" "{r9}");
          asm!("push %r10":::"memory" "{r10}");
          asm!("push %r11":::"memory" "{r11}");
          asm!("push %rdi":::"memory" "{rdi}");
          asm!("push %rsi":::"memory" "{rsi}");
        }
    };
}

macro_rules! caller_restore {
    ( $( $x:expr ),* ) => {
        {
          // Now pop everything back off the stack and to the registers.
          asm!("pop %rsi;
            pop %rdi;
            pop %r11;
            pop %r10;
            pop %r9;
            pop %r8;
            pop %rdx;
            pop %rcx;
            pop %rax;
            iretq;");
        }
    };
}

#[naked]
fn non_error_handler(id: u64) {
    unsafe {
        caller_save!();

        asm!("push $0"::"r"(id):"memory");

        // Note: This is only necessary in the non-error case.  In the case of error, it would mess things up.
        asm!("push $$0x0":::"memory");

        let sp: usize;
        asm!("" : "={rsp}"(sp));
        let ref ic: InterruptContext = *((sp - 64) as *const InterruptContext);

        interrupt_handler(&ic);

        caller_restore!();
    }
}

#[naked]
extern "C" fn noop_wrapper() {
    unsafe {
        caller_save!();

        asm!("push $$0xFF":::"memory");

        // Note: This is only necessary in the non-error case.  In the case of error, it would mess things up.
        asm!("push $$0x0":::"memory");

        let sp: usize;
        asm!("" : "={rsp}"(sp));
        let ref ic: InterruptContext = *((sp - 64) as *const InterruptContext);

        interrupt_handler(&ic);

        caller_restore!();
    }
}

#[naked]
extern "C" fn syscall_wrapper() {
    non_error_handler(SYSCALL_INTERRUPT as u64);
}

static mut test_passed: bool = false;

extern "C" fn interrupt_handler(ctx: &InterruptContext) {
    ::state().interrupt_count[ctx.int_id as usize] += 1;

    match ctx.int_id {
        // 0x00...0x0F => cpu_exception_handler(ctx),
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
            unsafe {
                if !test_passed {
                    test_passed = true;
                    return;
                }
            }

            println!("Syscall {:?}", ctx)
        }
        _ => {
            // println!("UNKNOWN INTERRUPT #{}", ctx.int_id);
            ::state().interrupt_count[0 as usize] += 1;
        }
    }

    unsafe {
        PICS.lock().notify_end_of_interrupt(ctx.int_id as u8);
    }
}

lazy_static! {
    static ref IDT: idt::Idt = {
        let mut idt = idt::Idt::new();

        for i in 0..255 {
          idt.set_handler(i as u8, syscall_wrapper);
        }

        idt.set_handler(SYSCALL_INTERRUPT as u8, syscall_wrapper);

        idt
    };
}

pub fn init() {
    IDT.load();

    unsafe {
        PICS.lock().initialize();

        int!(0x80);

        if test_passed {
            println!("Party on");
            x86::irq::enable();
        }
    }
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });

/// Various data available on our stack when handling an interrupt.
#[repr(C, packed)]
#[derive(Debug)]
struct InterruptContext {
    rsi: u64,
    rdi: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    int_id: u32,
    _pad_1: u32,
    error_code: u32,
    _pad_2: u32,
}

impl InterruptContext {
    fn empty() -> InterruptContext {
        InterruptContext {
            rsi: 0,
            rdi: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdx: 0,
            rcx: 0,
            rax: 0,
            int_id: 0,
            _pad_1: 0,
            error_code: 0,
            _pad_2: 0,
        }
    }
}
