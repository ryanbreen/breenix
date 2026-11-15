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
    clock: Option<[u64; 4]>,
    costs: Option<[[u64; 3]; 2]>,
}

impl ModeAttempt {
    pub(crate) fn new(index: u32, phase: u32, now: u64, state: [u64; 4]) -> Self {
        let mut record = Self { index, steps: [Step { phase: 0, elapsed_ns: 0, state: [0; 4] }; 24],
            count: 0, dropped: 0, start: now, clock: None, costs: None };
        record.phase(phase, now, state);
        record
    }

    pub(crate) fn clock_observation(&mut self, counts: [u64; 4]) { self.clock = Some(counts); }

    pub(crate) fn clock_costs(&mut self, costs: [[u64; 3]; 2]) { self.costs = Some(costs); }

    pub(crate) fn annotate_refusal(&self, report: &mut gpu_abi::ModeReport) {
        struct Reason<'a>(&'a mut gpu_abi::ModeReport);
        impl fmt::Write for Reason<'_> {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                let start = self.0.reason_len as usize;
                let end = start + text.len();
                if end > self.0.reason_bytes.len() { return Err(fmt::Error); }
                self.0.reason_bytes[start..end].copy_from_slice(text.as_bytes());
                self.0.reason_len = end as u32;
                Ok(())
            }
        }
        if let Some(c) = self.clock {
            use fmt::Write;
            let _ = write!(Reason(report), " samples={} advances={} rejected={} max_gap_ns={}", c[0], c[1], c[2], c[3]);
        }
        if let Some([v, f]) = self.costs {
            use fmt::Write;
            let _ = write!(Reason(report), " vert_ns={}/{}/{} full_ns={}/{}/{}", v[0], v[1], v[2], f[0], f[1], f[2]);
        }
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
        write!(out, "] dropped={}", self.dropped)?;
        if let Some(c) = self.clock {
            write!(out, " samples={} advances={} rejected={} max_gap_ns={}", c[0], c[1], c[2], c[3])?;
        }
        if let Some([v, f]) = self.costs {
            write!(out, " vert_ns={}/{}/{} full_ns={}/{}/{}", v[0], v[1], v[2], f[0], f[1], f[2])?;
        }
        Ok(())
    }
}
