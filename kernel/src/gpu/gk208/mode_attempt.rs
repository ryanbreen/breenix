//! Ordered timings for one native mode switch, formatted after device guards drop.
use core::fmt;
use gpu_abi::Plain;

#[derive(Clone, Copy)]
struct Step {
    phase: u32,
    elapsed_ns: u64,
    state: [u64; 4],
}

pub(crate) struct ModeAttempt {
    index: u32,
    // Bound storage and formatting; report any overflow without allocating here.
    steps: [Step; 24],
    count: usize,
    dropped: usize,
    start: u64,
}

impl ModeAttempt {
    pub(crate) fn new(index: u32, phase: u32, now: u64, state: [u64; 4]) -> Self {
        let mut record = Self { index, steps: [Step { phase: 0, elapsed_ns: 0, state: [0; 4] }; 24],
            count: 0, dropped: 0, start: now };
        record.phase(phase, now, state);
        record
    }

    pub(crate) fn phase(&mut self, phase: u32, now: u64, state: [u64; 4]) {
        self.finish(now);
        if self.count < self.steps.len() {
            self.steps[self.count] = Step { phase, elapsed_ns: 0, state };
            self.count += 1;
        } else {
            self.dropped += 1;
        }
        self.start = now;
    }

    pub(crate) fn finish(&mut self, now: u64) {
        if self.count != 0 && self.dropped == 0 {
            self.steps[self.count - 1].elapsed_ns = now.saturating_sub(self.start);
        }
    }
}

impl fmt::Display for ModeAttempt {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "index={} phases=[", self.index)?;
        for (i, step) in self.steps[..self.count].iter().enumerate() {
            let name = gpu_abi::ModeReport { phase: step.phase, ..gpu_abi::ModeReport::ZERO };
            write!(out, "{}{}:{}:{}:{}:{}:{}", if i == 0 { "" } else { "," },
                name.phase_name(), step.elapsed_ns, step.state[0], step.state[1], step.state[2], step.state[3])?;
        }
        write!(out, "] dropped={}", self.dropped)
    }
}
