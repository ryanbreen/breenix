use core::marker::PhantomData;

mod x86;

use spin::Mutex;

pub static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
    Port::new(0x60)
});

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
    unsafe fn port_in(port: u16) -> u8 { x86::inb(port) }
    unsafe fn port_out(port: u16, value: u8) { x86::outb(value, port); }
}

impl InOut for u16 {
    unsafe fn port_in(port: u16) -> u16 { x86::inw(port) }
    unsafe fn port_out(port: u16, value: u16) { x86::outw(value, port); }
}

impl InOut for u32 {
    unsafe fn port_in(port: u16) -> u32 { x86::inl(port) }
    unsafe fn port_out(port: u16, value: u32) { x86::outl(value, port); }
}

pub struct Port<T: InOut> {
    port: u16,
    phantom: PhantomData<T>,
}

impl<T: InOut> Port<T> {
    /// Create a new I/O port.
    pub const unsafe fn new(port: u16) -> Port<T> {
        Port { port: port, phantom: PhantomData }
    }

    /// Read data from the port.  This is nominally safe, because you
    /// shouldn't be able to get hold of a port object unless somebody
    /// thinks it's safe to give you one.
    pub unsafe fn read(&mut self) -> T {
        unsafe { T::port_in(self.port) }
    }

    /// Write data to the port.
    pub unsafe fn write(&mut self, value: T) {
        unsafe { T::port_out(self.port, value); }
    }
}