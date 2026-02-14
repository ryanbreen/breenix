//! Fart Sound Simulator for Breenix
//!
//! Synthesizes realistic fart sounds based on acoustic research of real
//! flatulence (ICEF/flatology.com, Journal of the Acoustical Society).
//!
//! Uses a source-filter model: sphincter pulses (100-200 Hz) are filtered
//! through a resonant bandpass (~270 Hz rectal cavity) and mixed with
//! filtered noise (airflow turbulence).
//!
//! Usage:
//!   fart          # Single fart
//!   fart 3        # Three farts in a row
//!   fart --help   # Show usage

use libbreenix::audio;
use libbreenix::synth::{self, Biquad, Noise, FP_ONE, FP_SHIFT};
use libbreenix::time;

const SAMPLE_RATE: u32 = 44100;

/// Simple LCG PRNG for parameter randomization.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng { state: seed.wrapping_add(1) }
    }

    fn next(&mut self) -> u32 {
        self.state = self.state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 33) as u32
    }

    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if lo >= hi { return lo; }
        lo + (self.next() % (hi - lo + 1))
    }
}

/// Overall pressure envelope: quick attack, sustain with decay, long tail-off.
fn envelope(sample_idx: u32, total_samples: u32) -> i64 {
    let attack = SAMPLE_RATE * 20 / 1000;
    let release = total_samples * 35 / 100;

    if sample_idx < attack {
        (sample_idx as i64 * 256) / attack as i64
    } else if sample_idx >= total_samples.saturating_sub(release) {
        let remaining = total_samples.saturating_sub(sample_idx);
        (remaining as i64 * 256) / release as i64
    } else {
        let pos = sample_idx - attack;
        let len = total_samples.saturating_sub(attack + release);
        if len == 0 { 256 } else { 256 - (pos as i64 * 70 / len as i64) }
    }
}

