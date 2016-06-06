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
fn error_handler(id: u8) {
    unsafe {
        caller_save!();

        interrupt_handler(id);

        caller_restore!();

        asm!("addq $$8, %rsp");

        //asm!("iretq");
    }
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

#[naked]
extern "C" fn noop_wrapper0() {
    non_error_handler(0);
}

#[naked]
extern "C" fn noop_wrapper1() {
    non_error_handler(1);
}

#[naked]
extern "C" fn noop_wrapper2() {
    non_error_handler(2);
}

#[naked]
extern "C" fn noop_wrapper3() {
    non_error_handler(3);
}

#[naked]
extern "C" fn noop_wrapper4() {
    non_error_handler(4);
}

#[naked]
extern "C" fn noop_wrapper5() {
    non_error_handler(5);
}

#[naked]
extern "C" fn noop_wrapper6() {
    non_error_handler(6);
}

#[naked]
extern "C" fn noop_wrapper7() {
    non_error_handler(7);
}

#[naked]
extern "C" fn noop_wrapper8() {
    non_error_handler(8);
}

#[naked]
extern "C" fn noop_wrapper9() {
    non_error_handler(9);
}

#[naked]
extern "C" fn noop_wrapper10() {
    non_error_handler(10);
}

#[naked]
extern "C" fn noop_wrapper11() {
    non_error_handler(11);
}

#[naked]
extern "C" fn noop_wrapper12() {
    non_error_handler(12);
}

#[naked]
extern "C" fn noop_wrapper13() {
    error_handler(13);
}

#[naked]
extern "C" fn noop_wrapper14() {
    error_handler(14);
}

#[naked]
extern "C" fn noop_wrapper15() {
    non_error_handler(15);
}

#[naked]
extern "C" fn noop_wrapper16() {
    non_error_handler(16);
}

#[naked]
extern "C" fn noop_wrapper17() {
    non_error_handler(17);
}

#[naked]
extern "C" fn noop_wrapper18() {
    non_error_handler(18);
}

#[naked]
extern "C" fn noop_wrapper19() {
    non_error_handler(19);
}

#[naked]
extern "C" fn noop_wrapper20() {
    non_error_handler(20);
}

#[naked]
extern "C" fn noop_wrapper21() {
    non_error_handler(21);
}

#[naked]
extern "C" fn noop_wrapper22() {
    non_error_handler(22);
}

#[naked]
extern "C" fn noop_wrapper23() {
    non_error_handler(23);
}

#[naked]
extern "C" fn noop_wrapper24() {
    non_error_handler(24);
}

#[naked]
extern "C" fn noop_wrapper25() {
    non_error_handler(25);
}

#[naked]
extern "C" fn noop_wrapper26() {
    non_error_handler(26);
}

#[naked]
extern "C" fn noop_wrapper27() {
    non_error_handler(27);
}

#[naked]
extern "C" fn noop_wrapper28() {
    non_error_handler(28);
}

#[naked]
extern "C" fn noop_wrapper29() {
    non_error_handler(29);
}

#[naked]
extern "C" fn noop_wrapper30() {
    non_error_handler(30);
}

#[naked]
extern "C" fn noop_wrapper31() {
    non_error_handler(31);
}

#[naked]
extern "C" fn noop_wrapper32() {
    non_error_handler(32);
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

        idt.set_handler(0, noop_wrapper0);
        idt.set_handler(1, noop_wrapper1);
        idt.set_handler(2, noop_wrapper2);
        idt.set_handler(3, noop_wrapper3);
        idt.set_handler(4, noop_wrapper4);
        idt.set_handler(5, noop_wrapper5);
        idt.set_handler(6, noop_wrapper6);
        idt.set_handler(7, noop_wrapper7);
        idt.set_handler(8, noop_wrapper8);
        idt.set_handler(9, noop_wrapper9);
        idt.set_handler(10, noop_wrapper10);
        idt.set_handler(11, noop_wrapper11);
        idt.set_handler(12, noop_wrapper12);
        idt.set_handler(13, noop_wrapper13);
        idt.set_handler(14, noop_wrapper14);
        idt.set_handler(15, noop_wrapper15);
        idt.set_handler(16, noop_wrapper16);
        idt.set_handler(17, noop_wrapper17);
        idt.set_handler(18, noop_wrapper18);
        idt.set_handler(19, noop_wrapper19);
        idt.set_handler(20, noop_wrapper20);
        idt.set_handler(21, noop_wrapper21);
        idt.set_handler(22, noop_wrapper22);
        idt.set_handler(23, noop_wrapper23);
        idt.set_handler(24, noop_wrapper24);
        idt.set_handler(25, noop_wrapper25);
        idt.set_handler(26, noop_wrapper26);
        idt.set_handler(27, noop_wrapper27);
        idt.set_handler(28, noop_wrapper28);
        idt.set_handler(29, noop_wrapper29);
        idt.set_handler(30, noop_wrapper30);
        idt.set_handler(31, noop_wrapper31);
        idt.set_handler(32, noop_wrapper32);

        idt.set_handler(SYSCALL_INTERRUPT as u8, syscall_wrapper);

        idt
    };
}

pub unsafe fn test_interrupt() {
    int!(SYSCALL_INTERRUPT);
    test_passed = true;
}

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
