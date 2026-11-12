//! Shared userspace UI test client (docs/gui/plan.md section 8).
mod client;
#[cfg(target_os = "breenix")]
mod executor;
pub mod outcome;

pub use client::RecordClient;
#[cfg(target_os = "breenix")]
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
pub mod messages;

pub mod capture_marker;
pub mod capture;
pub mod phases;

pub mod capture_race;
pub mod tree;
pub mod input;

pub mod clock;
pub mod settle;

#[cfg(target_os = "breenix")]
pub mod retention;

pub mod app;
#[cfg(target_os = "breenix")]
pub use app::App;
