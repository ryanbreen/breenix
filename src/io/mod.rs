use core::marker::PhantomData;

use x86::io::{inl, outl, outw, inw, outb, inb};

use crate::event;

//#[macro_use]
pub mod keyboard;

//pub mod pci;

//pub mod printk;

pub mod serial;

pub mod timer;

pub mod drivers;

/// This trait is defined for any type which can be read or written over a
/// port.  The processor supports I/O with `u8`, `u16` and `u32`.  The
/// functions in this trait are all unsafe because they can write to
/// arbitrary ports.
pub trait InOut {
    /// Read a value from the specified port.
    unsafe fn port_in(port: u16) -> Self;

    /// Write a value to the specified port.
    unsafe fn port_out(port: u16, value: Self);
}

impl InOut for u8 {
    unsafe fn port_in(port: u16) -> u8 {
        inb(port)
    }
    unsafe fn port_out(port: u16, value: u8) {
        outb(port, value);
    }
}

impl InOut for u16 {
    unsafe fn port_in(port: u16) -> u16 {
        inw(port)
    }
    unsafe fn port_out(port: u16, value: u16) {
        outw(port, value);
    }
}

impl InOut for u32 {
    unsafe fn port_in(port: u16) -> u32 {
        inl(port)
    }
    unsafe fn port_out(port: u16, value: u32) {
        outl(port, value);
    }
}

pub struct Port<T: InOut> {
    port: u16,
    phantom: PhantomData<T>,
}

impl<T: InOut> Port<T> {
    /// Create a new I/O port.
    pub const unsafe fn new(port: u16) -> Port<T> {
        Port {
            port: port,
            phantom: PhantomData,
        }
    }

    /// Read data from the port.  This is nominally safe, because you
    /// shouldn't be able to get hold of a port object unless somebody
    /// thinks it's safe to give you one.
    pub fn read(&self) -> T {
        unsafe { T::port_in(self.port) }
    }

    /// Write data to the port.
    pub fn write(&mut self, value: T) {
        unsafe {
            T::port_out(self.port, value);
        }
    }
}

pub fn initialize() {
    serial::initialize();
    timer::initialize();
    //event::keyboard::initialize();
    //pci::initialize();
}

