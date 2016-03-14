
#[derive(Clone, Copy)]
pub enum EventType {
  KeyEvent,
  MouseEvent,
  FsEvent,
}

pub trait IsEvent {
  fn event_type(&self) -> EventType;
}

pub trait IsListener<T: IsEvent> {
  fn handles_event(&self, ev: &T) -> bool;

  fn notify(&self, ev: &T);
}

#[derive(Clone, Copy)]
pub struct ControlKeyState {
  ctrl: bool,
  alt: bool,
  shift: bool,
  caps_lock: bool,
  scroll_lock: bool,
  num_lock: bool
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
  event_type: EventType,
  scancode: u8,
  character: char,
  controls: ControlKeyState
}

impl IsEvent for KeyEvent {
  fn event_type(&self) -> EventType {
    self.event_type
  }
}