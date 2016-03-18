
use alloc::boxed::Box;
use collections::vec::Vec;
use core::intrinsics;
use core::ptr;
use core::ptr::Unique;

use event::IsListener;
use event::keyboard::KeyEvent;

struct State {
  key_listeners: Vec<Box<IsListener<KeyEvent>>>,
}

static mut STATE_PTR:Unique<State> = unsafe { Unique::new(0xfff000 as * mut _) };

fn key_event_listeners() -> &'static mut Vec<Box<IsListener<KeyEvent>>> {
  unsafe {
    return &mut(STATE_PTR.get_mut().key_listeners);
  }
}

pub fn register_key_event_listener(listener: Box<IsListener<KeyEvent>>) {
  unsafe {
    let mut listeners:&'static mut Vec<Box<IsListener<KeyEvent>>> = key_event_listeners();
    listeners.push(listener);
    println!("There are now {} key listeners", listeners.len());
  }
}

pub fn dispatch_key_event(ev: &KeyEvent) {
  for listener in key_event_listeners() {
    if listener.handles_event(ev) {
      listener.notify(ev);
    }
  }
}
