
#[derive(Clone, Copy)]
pub enum EventType {
  KeyEvent,
  MouseEvent,
  FsEvent,
}

#[derive(Clone, Copy)]
pub struct Event {
  event_type: EventType,
}

pub trait IsListener {
  fn event_type_subscribed(&self) -> EventType;

  fn handles_event(&self, ev: &Event) -> bool;

  fn notify(&self, ev: &Event);
}
