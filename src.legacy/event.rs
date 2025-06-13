pub mod keyboard;

pub use self::keyboard::KeyEvent;

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum EventType {
    KeyEvent,
    MouseEvent,
    FsEvent,
}
