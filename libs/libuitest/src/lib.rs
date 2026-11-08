//! Shared userspace UI test client (docs/gui/plan.md section 8).
mod client;
mod executor;
pub mod outcome;

pub use client::RecordClient;
pub use executor::{run_scenario, ScenarioSpec};
pub use outcome::{classify, Classification, Expected, FailureClass, ScenarioOutcome};

pub mod supervisor;
mod channel;
mod session;
pub use session::{Session, ProcessExit, AppStderr};
mod window_wait;
pub use window_wait::WindowReady;
pub mod fixtures;

pub mod arena;
pub mod artifacts;
#[cfg(target_os = "breenix")]
mod collect;

pub mod startup;

pub mod schema;

pub mod compare;

pub mod formula;

pub mod frames;
