//! The jitter source's health tests and credit (kernel/src/random/health.rs)
//! against NIST SP 800-90B: the 4.4.1 example cutoff, Table 2's adaptive
//! proportion cutoffs from the 4.4.2 formula, and the failures the tests exist
//! to catch.

use crate::health::{
    Credit, HealthTests, WindowVerdict, APT_CUTOFF, CUTOFF_REFERENCE_BITS, RCT_CUTOFF, STARTUP_SAMPLES, WINDOW,
    WINDOW_CREDIT_BITS,
};

/// 4.4.1: C = 1 + ceil(-log2(alpha) / H).
fn repetition_cutoff(h: f64, minus_log2_alpha: f64) -> u32 {
    1 + (minus_log2_alpha / h).ceil() as u32
}

/// 4.4.2: C = 1 + CRITBINOM(W, 2^-H, 1 - alpha), the smallest C with
/// Pr(B >= C) <= alpha for B ~ Binomial(W, 2^-H).
fn proportion_cutoff(w: u32, h: f64) -> u32 {
    let p = 2f64.powf(-h);
    let ln_choose = |k: u32| -> f64 {
        (1..=k).map(|i| ((w - k + i) as f64).ln() - (i as f64).ln()).sum()
    };
    let pmf = |k: u32| (ln_choose(k) + k as f64 * p.ln() + (w - k) as f64 * (1.0 - p).ln()).exp();
    let alpha = 2f64.powi(-20);
    let mut tail = 0.0;
    for c in (0..=w).rev() {
        if tail + pmf(c) > alpha {
            return c + 1;
        }
        tail += pmf(c);
    }
    0
}

#[test]
fn repetition_cutoff_is_the_documents_example_for_two_bits() {
    // "for alpha = 2^-20, an entropy source with H = 2.0 bits per sample would
    // have a repetition count test cutoff value of 1+20/2.0 = 11"; eight bits
    // gives six repetitions at a false-positive rate near once per 10^12
    // samples (alpha = 2^-40).
    assert_eq!(repetition_cutoff(2.0, 20.0), 11);
    assert_eq!(repetition_cutoff(8.0, 40.0), 6);
    assert_eq!(CUTOFF_REFERENCE_BITS, 2);
    assert_eq!(RCT_CUTOFF, repetition_cutoff(CUTOFF_REFERENCE_BITS as f64, 20.0));
}

#[test]
fn proportion_cutoffs_match_table_2() {
    for (h, cutoff) in [(0.2, 941), (0.4, 840), (0.6, 748), (0.8, 664), (1.0, 589)] {
        assert_eq!(proportion_cutoff(1024, h), cutoff, "binary W=1024, H={h}");
    }
    for (h, cutoff) in [(0.5, 410), (1.0, 311), (2.0, 177), (4.0, 62), (8.0, 13)] {
        assert_eq!(proportion_cutoff(512, h), cutoff, "non-binary W=512, H={h}");
    }
    // The samples are bytes (non-binary), so the window is 512.
    assert_eq!(WINDOW, 512);
    assert_eq!(APT_CUTOFF, proportion_cutoff(WINDOW, CUTOFF_REFERENCE_BITS as f64));
}

/// Feed `samples` and collect the verdicts of the windows they complete.
fn verdicts(samples: impl IntoIterator<Item = u8>) -> Vec<WindowVerdict> {
    let mut tests = HealthTests::new();
    samples.into_iter().filter_map(|sample| tests.feed(sample)).collect()
}

fn credit_for(verdicts: &[WindowVerdict]) -> u32 {
    let mut credit = Credit::new();
    verdicts.iter().map(|verdict| credit.record(verdict)).sum()
}

/// Predictable fixed-seed xorshift bytes that pass the health diagnostics.
fn uniform(count: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect()
}

#[test]
fn a_stuck_clock_fails_the_repetition_count_test_and_earns_nothing() {
    let windows = verdicts(std::iter::repeat_n(100, 8 * WINDOW as usize));
    assert_eq!(windows.len(), 8);
    assert!(windows.iter().all(|w| w.repetition_failed() && !w.passed()));
    assert_eq!(credit_for(&windows), 0);
}

#[test]
fn an_alternating_clock_fails_the_adaptive_proportion_test_and_earns_nothing() {
    // The review's case: 101, 100, 101, 100, ...
    let windows = verdicts((0..8 * WINDOW).map(|i| if i % 2 == 0 { 101 } else { 100 }));
    for window in &windows {
        assert!(!window.repetition_failed(), "no value repeats");
        assert_eq!(window.first_count, WINDOW / 2);
        assert!(window.proportion_failed() && !window.passed());
    }
    assert_eq!(credit_for(&windows), 0);
}

