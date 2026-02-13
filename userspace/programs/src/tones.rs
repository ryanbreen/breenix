//! Tones - Musical rising tone demo for Breenix
//!
//! Plays a C major pentatonic ascending run through the VirtIO sound device.
//! Uses fixed-point integer math for synthesis (no floats).

use libbreenix::audio;

/// Fixed-point scale (16 bits fractional)
const FP_SHIFT: u32 = 16;
const FP_ONE: u32 = 1 << FP_SHIFT;

/// 256-entry sine table, amplitude 32767 (precomputed)
/// sin(i * 2*PI / 256) * 32767
const SINE_TABLE: [i16; 256] = [
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

/// Look up sine value using fixed-point phase (0..FP_ONE maps to 0..2*PI)
fn sine_fp(phase: u32) -> i16 {
    // phase is 0..FP_ONE representing 0..2*PI
    // Map to table index 0..255
    let idx = ((phase >> (FP_SHIFT - 8)) & 0xFF) as usize;
    SINE_TABLE[idx]
}

/// Note definition
struct Note {
    /// Frequency in Hz (fixed-point: freq * 256)
    freq_fp256: u32,
    /// Duration in samples
    duration_samples: u32,
}

/// ADSR envelope parameters (in samples)
const ATTACK_SAMPLES: u32 = 661;    // ~15ms at 44100 Hz
const RELEASE_SAMPLES: u32 = 2646;  // ~60ms at 44100 Hz

/// Apply ADSR envelope (returns amplitude 0..256)
fn envelope(sample_idx: u32, total_samples: u32) -> u32 {
    if sample_idx < ATTACK_SAMPLES {
        // Attack: ramp up
        (sample_idx * 256) / ATTACK_SAMPLES
    } else if sample_idx >= total_samples.saturating_sub(RELEASE_SAMPLES) {
        // Release: ramp down
        let remaining = total_samples.saturating_sub(sample_idx);
        (remaining * 256) / RELEASE_SAMPLES
    } else {
        // Sustain
        256
    }
}

fn main() {
    println!("Tones: C major pentatonic rising scale");

    // Initialize audio
    if let Err(_) = audio::init() {
        eprintln!("tones: audio init failed");
        std::process::exit(1);
    }

    println!("Audio initialized. Playing tones...");

    // C major pentatonic ascending: C4, E4, G4, A4, C5, E5, G5
    // Frequencies * 256 for fixed-point
    let notes = [
        Note { freq_fp256: 261 * 256 + 161, duration_samples: 44100 * 280 / 1000 },  // C4  261.63 Hz, 280ms
        Note { freq_fp256: 329 * 256 + 161, duration_samples: 44100 * 280 / 1000 },  // E4  329.63 Hz, 280ms
        Note { freq_fp256: 392 * 256 + 0,   duration_samples: 44100 * 280 / 1000 },  // G4  392.00 Hz, 280ms
        Note { freq_fp256: 440 * 256 + 0,   duration_samples: 44100 * 280 / 1000 },  // A4  440.00 Hz, 280ms
        Note { freq_fp256: 523 * 256 + 64,  duration_samples: 44100 * 280 / 1000 },  // C5  523.25 Hz, 280ms
        Note { freq_fp256: 659 * 256 + 64,  duration_samples: 44100 * 280 / 1000 },  // E5  659.25 Hz, 280ms
        Note { freq_fp256: 783 * 256 + 253, duration_samples: 44100 * 600 / 1000 },  // G5  783.99 Hz, 600ms
    ];

    // Generate and play each note
    for (note_idx, note) in notes.iter().enumerate() {
        // Phase increment per sample = freq * FP_ONE / SAMPLE_RATE
        // freq is in fixed-point * 256, so: (freq_fp256 * FP_ONE) / (256 * 44100)
        let phase_inc1 = (note.freq_fp256 as u64 * FP_ONE as u64 / (256 * 44100)) as u32;
        // Detuned by ~5 cents: multiply by 1.003 ≈ add 0.3%
        let phase_inc2 = phase_inc1 + (phase_inc1 / 333);
        // 2nd harmonic
        let phase_inc_harm = phase_inc1 * 2;

        // Phase accumulators (reset for each note)
        let mut phase1: u32 = 0;
        let mut phase2: u32 = 0;
        let mut phase_harm: u32 = 0;

        let mut samples_written: u32 = 0;
        let chunk_frames = 1024;
        let mut buf = [0i16; 1024 * 2]; // stereo

        while samples_written < note.duration_samples {
            let remaining = note.duration_samples - samples_written;
            let frames = core::cmp::min(chunk_frames, remaining);

            for i in 0..frames as usize {
                let sample_idx = samples_written + i as u32;

                // Oscillator 1 (fundamental)
                let osc1 = sine_fp(phase1) as i32;
                // Oscillator 2 (detuned for chorus)
                let osc2 = sine_fp(phase2) as i32;
                // 2nd harmonic at 25% amplitude
                let harm = sine_fp(phase_harm) as i32 / 4;

                // Mix: (osc1 + osc2) / 2 + harmonic
                let mixed = (osc1 + osc2) / 2 + harm;

                // Apply envelope
                let env = envelope(sample_idx, note.duration_samples);
                let sample = ((mixed as i64 * env as i64) / 256) as i32;

                // Clamp to i16 range
                let sample = sample.max(-32767).min(32767) as i16;

                // Stereo (same on both channels)
                buf[i * 2] = sample;
                buf[i * 2 + 1] = sample;

                phase1 = phase1.wrapping_add(phase_inc1) % FP_ONE;
                phase2 = phase2.wrapping_add(phase_inc2) % FP_ONE;
                phase_harm = phase_harm.wrapping_add(phase_inc_harm) % FP_ONE;
            }

            match audio::write_samples(&buf[..frames as usize * 2]) {
                Ok(_) => {}
                Err(_) => {
                    println!("Error: audio write failed");
                    break;
                }
            }

            samples_written += frames;
        }

        // Print note info
        let note_names = ["C4", "E4", "G4", "A4", "C5", "E5", "G5"];
        if note_idx < note_names.len() {
            println!("  Played {}", note_names[note_idx]);
        }
    }

    println!("Tones complete!");
}
