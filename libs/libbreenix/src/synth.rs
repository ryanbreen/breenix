//! Audio synthesis and DSP primitives.
//!
//! Provides building blocks for sound synthesis: waveform generation,
//! digital filters, and noise generators. All integer math, no allocations,
//! no floating point — suitable for real-time audio in a no_std environment.
//!
//! # Fixed-point conventions
//!
//! - Phase accumulators: 16.16 fixed-point, where `FP_ONE` = one full cycle
//! - Audio samples: i16 range (-32767..32767)
//! - Filter coefficients: 20-bit fractional in i64

use crate::audio::SAMPLE_RATE;

/// Fixed-point shift for phase accumulators (16 bits fractional).
pub const FP_SHIFT: u32 = 16;

/// One full cycle in fixed-point phase units.
pub const FP_ONE: u32 = 1 << FP_SHIFT;

/// Fixed-point shift for filter coefficients (20 bits fractional).
const COEFF_SHIFT: u32 = 20;

/// 1.0 in coefficient fixed-point scale.
const COEFF_ONE: i64 = 1 << COEFF_SHIFT;

/// 256-entry sine table, amplitude 32767 (full i16 positive range).
///
/// Covers one complete cycle: index 0 = 0°, 64 = 90°, 128 = 180°, 192 = 270°.
pub static SINE_TABLE: [i16; 256] = [
         0,    804,   1608,   2410,   3212,   4011,   4808,   5602,
      6393,   7179,   7962,   8739,   9512,  10278,  11039,  11793,
     12539,  13279,  14010,  14732,  15446,  16151,  16846,  17530,
     18204,  18868,  19519,  20159,  20787,  21403,  22005,  22594,
     23170,  23731,  24279,  24811,  25329,  25832,  26319,  26790,
     27245,  27683,  28105,  28510,  28898,  29268,  29621,  29956,
     30273,  30571,  30852,  31113,  31356,  31580,  31785,  31971,
     32137,  32285,  32412,  32521,  32609,  32678,  32728,  32757,
     32767,  32757,  32728,  32678,  32609,  32521,  32412,  32285,
     32137,  31971,  31785,  31580,  31356,  31113,  30852,  30571,
     30273,  29956,  29621,  29268,  28898,  28510,  28105,  27683,
     27245,  26790,  26319,  25832,  25329,  24811,  24279,  23731,
     23170,  22594,  22005,  21403,  20787,  20159,  19519,  18868,
     18204,  17530,  16846,  16151,  15446,  14732,  14010,  13279,
     12539,  11793,  11039,  10278,   9512,   8739,   7962,   7179,
      6393,   5602,   4808,   4011,   3212,   2410,   1608,    804,
         0,   -804,  -1608,  -2410,  -3212,  -4011,  -4808,  -5602,
     -6393,  -7179,  -7962,  -8739,  -9512, -10278, -11039, -11793,
    -12539, -13279, -14010, -14732, -15446, -16151, -16846, -17530,
    -18204, -18868, -19519, -20159, -20787, -21403, -22005, -22594,
    -23170, -23731, -24279, -24811, -25329, -25832, -26319, -26790,
    -27245, -27683, -28105, -28510, -28898, -29268, -29621, -29956,
    -30273, -30571, -30852, -31113, -31356, -31580, -31785, -31971,
    -32137, -32285, -32412, -32521, -32609, -32678, -32728, -32757,
    -32767, -32757, -32728, -32678, -32609, -32521, -32412, -32285,
    -32137, -31971, -31785, -31580, -31356, -31113, -30852, -30571,
    -30273, -29956, -29621, -29268, -28898, -28510, -28105, -27683,
    -27245, -26790, -26319, -25832, -25329, -24811, -24279, -23731,
    -23170, -22594, -22005, -21403, -20787, -20159, -19519, -18868,
    -18204, -17530, -16846, -16151, -15446, -14732, -14010, -13279,
    -12539, -11793, -11039, -10278,  -9512,  -8739,  -7962,  -7179,
     -6393,  -5602,  -4808,  -4011,  -3212,  -2410,  -1608,   -804,
];

// =========================================================================
// Waveform generation
// =========================================================================