/// Synthesize and play a single fart sound.
///
/// Source-filter model based on acoustic research:
/// - Source: sphincter pulse train (100-200 Hz) with jitter and shimmer
/// - Filter: resonant bandpass at ~250-340 Hz (rectal cavity resonance)
/// - Turbulence: filtered noise (brown noise) for airflow component
/// - Modulation: sub-harmonic drops, sputtering gate, pressure envelope
fn play_one_fart(rng: &mut Rng) {
    // --- Randomize parameters ---

    // Sphincter vibration rate: 100-200 Hz (comparable to vocal folds).
    let base_freq = rng.range(100, 185) as i64;
    let duration_ms = rng.range(800, 2500);
    let total_samples = SAMPLE_RATE * duration_ms / 1000;

    // Pitch contour: onset slightly higher, drops as pressure fades
    let start_freq = base_freq * rng.range(108, 135) as i64 / 100;
    let end_freq = base_freq * rng.range(50, 80) as i64 / 100;

    // Pulse duty cycle: fraction of each cycle the sphincter is "open".
    // Short duty = bright/squeaky, long duty = deep/round.
    let duty_start = rng.range(30, 55) as i64;
    let duty_end = rng.range(40, 65) as i64;

    // Rectal cavity resonance: bandpass filter at ~250-340 Hz (observed peak ~270 Hz).
    // Q controls how sharp the resonance is (higher Q = ringing longer).
    let resonant_freq = rng.range(230, 340);
    let resonant_q = rng.range(200, 600); // Q * 256 (Q = 0.8 to 2.3)

    // How much of the signal goes through the resonant filter vs direct
    let filter_mix: i64 = rng.range(100, 200) as i64; // out of 256

    // Turbulence noise: filtered brown noise for airflow component
    let noise_cutoff = rng.range(200, 500); // Hz
    let noise_mix: i64 = rng.range(25, 70) as i64; // out of 256

    // Frequency jitter: random per-cycle pitch variation
    let jitter_hz = rng.range(8, 25) as i32;
    let jitter_interval = rng.range(80, 200);

    // Shimmer: random amplitude variation per sphincter pulse
    let shimmer_amount: i64 = rng.range(30, 80) as i64;

    // Sub-harmonic drops: period doubling ("motorboat" moments)
    let subharm_block = SAMPLE_RATE * 50 / 1000;
    let subharm_chance = rng.range(5, 18);

    // Sputtering in the tail
    let sputter_block = SAMPLE_RATE * 30 / 1000;
    let sputter_onset_pct = rng.range(55, 75) as i64;
    let sputter_ramp = SAMPLE_RATE * 10 / 1000;

    // --- Initialize DSP ---

    let mut resonance = Biquad::bandpass(resonant_freq, resonant_q);
    let mut turbulence = Noise::brown(
        rng.next() as u64 | ((rng.next() as u64) << 32),
        noise_cutoff,
    );

    // --- Synthesis state ---

    let mut phase: u32 = 0;

    let mut jitter_offset: i32 = 0;
    let mut jitter_target: i32 = 0;
    let mut jitter_counter: u32 = 0;

    let mut shimmer_current: i64 = 256;
    let mut prev_in_pulse = false;

    let mut subharm_active = false;
    let mut subharm_counter: u32 = 0;

    let mut sputter_gate: i64 = 256;
    let mut sputter_target: i64 = 256;
    let mut sputter_counter: u32 = 0;

    let mut samples_written: u32 = 0;
    let chunk_frames: u32 = 1024;
    let mut buf = [0i16; 1024 * 2];

    while samples_written < total_samples {
        let remaining = total_samples - samples_written;
        let frames = core::cmp::min(chunk_frames, remaining);

        for i in 0..frames as usize {
            let sample_idx = samples_written + i as u32;
            let progress = sample_idx as i64 * 256 / total_samples as i64;

            // --- Pitch: contour + jitter + sub-harmonic ---

            let contour_freq = if progress < 38 {
                let t = progress * 256 / 38;
                start_freq + (base_freq - start_freq) * t / 256
            } else if progress < 166 {
                base_freq
            } else {
                let t = (progress - 166) * 256 / (256 - 166);
                base_freq + (end_freq - base_freq) * t / 256
            };

            jitter_counter += 1;
            if jitter_counter >= jitter_interval {
                jitter_counter = 0;
                jitter_target = (rng.next() as i32 % (jitter_hz * 2 + 1)) - jitter_hz;
            }
            jitter_offset += (jitter_target - jitter_offset + 4) / 8;

            let mut current_freq = contour_freq + jitter_offset as i64;

            subharm_counter += 1;
            if subharm_counter >= subharm_block {
                subharm_counter = 0;
                if rng.range(0, 99) < subharm_chance {
                    subharm_active = !subharm_active;
                } else {
                    subharm_active = false;
                }
            }
            if subharm_active {
                current_freq /= 2;
            }

            let phase_inc = synth::freq_to_inc(current_freq.max(15) as u32);

            // --- Source: sphincter pulse waveform ---

            let duty = duty_start + (duty_end - duty_start) * progress / 256;
            let duty_threshold = (duty * 256 / 100) as u32;
            let cycle_pos = ((phase >> (FP_SHIFT - 8)) & 0xFF) as u32;
            let in_pulse = cycle_pos < duty_threshold;

            // Shimmer: new random amplitude at each pulse onset
            if in_pulse && !prev_in_pulse {
                let var = (rng.next() % (shimmer_amount as u32 * 2 + 1)) as i64
                    - shimmer_amount;
                shimmer_current = (256 + var).max(80).min(256);
            }
            prev_in_pulse = in_pulse;

            // Half-sine pulse during open phase, silence during closed
            let pulse = if in_pulse && duty_threshold > 0 {
                let remapped = (cycle_pos * 128 / duty_threshold) as usize;
                synth::SINE_TABLE[remapped.min(127)] as i64 * shimmer_current / 256
            } else {
                0i64
            };

            // --- Filter: resonant bandpass (rectal cavity) ---

            let filtered = resonance.process(pulse as i16) as i64;

            // Mix direct pulse and filtered (resonant) signal
            let source = pulse * (256 - filter_mix) / 256
                + filtered * filter_mix / 256;

            // --- Turbulence: brown noise for airflow ---

            let noise = turbulence.sample() as i64;

            // Mix source and turbulence
            let mixed = source * (256 - noise_mix) / 256
                + noise * noise_mix / 256;

            // --- Sputtering gate ---

            let sputter_threshold = sputter_onset_pct * 256 / 100;
            sputter_counter += 1;
            if sputter_counter >= sputter_block {
                sputter_counter = 0;
                if progress > sputter_threshold {
                    let depth = (progress - sputter_threshold) * 256
                        / (256 - sputter_threshold);
                    let gap_prob = 15 + depth * 65 / 256;
                    if rng.range(0, 99) < gap_prob as u32 {
                        sputter_target = rng.range(0, 30) as i64;
                    } else {
                        sputter_target = 256;
                    }
                }
            }
            if sputter_gate < sputter_target {
                sputter_gate += 256 / sputter_ramp.max(1) as i64;
                if sputter_gate > sputter_target { sputter_gate = sputter_target; }
            } else if sputter_gate > sputter_target {
                sputter_gate -= 256 / sputter_ramp.max(1) as i64;
                if sputter_gate < sputter_target { sputter_gate = sputter_target; }
            }

            // --- Final output ---

            let env = envelope(sample_idx, total_samples);
            let sample = (mixed * sputter_gate / 256 * env / 256) as i32;
            let sample = sample.max(-32767).min(32767) as i16;

            buf[i * 2] = sample;
            buf[i * 2 + 1] = sample;

            phase = phase.wrapping_add(phase_inc) % FP_ONE;
        }

        match audio::write_samples(&buf[..frames as usize * 2]) {
            Ok(_) => {}
            Err(_) => {
                eprintln!("fart: audio write failed");
                break;
            }
        }

        samples_written += frames;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut count: u32 = 1;
    for arg in args.iter().skip(1) {
        if arg == "--help" || arg == "-h" {
            println!("Usage: fart [count]");
            println!("  Synthesizes realistic fart sounds.");
            println!("  count  Number of farts (default: 1)");
            return;
        }
        if let Ok(n) = arg.parse::<u32>() {
            if n > 0 { count = n; }
        }
    }

    if audio::init().is_err() {
        eprintln!("fart: audio init failed (no VirtIO sound device?)");
        std::process::exit(1);
    }

    let seed = match time::now_monotonic() {
        Ok(ts) => (ts.tv_sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.tv_nsec as u64),
        Err(_) => 42,
    };
    let mut rng = Rng::new(seed);

    for i in 0..count {
        play_one_fart(&mut rng);

        if i + 1 < count {
            let gap_ms = rng.range(200, 800);
            let gap_samples = SAMPLE_RATE * gap_ms / 1000;
            let silence = vec![0i16; gap_samples as usize * 2];
            let _ = audio::write_samples(&silence);
        }
    }
}
