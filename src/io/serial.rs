use debug;
use constants::serial::COM1;
use x86::shared::io::{inb, outb};

unsafe fn is_transmit_empty() -> u8 {
    return inb(COM1 + 5) & 0x20;
}

pub fn write_char(c: char) {
    unsafe {
        while is_transmit_empty() == 0 {}

        outb(COM1, c as u8);
    }
}

pub fn write(s: &str) {
    for c in s.chars() {
        write_char(c);
    }
}

unsafe fn serial_received() -> u8 {
    return inb(COM1 + 5) & 1;
}

fn read_char() -> char {
    unsafe {
        while serial_received() == 0 {}

        inb(COM1) as char
    }
}

pub fn read() {
    debug::handle_serial_input(read_char() as u8);
}

pub fn initialize() {
    unsafe {
        outb(COM1 + 1, 0x00);    // Disable all interrupts
        outb(COM1 + 3, 0x80);    // Enable DLAB (set baud rate divisor)
        outb(COM1 + 0, 0x03);    // Set divisor to 3 (lo byte) 38400 baud
        outb(COM1 + 1, 0x00);    //                  (hi byte)
        outb(COM1 + 3, 0x03);    // 8 bits, no parity, one stop bit
        outb(COM1 + 2, 0xC7);    // Enable FIFO, clear them, with 14-byte threshold
        outb(COM1 + 4, 0x0B);    // IRQs enabled, RTS/DSR set
        outb(COM1 + 1, 0x01);    // Disable all interrupts

        write("serial port initialized\n");
    }
}
