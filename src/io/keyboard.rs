use spin::Mutex;

use io;
use io::Port;

static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
  Port::new(0x60)
});

pub fn test() {

  loop {
    unsafe {

      let scancode = KEYBOARD.lock().read();

      // If the user hits 'q', exit.
      if scancode == 16 {
        io::PICS.lock().initialize();
        println!("Interrupts engaged");
        panic!();
      }

      if scancode != 0xFA {
        println!("Got keyboard code {}", scancode);
      }

    }
  }
}