#[test]
fn a_short_period_starting_on_its_rare_value_fails_the_common_value_test() {
    // 101, 100, 100, 100 repeats; each window starts on 101 (a quarter of the
    // window, under the cutoff), so only the any-value check sees 100 at three quarters.
    let windows = verdicts((0..8 * WINDOW).map(|i| if i % 4 == 0 { 101 } else { 100 }));
    for window in &windows {
        assert_eq!(window.first_count, WINDOW / 4);
        assert!(!window.proportion_failed() && !window.repetition_failed());
        assert_eq!(window.most_common, 3 * WINDOW / 4);
        assert!(window.common_value_failed() && !window.passed());
    }
    assert_eq!(credit_for(&windows), 0);
}

#[test]
fn a_quantized_instruction_counting_clock_fails() {
    // A walk of fixed length timed by a clock that counts instructions: 4096.3
    // ticks per walk, so a sample is 4096 or 4097 depending only on the phase.
    let mut now = 0.0f64;
    let windows = verdicts((0..8 * WINDOW).map(|_| {
        let start = now.floor() as u64;
        now += 4096.3;
        (now.floor() as u64 - start) as u8
    }));
    assert!(windows.iter().all(|w| w.most_common >= WINDOW / 2 && !w.passed()));
    assert_eq!(credit_for(&windows), 0);
}

#[test]
fn passing_diagnostics_never_establish_entropy_credit() {
    let windows = verdicts(uniform(8 * WINDOW as usize));
    assert!(windows.iter().all(WindowVerdict::passed));
    let mut credit = Credit::new();
    let startup = (STARTUP_SAMPLES / WINDOW) as usize;
    assert_eq!(STARTUP_SAMPLES, 1024, "4.3 item 4: at least 1024 consecutive samples");
    let earned: Vec<u32> = windows.iter().map(|w| credit.record(w)).collect();
    assert!(earned[..startup - 1].iter().all(|&bits| bits == 0));
    assert_eq!(earned[startup - 1], startup as u32 * WINDOW_CREDIT_BITS);
    assert!(earned[startup..].iter().all(|&bits| bits == WINDOW_CREDIT_BITS));
    assert_eq!(WINDOW_CREDIT_BITS, 0, "no independent entropy assessment");
    assert!(earned.iter().all(|&bits| bits == 0));
}

#[test]
fn a_failure_is_persistent() {
    let good = verdicts(uniform(4 * WINDOW as usize));
    let stuck = verdicts(std::iter::repeat_n(7, WINDOW as usize));
    let mut credit = Credit::new();
    let before: u32 = good[..3].iter().map(|w| credit.record(w)).sum();
    assert_eq!(before, 3 * WINDOW_CREDIT_BITS);
    assert_eq!(credit.record(&stuck[0]), 0);
    assert!(credit.failed());
    assert_eq!(credit.record(&good[3]), 0, "a passing window after a failure earns nothing");
}

#[test]
fn a_failed_start_up_earns_nothing() {
    let mut samples = uniform(4 * WINDOW as usize);
    samples[..WINDOW as usize].fill(9);
    let windows = verdicts(samples);
    assert!(!windows[0].passed() && windows[1..].iter().all(WindowVerdict::passed));
    assert_eq!(credit_for(&windows), 0);
}

#[test]
fn the_repetition_count_fails_at_its_cutoff_and_runs_across_windows() {
    let mut samples = uniform(2 * WINDOW as usize);
    // Ten identical samples ending the first window and one starting the second: eleven.
    let boundary = WINDOW as usize;
    for sample in &mut samples[boundary - 10..=boundary] {
        *sample = 42;
    }
    samples[boundary - 11] = 41;
    samples[boundary + 1] = 43;
    let windows = verdicts(samples.clone());
    assert_eq!(windows[0].longest_run, 10);
    assert!(!windows[0].repetition_failed());
    assert_eq!(windows[1].longest_run, RCT_CUTOFF);
    assert!(windows[1].repetition_failed());

    // A run that ends at the end of a window is not charged to the next one.
    samples[boundary] = 44;
    let windows = verdicts(samples);
    assert_eq!(windows[0].longest_run, 10);
    assert!(windows[1].longest_run < RCT_CUTOFF);
}

#[test]
fn the_adaptive_proportion_test_fails_at_its_cutoff() {
    for (count, fails) in [(APT_CUTOFF - 1, false), (APT_CUTOFF, true)] {
        let mut samples = uniform(WINDOW as usize);
        // The window's first sample, `count` times in the window, with no two in a row.
        let value = samples[0];
        for sample in samples.iter_mut().skip(1) {
            if *sample == value {
                *sample = value ^ 0x80;
            }
        }
        for slot in 1..count as usize {
            samples[slot * 2] = value;
        }
        let window = verdicts(samples)[0];
        assert_eq!(window.first_count, count);
        assert_eq!(window.proportion_failed(), fails);
        assert!(!window.repetition_failed());
    }
}

#[test]
fn a_predictable_three_value_cycle_passes_but_earns_nothing() {
    let windows = verdicts((0..8 * WINDOW).map(|i| 100 + (i % 3) as u8));
    assert_eq!(windows.len(), 8);
    for window in &windows {
        assert_eq!(window.longest_run, 1);
        assert_eq!(window.most_common, 171);
        assert!(window.passed());
    }
    assert_eq!(credit_for(&windows), 0);
}
