pub use self::port::Port;
mod port;

use spin::Mutex;

pub static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
    Port::new(0x60)
});