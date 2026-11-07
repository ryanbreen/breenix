//! Shared userspace UI test client (docs/gui/plan.md section 8).
mod client;
mod executor;
pub mod outcome;

pub use client::RecordClient;
pub use executor::{run_scenario, ScenarioSpec};
pub use outcome::{classify, Classification, Expected, FailureClass, ScenarioOutcome};
