
use alloc::boxed::Box;
use collections::vec::Vec;
use core::intrinsics;
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

fn key_event_listeners() -> &'static mut Vec<Box<IsListener<KeyEvent>>> {
  unsafe {
    if KEY_EVENT_LISTENERS_PTR == 0 {
      panic!("Key Events uninitialized");
    }

    let pointer = KEY_EVENT_LISTENERS_PTR as *mut Vec<Box<IsListener<KeyEvent>>>;
    return &mut (*pointer);
  }
}

pub fn set_key_event_listener(listeners: &'static mut Vec<Box<IsListener<KeyEvent>>>) {
  unsafe {
    KEY_EVENT_LISTENERS_PTR = listeners as *mut _ as usize;
  }
}

pub fn register_key_event_listener(listener: Box<IsListener<KeyEvent>>) {

  let mut listeners:&'static mut Vec<Box<IsListener<KeyEvent>>> = key_event_listeners();
  println!("Got listeners: {}", listeners.len());

  listeners.push(listener);
}

pub fn initialize() {
  unsafe {
    let listeners:Vec<Box<IsListener<KeyEvent>>> = Vec::new();

    #[allow(mutable_transmutes)]
    let mut static_listeners:&'static mut Vec<Box<IsListener<KeyEvent>>> =
      intrinsics::transmute(&listeners);
    set_key_event_listener(static_listeners);
    println!("Got listeners: {}", key_event_listeners().len());
  }
}

pub fn dispatch_key_event(ev: &KeyEvent) {
  for listener in key_event_listeners() {
    if listener.handles_event(ev) {
      listener.notify(ev);
    }
  }
}
