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
          asm!("push %rax":::"memory" "{rax}");
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
            pop %rax;");
        }
    };
}

#[naked]
fn non_error_handler(id: u8) {
    unsafe {
        caller_save!();

        interrupt_handler(id);

        print_error(format_args!("interrupt handled\n"));

        caller_restore!();

        print_error(format_args!("caller restored\n"));

        asm!("iretq");
    }
}

#[naked]
extern "C" fn syscall_wrapper() {
    non_error_handler(SYSCALL_INTERRUPT);
}

#[naked]
extern "C" fn noop_wrapper() {
    non_error_handler(0xFF);
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
              idt.set_handler(i as u8, noop_wrapper);
          }
        }

        idt.set_handler(SYSCALL_INTERRUPT as u8, syscall_wrapper);

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
        }
    }
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });
