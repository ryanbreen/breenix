use core::marker::PhantomData;

/// Read a `u8`-sized value from `port`.
pub unsafe fn inb(port: u16) -> u8 {
    // The registers for the `in` and `out` instructions are always the
    // same: `a` for value, and `d` for the port address.
    let result: u8;
    asm!("inb %dx, %al" : "={al}"(result) : "{dx}"(port) :: "volatile");
    result
}

/// Write a `u8`-sized `value` to `port`.
pub unsafe fn outb(value: u8, port: u16) {
    asm!("outb %al, %dx" :: "{dx}"(port), "{al}"(value) :: "volatile");
}

/// Read a `u16`-sized value from `port`.
pub unsafe fn inw(port: u16) -> u16 {
    let result: u16;
    asm!("inw %dx, %ax" : "={ax}"(result) : "{dx}"(port) :: "volatile");
    result
}

/// Write a `u8`-sized `value` to `port`.
pub unsafe fn outw(value: u16, port: u16) {
    asm!("outw %ax, %dx" :: "{dx}"(port), "{ax}"(value) :: "volatile");
}

/// Read a `u32`-sized value from `port`.
pub unsafe fn inl(port: u16) -> u32 {
    let result: u32;
    asm!("inl %dx, %eax" : "={eax}"(result) : "{dx}"(port) :: "volatile");
    result
}

/// Write a `u32`-sized `value` to `port`.
pub unsafe fn outl(value: u32, port: u16) {
    asm!("outl %eax, %dx" :: "{dx}"(port), "{eax}"(value) :: "volatile");
}

pub struct Port<T> {
    port: u16,
    phantom: PhantomData<T>,
}

impl<T> Port<T> {
    pub unsafe fn new(port: u16) -> Port<T> {
        Port { port: port, phantom: PhantomData }
    }

    pub fn read(&mut self) -> u8 {
        unsafe { inb(self.port) }
    }

    pub fn write(&mut self, value: u8) {
        unsafe { outb(value, self.port) }
    }
}