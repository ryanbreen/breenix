
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
