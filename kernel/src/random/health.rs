//! Health tests and credit for the timing-jitter source, from NIST SP 800-90B
//! sections 4.3 and 4.4, on one-byte raw samples.
//!
//! The source is assessed at `ASSESSED_BITS` of min-entropy per sample, and the
//! cutoffs are the document's for that rate and a false-positive probability
//! of alpha = 2^-20:
//!
//! - Repetition count test (4.4.1): C = 1 + ceil(-log2(alpha) / H), 11 for
//!   H = 2 (the document's own example). A run of C identical samples fails.
//!   The run carries across windows.
//! - Adaptive proportion test (4.4.2): W = 512 for non-binary samples,
//!   C = 1 + CRITBINOM(W, 2^-H, 1 - alpha), 177 for H = 2 (Table 2). The test
//!   fails when the window's first sample occurs C times in the window.
//! - A developer-defined test (4.3 item 8, 4.5) for the failure the document
//!   names, periodic output like 101...01: a window also fails when any value
//!   occurs C times in it. A source with two values has one at half the window
//!   or more, so a clock that alternates or repeats a short pattern fails
//!   whichever value starts the window.
//!
//! Credit (`Credit`): the start-up tests (4.3 item 4) are these tests over the
//! first `STARTUP_SAMPLES`, and credit waits until they pass. A failure is
//! treated as persistent (4.3 item 2): the source earns no more credit after
//! it. A passing window earns `WINDOW_CREDIT_BITS`.
//!
//! Pure computation, so the host tests compile this file as is.

/// Assessed min-entropy per raw sample, in bits; the cutoffs below are for it.
pub const ASSESSED_BITS: u32 = 2;
/// Repetition count cutoff: 1 + ceil(20 / 2).
pub const RCT_CUTOFF: u32 = 11;
/// Adaptive proportion window for non-binary samples.
pub const WINDOW: u32 = 512;
/// Adaptive proportion cutoff: 1 + CRITBINOM(512, 2^-2, 1 - 2^-20).
pub const APT_CUTOFF: u32 = 177;
/// Samples the start-up tests cover before anything is credited.
pub const STARTUP_SAMPLES: u32 = 1024;
/// Credit for a window that passes: a sixteenth of the assessed rate, 1/8 bit
/// per sample.
pub const WINDOW_CREDIT_BITS: u32 = WINDOW * ASSESSED_BITS / 16;

/// What a finished window showed, and whether it passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowVerdict {
    /// Longest run of identical samples reached inside the window (a run that
    /// began in an earlier window counts from its start).
    pub longest_run: u32,
    /// Occurrences of the window's first sample.
    pub first_count: u32,
    /// Occurrences of the window's most common value.
    pub most_common: u32,
}

impl WindowVerdict {
    pub fn repetition_failed(&self) -> bool {
        self.longest_run >= RCT_CUTOFF
    }

    pub fn proportion_failed(&self) -> bool {
        self.first_count >= APT_CUTOFF
    }

    pub fn common_value_failed(&self) -> bool {
        self.most_common >= APT_CUTOFF
    }

    pub fn passed(&self) -> bool {
        !(self.repetition_failed() || self.proportion_failed() || self.common_value_failed())
    }
}

pub struct HealthTests {
    last: u8,
    run: u32,
    longest_run: u32,
    reference: u8,
    seen: u32,
    counts: [u16; 256],
}

impl HealthTests {
    pub const fn new() -> Self {
        Self {
            last: 0,
            run: 0,
            longest_run: 0,
            reference: 0,
            seen: 0,
            counts: [0; 256],
        }
    }

    /// Feed one raw sample; the sample that completes a window returns its verdict.
    pub fn feed(&mut self, sample: u8) -> Option<WindowVerdict> {
        if self.run > 0 && sample == self.last {
            self.run += 1;
        } else {
            self.last = sample;
            self.run = 1;
        }
        self.longest_run = self.longest_run.max(self.run);

        if self.seen == 0 {
            self.reference = sample;
        }
        self.counts[sample as usize] += 1;
        self.seen += 1;
        if self.seen < WINDOW {
            return None;
        }

        let verdict = WindowVerdict {
            longest_run: self.longest_run,
            first_count: u32::from(self.counts[self.reference as usize]),
            most_common: u32::from(self.counts.iter().copied().max().unwrap_or(0)),
        };
        self.counts = [0; 256];
        self.seen = 0;
        self.longest_run = 0;
        Some(verdict)
    }
}

/// The source's credit: held through the start-up tests, stopped for good by
/// any failure.
pub struct Credit {
    windows: u32,
    failed: bool,
}

impl Credit {
    pub const fn new() -> Self {
        Self { windows: 0, failed: false }
    }

    /// Record the next window's verdict; returns the bits to credit now.
    pub fn record(&mut self, verdict: &WindowVerdict) -> u32 {
        if self.failed {
            return 0;
        }
        if !verdict.passed() {
            self.failed = true;
            return 0;
        }
        self.windows += 1;
        let startup = STARTUP_SAMPLES / WINDOW;
        if self.windows < startup {
            0
        } else if self.windows == startup {
            startup * WINDOW_CREDIT_BITS
        } else {
            WINDOW_CREDIT_BITS
        }
    }

    /// Whether a failure has stopped the source's credit.
    pub fn failed(&self) -> bool {
        self.failed
    }
}
