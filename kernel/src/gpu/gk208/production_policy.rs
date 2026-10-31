//! Production clock policy (K8). Hardware mechanisms live in the shared C3-C5 helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage { Sensor, Pmu, Ram, MemoryIntermediate, MemoryTarget, Gpc, Verify }
impl Stage {
    pub const ALL: [Self; 7] = [Self::Sensor, Self::Pmu, Self::Ram,
        Self::MemoryIntermediate, Self::MemoryTarget, Self::Gpc, Self::Verify];
}
pub trait Startup {
    fn step(&mut self, stage: Stage) -> Result<(), &'static str>;
    fn restore_gpc(&mut self) -> Result<(), &'static str>;
    fn restore_memory(&mut self) -> Result<(), &'static str>;
}
pub struct Outcome {
    pub full: bool,
    pub failure: Option<&'static str>,
    pub gpc_restore: Option<&'static str>,
    pub memory_restore: Option<&'static str>,
}
pub fn start(io: &mut impl Startup, keep_post: bool) -> Outcome {
    let mut out = Outcome { full: false, failure: None, gpc_restore: None, memory_restore: None };
    if keep_post { return out; }
    let mut memory = false;
    let mut gpc = false;
    for stage in Stage::ALL {
        memory |= stage == Stage::MemoryIntermediate;
        gpc |= stage == Stage::Gpc;
        if let Err(reason) = io.step(stage) {
            out.failure = Some(reason);
            // Attempt the second domain even when the first restore fails.
            if gpc { out.gpc_restore = io.restore_gpc().err(); }
            if memory { out.memory_restore = io.restore_memory().err(); }
            return out;
        }
    }
    out.full = true;
    out
}
pub fn within_one_percent(value: Result<u32, &'static str>, target: u32) -> bool {
    value.is_ok_and(|v| v.abs_diff(target) <= target / 100)
}
pub struct Guard { tripped: bool }
impl Guard {
    pub const fn new() -> Self { Self { tripped: false } }
    pub fn sample(&mut self, temperature: Result<u32, &'static str>) -> bool {
        if self.tripped { return false; }
        if temperature.is_err() || temperature.is_ok_and(|v| v >= 85) {
            self.tripped = true;
            return true;
        }
        false
    }
    pub fn tripped(&self) -> bool { self.tripped }
}