/// Look up sine value from a fixed-point phase (0..FP_ONE = one cycle).
///
/// Returns a value in -32767..32767.
#[inline]
pub fn sine(phase: u32) -> i16 {
    let idx = ((phase >> (FP_SHIFT - 8)) & 0xFF) as usize;
    SINE_TABLE[idx]
}

/// Compute the phase increment per sample for a given frequency in Hz.
///
/// Usage: `phase = phase.wrapping_add(freq_to_inc(440)) % FP_ONE;`
#[inline]
pub fn freq_to_inc(freq_hz: u32) -> u32 {
    (freq_hz as u64 * FP_ONE as u64 / SAMPLE_RATE as u64) as u32
}

// =========================================================================
// Biquad digital filter
// =========================================================================

/// Compute sin(2*pi*freq/sample_rate) in coefficient fixed-point scale.
///
/// Uses the sine table with linear interpolation for adequate precision
/// at audio filter frequencies.
fn sin_w0(freq: u32) -> i64 {
    // Table index = freq * 256 / sample_rate, in 16.16 fixed-point
    let index_fp16 = (freq as u64 * 256 * 65536 / SAMPLE_RATE as u64) as u32;
    let idx = (index_fp16 >> 16) as usize;
    let frac = (index_fp16 & 0xFFFF) as i64;

    let s0 = SINE_TABLE[idx & 0xFF] as i64;
    let s1 = SINE_TABLE[(idx + 1) & 0xFF] as i64;
    let interp = s0 + (s1 - s0) * frac / 65536;

    // Scale from -32767..32767 to COEFF_ONE scale
    interp * COEFF_ONE / 32767
}

/// Compute cos(2*pi*freq/sample_rate) in coefficient fixed-point scale.
fn cos_w0(freq: u32) -> i64 {
    // Same as sin but offset by 64 entries (90 degrees)
    let index_fp16 = (freq as u64 * 256 * 65536 / SAMPLE_RATE as u64) as u32;
    let idx = ((index_fp16 >> 16) + 64) as usize;
    let frac = (index_fp16 & 0xFFFF) as i64;

    let s0 = SINE_TABLE[idx & 0xFF] as i64;
    let s1 = SINE_TABLE[(idx + 1) & 0xFF] as i64;
    let interp = s0 + (s1 - s0) * frac / 65536;

    interp * COEFF_ONE / 32767
}

/// Second-order IIR (biquad) digital filter.
///
/// Implements the standard Direct Form I difference equation:
///
/// ```text
/// y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
/// ```
///
/// Coefficients are stored pre-normalized (divided by a0) in 20-bit
/// fixed-point. State variables use i64 to handle resonant gain > 1.0.
pub struct Biquad {
    b0: i64,
    b1: i64,
    b2: i64,
    a1: i64,
    a2: i64,
    x1: i64,
    x2: i64,
    y1: i64,
    y2: i64,
}

impl Biquad {
    /// Create a resonant bandpass filter.
    ///
    /// - `center_freq`: Center frequency in Hz
    /// - `q_x256`: Q factor * 256 (e.g., Q=5.0 → pass 1280)
    ///
    /// At the center frequency, the filter passes signal through with unity
    /// gain. Frequencies away from center are attenuated. Higher Q = narrower
    /// bandwidth, sharper resonance.
    pub fn bandpass(center_freq: u32, q_x256: u32) -> Self {
        let sw = sin_w0(center_freq);
        let cw = cos_w0(center_freq);

        // alpha = sin(w0) / (2*Q) where Q = q_x256/256
        let alpha = sw * 128 / q_x256.max(1) as i64;

        let a0 = COEFF_ONE + alpha;

        // Normalize all coefficients by a0
        let b0 = alpha * COEFF_ONE / a0;
        let b1 = 0;
        let b2 = -alpha * COEFF_ONE / a0;
        let a1 = -2 * cw * COEFF_ONE / a0;
        let a2 = (COEFF_ONE - alpha) * COEFF_ONE / a0;

        Biquad { b0, b1, b2, a1, a2, x1: 0, x2: 0, y1: 0, y2: 0 }
    }

