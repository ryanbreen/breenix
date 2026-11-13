//! Immediate, size-bounded phase records for multi-checkpoint scenarios.
use std::{cell::RefCell, io::Write, time::Instant};

const LIMIT: usize = 8192;
struct State {
    case: &'static str,
    checkpoint: u32,
    reacquires: u64,
    bytes: usize,
    limited: bool,
}
thread_local! { static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) }; }

/// A scenario opts into ordinary phase reporting for its whole execution.
pub struct Scenario;
impl Scenario {
    pub fn new(case: &'static str) -> Self {
        ACTIVE.with(|active| *active.borrow_mut() = Some(State {
            case, checkpoint: 0, reacquires: 0, bytes: 0, limited: false,
        }));
        Self
    }
}
impl Drop for Scenario {
    fn drop(&mut self) { ACTIVE.with(|active| *active.borrow_mut() = None); }
}

pub fn checkpoint() -> Phase {
    ACTIVE.with(|active| if let Some(state) = active.borrow_mut().as_mut() {
        state.checkpoint = state.checkpoint.saturating_add(1);
        state.reacquires = 0;
    });
    Phase::new("checkpoint")
}
pub fn reacquire() {
    ACTIVE.with(|active| if let Some(state) = active.borrow_mut().as_mut() {
        state.reacquires = state.reacquires.saturating_add(1);
    });
    emit("reacquire", 0);
}

pub struct Phase { name: &'static str, start: Option<Instant> }
impl Phase {
    pub fn new(name: &'static str) -> Self {
        let start = ACTIVE.with(|active| active.borrow().is_some().then(Instant::now));
        Self { name, start }
    }
    /// Explicit completion: a killed case leaves its last completed phase visible,
    /// without a destructor claiming that an interrupted phase finished.
    pub fn finish(self) {
        if let Some(start) = self.start {
            let us = start.elapsed().as_nanos().div_ceil(1000).min(u64::MAX as u128) as u64;
            emit(self.name, us);
        }
    }
}

fn emit(name: &str, us: u64) {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(state) = active.as_mut() else { return };
        if state.limited { return; }
        let mut line = format!("VALUE wm-phase {} {} {name} {}.{:03} {}\n",
            state.case, state.checkpoint, us / 1000, us % 1000, state.reacquires);
        // Reserve space for an explicit truncation record instead of silently
        // turning a busy retry path into an unbounded serial export.
        if state.bytes + line.len() > LIMIT - 128 {
            line = format!("VALUE wm-phase {} {} limit 0.000 {}\n",
                state.case, state.checkpoint, state.reacquires);
            state.limited = true;
        }
        if state.bytes + line.len() > LIMIT { return; }
        state.bytes += line.len();
        if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
            let _ = evidence.write_all(line.as_bytes());
        }
    });
}
