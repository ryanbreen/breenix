use event::EventType;

#[derive(Clone, Copy)]
pub struct ControlKeyState {
  pub ctrl: bool,
  pub alt: bool,
  pub shift: bool,
  pub caps_lock: bool,
  pub scroll_lock: bool,
  pub num_lock: bool
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
  pub event_type: EventType,
  pub scancode: u8,
  pub character: char,
  pub controls: ControlKeyState
}