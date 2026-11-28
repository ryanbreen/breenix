//! Host model of the kernel's event queue (E2.1, brief B1).
//!
//! `event_queue` is the production file, compiled here unchanged. `model` is
//! a recording scheduler behind the queue's `Hooks` seam. The tests drive the
//! production queue through the interleavings of a waiter's and a waker's
//! steps, enumerated by `model::interleavings`, and assert brief B1's
//! requirements.

extern crate alloc;

#[path = "../../../kernel/src/task/event_queue.rs"]
pub mod event_queue;

pub mod model;

#[cfg(test)]
mod budget;

#[cfg(test)]
mod tests;
