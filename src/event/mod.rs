
use alloc::boxed::Box;
use collections::vec::Vec;
use core::intrinsics;
use core::ptr;
use core::ptr::Unique;

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

//static mut KEY_EVENT_LISTENERS_PTR:usize = 0;
static mut KEY_EVENT_LISTENERS_PTR:Unique<Vec<Box<IsListener<KeyEvent>>>> = unsafe { Unique::new(0xffff000 as * mut _) };

fn key_event_listeners() -> &'static Vec<Box<IsListener<KeyEvent>>> {
  unsafe {
    return KEY_EVENT_LISTENERS_PTR.get();
  }
}

pub fn register_key_event_listener(listener: Box<IsListener<KeyEvent>>) {
  unsafe {
    let mut listeners:&'static mut Vec<Box<IsListener<KeyEvent>>> = KEY_EVENT_LISTENERS_PTR.get_mut();
    listeners.push(listener);
    println!("Got listeners: {}", listeners.len());
  }
}

pub fn dispatch_key_event(ev: &KeyEvent) {
  for listener in key_event_listeners() {
    if listener.handles_event(ev) {
      listener.notify(ev);
    }
  }
}
