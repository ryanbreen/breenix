pub struct Port {
    port: u16,
}

impl Port {
    pub unsafe fn new(port: u16) -> Port {
        Port { port: port }
    }

    pub fn read(&mut self) -> u8 {
        unsafe { inb(self.port) }
    }

    pub fn write(&mut self, value: u8) {
        unsafe { outb(value, self.port) }
    }
}