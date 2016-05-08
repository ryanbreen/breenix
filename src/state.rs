use alloc::boxed::Box;
use collections::vec::Vec;

use event::IsListener;
use event::keyboard::KeyEvent;

use task::scheduler::Scheduler;

pub struct State {
  pub key_listeners: Vec<Box<IsListener<KeyEvent>>>,
  pub interrupt_count: [u64; 256],
  pub scheduler: Scheduler,
}

impl State {
  fn new() -> Box<State> {
    box State {
      key_listeners: Vec::new(),
      interrupt_count: [0; 256],
      scheduler: Scheduler::new(),
    }
  }
}

static mut STATE_PTR: Option<&'static mut State> = None;

pub fn state() -> &'static mut State {
  unsafe {
    match STATE_PTR {
      Some(ref mut p) => p,
      None => {
        STATE_PTR = Some(&mut *Box::into_raw(State::new()));
        match STATE_PTR {
          Some(ref mut s) => {
            /// Do any sort of state init we need beyond new().
            s
          },
          None => {
            panic!("Failed to init state");
          }
        }
      },
    }
  }
}

pub fn register_key_event_listener(listener: Box<IsListener<KeyEvent>>) {
  state().key_listeners.push(listener);
  println!("There are now {} key listeners", state().key_listeners.len());
}

pub fn dispatch_key_event(ev: &KeyEvent) {

  let listeners = &(state().key_listeners);
  for listener in listeners {
    if listener.handles_event(ev) {
      listener.notify(ev);
    }
  }
}
