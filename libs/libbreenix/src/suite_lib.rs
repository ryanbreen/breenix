//! Guest suite harness, separate from the production syscall library.
//! CPU reference drawing supplies independent expectations only in suite binaries.

use libbreenix::{compositor, errno, error, fs, gpu, graphics, io, process, signal, syscall, test_control, time, types, ui_clock, window};

pub mod suite;
mod suite_stage;
pub mod suite_spawn;