    /// Create a second-order low-pass filter.
    ///
    /// - `cutoff`: Cutoff frequency in Hz (-3dB point)
    /// - `q_x256`: Q factor * 256 (e.g., Butterworth Q=0.707 → pass 181)
    ///
    /// Passes frequencies below cutoff, attenuates above at 12 dB/octave.
    pub fn lowpass(cutoff: u32, q_x256: u32) -> Self {
        let sw = sin_w0(cutoff);
        let cw = cos_w0(cutoff);

        let alpha = sw * 128 / q_x256.max(1) as i64;
        let one_minus_cos = COEFF_ONE - cw;

        let a0 = COEFF_ONE + alpha;

        let b0 = (one_minus_cos / 2) * COEFF_ONE / a0;
        let b1 = one_minus_cos * COEFF_ONE / a0;
        let b2 = (one_minus_cos / 2) * COEFF_ONE / a0;
        let a1 = -2 * cw * COEFF_ONE / a0;
        let a2 = (COEFF_ONE - alpha) * COEFF_ONE / a0;

        Biquad { b0, b1, b2, a1, a2, x1: 0, x2: 0, y1: 0, y2: 0 }
    }

    /// Process a single sample through the filter.
    ///
    /// Call this once per sample at the audio sample rate.
    #[inline]
    pub fn process(&mut self, input: i16) -> i16 {
        let x = input as i64;

        let y = (self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1 - self.a2 * self.y2)
            >> COEFF_SHIFT;

        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;

        y.max(-32767).min(32767) as i16
    }

    /// Reset filter state to zero (silence).
    pub fn reset(&mut self) {
        self.x1 = 0;
        self.x2 = 0;
        self.y1 = 0;
        self.y2 = 0;
    }
}

// =========================================================================
// Noise generator
// =========================================================================

/// Pseudo-random noise generator with optional spectral shaping.
///
/// Uses a 64-bit LCG (linear congruential generator) for fast, deterministic
/// noise suitable for audio synthesis. Not cryptographically secure.
pub struct Noise {
    state: u64,
    /// 1-pole low-pass filter state for brown noise
    lp_state: i64,
    /// LP filter coefficient (0..256, higher = more highs pass through)
    lp_alpha: i64,
}

impl Noise {
    /// Create a white noise generator (flat spectrum).
    pub fn white(seed: u64) -> Self {
        Noise {
            state: seed.wrapping_add(1),
            lp_state: 0,
            lp_alpha: 256, // bypass filter (alpha=1.0)
        }
    }

    /// Create a brown noise generator (low-pass filtered, -6 dB/octave).
    ///
    /// - `seed`: PRNG seed
    /// - `cutoff_hz`: Approximate cutoff frequency
    ///
    /// Brown noise sounds like a low rumble — good for wind, turbulence, etc.
    pub fn brown(seed: u64, cutoff_hz: u32) -> Self {
        // 1-pole LP: alpha ≈ 2*pi*fc/fs, scaled to 0..256
        // 300 Hz -> alpha ≈ 0.043 -> ~11/256
        let alpha = (cutoff_hz as u64 * 256 * 2 * 314 / (SAMPLE_RATE as u64 * 100))
            .min(256) as i64;
        Noise {
            state: seed.wrapping_add(1),
            lp_state: 0,
            lp_alpha: alpha.max(1),
        }
    }

    /// Generate the next raw PRNG value (full 32-bit range).
    #[inline]
    fn next_raw(&mut self) -> u32 {
        self.state = self.state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 33) as u32
    }

    /// Generate the next noise sample.
    ///
    /// For white noise, returns full-spectrum random values.
    /// For brown noise, the output is low-pass filtered.
    /// Returns a value in approximately -32767..32767.
    #[inline]
    pub fn sample(&mut self) -> i16 {
        // Raw white noise in full i16 range
        let raw = (self.next_raw() as i32 >> 16) as i64; // -32768..32767

        if self.lp_alpha >= 256 {
            // White: no filtering
            raw as i16
        } else {
            // Brown: 1-pole IIR low-pass
            // y[n] = alpha * x[n] + (1-alpha) * y[n-1]
            self.lp_state = (self.lp_alpha * raw
                + (256 - self.lp_alpha) * self.lp_state)
                / 256;
            // Scale up to compensate for LP gain reduction
            (self.lp_state * 4).max(-32767).min(32767) as i16
        }
    }
}
