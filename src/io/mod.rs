use core::marker::PhantomData;

mod x86;

use spin::Mutex;

/// Command sent to begin PIC initialization.
const CMD_INIT: u8 = 0x11;

/// Command sent to acknowledge an interrupt.
const CMD_END_OF_INTERRUPT: u8 = 0x20;

// The mode in which we want to run our PICs.
const MODE_8086: u8 = 0x01;

pub static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
  Port::new(0x60)
});

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe {
  ChainedPics::new(0x20, 0x28)
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

struct Pic {
  offset: u8,
  command: Port<u8>,
  data: Port<u8>,
}

impl Pic {
  fn handles_interrupt(&self, interupt_id: u8) -> bool {
    self.offset <= interupt_id && interupt_id < self.offset + 8
  }

  unsafe fn end_of_interrupt(&mut self) {
    self.command.write(CMD_END_OF_INTERRUPT);
  }
}

pub struct ChainedPics {
  pics: [Pic; 2],
}

impl ChainedPics {
  pub const unsafe fn new(offset1: u8, offset2: u8) -> ChainedPics {
    ChainedPics {
      pics: [
        Pic {
          offset: offset1,
          command: Port::new(0x20),
          data: Port::new(0x21),
        },
        Pic {
          offset: offset2,
          command: Port::new(0xA0),
          data: Port::new(0xA1),
        },
      ]
    }
  }

  pub unsafe fn initialize(&mut self) {

    let mut wait_port: Port<u8> = Port::new(0x80);
    let mut wait = || { wait_port.write(0) };

    let saved_mask1 = self.pics[0].data.read();
    let saved_mask2 = self.pics[1].data.read();

     // Tell each PIC that we're going to send it a three-byte
    // initialization sequence on its data port.
    self.pics[0].command.write(CMD_INIT);
    wait();
    self.pics[1].command.write(CMD_INIT);
    wait();

    // Byte 1: Set up our base offsets.
    self.pics[0].data.write(self.pics[0].offset);
    wait();
    self.pics[1].data.write(self.pics[1].offset);
    wait();

    // Byte 2: Configure chaining between PIC1 and PIC2.
    self.pics[0].data.write(4);
    wait();
    self.pics[1].data.write(2);
    wait();

    // Byte 3: Set our mode.
    self.pics[0].data.write(MODE_8086);
    wait();
    self.pics[1].data.write(MODE_8086);
    wait();

    self.pics[0].data.write(saved_mask1);
    self.pics[1].data.write(saved_mask2);
  }

  pub fn handles_interrupt(&self, interrupt_id: u8) -> bool {
    self.pics.iter().any(|p| p.handles_interrupt(interrupt_id))
  }

  pub unsafe fn notify_end_of_interrupt(&mut self, interrupt_id: u8) {
    if self.handles_interrupt(interrupt_id) {
      if self.pics[1].handles_interrupt(interrupt_id) {
        self.pics[1].end_of_interrupt();
      }
      self.pics[0].end_of_interrupt();
    }
  }
}