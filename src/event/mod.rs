
use alloc::boxed::Box;
use collections::vec::Vec;
use core::cell::UnsafeCell;

use spin::Mutex;

use io::keyboard::KeyEvent;

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

static mut KEY_EVENT_LISTENERS_PTR:usize = 0;

pub fn key_event_listeners() -> Option<&'static mut Vec<Box<IsListener<KeyEvent>>>> {
  unsafe {
    if KEY_EVENT_LISTENERS_PTR == 0 {
      return None;
    }

    let pointer = KEY_EVENT_LISTENERS_PTR as *mut Vec<Box<IsListener<KeyEvent>>>;
    return Some(&mut (*pointer));
  }
}

pub fn set_key_event_listener(listeners: &'static mut Vec<Box<IsListener<KeyEvent>>>) {
  unsafe {
    KEY_EVENT_LISTENERS_PTR = listeners as *mut _ as usize;
  }
}

pub fn dispatch_key_event(ev: &KeyEvent) {
  if let Some(listeners) = key_event_listeners() {
    for listener in listeners {
      if listener.handles_event(ev) {
        listener.notify(ev);
      }
    }
  }
}
