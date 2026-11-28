//! Host model of the park contract (E2.2, docs/plans/e2-sleepable-contexts.md
//! §3.1-§3.4).
//!
//! `park_core` and `event_queue` are the production files, compiled here
//! unchanged. `model` is a recording scheduler and CPU behind their hook
//! traits. The tests drive production `wait`, `wait_any`, `classify` and
//! `Braked`, and place wakes, signals, kills and deadlines at each point of a
//! wait through the `Point` seam.

extern crate alloc;

#[path = "../../../kernel/src/task/event_queue.rs"]
pub mod event_queue;

#[path = "../../../kernel/src/task/park_core.rs"]
pub mod park_core;

pub mod model;

/// E2.1's allocation budget, so a failed reservation runs the real
/// `try_reserve` failure.
#[cfg(test)]
#[path = "../../waitqueue-host-tests/src/budget.rs"]
mod budget;

#[cfg(test)]
mod tests;
