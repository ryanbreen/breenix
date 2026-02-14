//! Breenish Shell (bsh) - ECMAScript-powered shell for Breenix OS
//!
//! A shell with a full ECMAScript scripting language. Processes and
//! subprocesses are managed through async/await and Promises.
//!
//! Usage:
//!   bsh              # Interactive REPL
//!   bsh script.js    # Execute a script file
//!   bsh -e 'code'    # Evaluate a string

use std::io::{self, Read, Write};

use breenish_js::error::{JsError, JsResult};
use breenish_js::object::{JsObject, ObjectHeap, ObjectKind};
use breenish_js::string::StringPool;
use breenish_js::value::JsValue;
use breenish_js::Context;
use libbreenix::audio;
use libbreenix::synth;

// ---------------------------------------------------------------------------
// Terminal bell (short chime on ambiguous tab completion)
// ---------------------------------------------------------------------------

/// Play a short bell/chime tone for tab completion feedback.
/// Lazily initializes audio on first call; silently does nothing if audio
/// is unavailable.
fn play_bell() {
    static mut AUDIO_OK: Option<bool> = None;

    let ok = unsafe {
        if let Some(v) = AUDIO_OK {
            v
        } else {
            let v = audio::init().is_ok();
            AUDIO_OK = Some(v);
            v
        }
    };
    if !ok {
        return;
    }

    // Bell: 880 Hz (A5), ~60ms, exponential-ish decay
    const SAMPLE_RATE: u32 = 44100;
    const DURATION_SAMPLES: u32 = SAMPLE_RATE * 60 / 1000; // 2646 samples
    const FREQ: u32 = 880;
    // Phase increment per sample: freq * 65536 / sample_rate
    const PHASE_INC: u32 = FREQ * 65536 / SAMPLE_RATE; // ~1306

    let mut samples = [0i16; (DURATION_SAMPLES as usize) * 2]; // stereo
    let mut phase: u32 = 0;

    for i in 0..DURATION_SAMPLES as usize {
        let idx = ((phase >> 8) & 0xFF) as usize;
        let raw = synth::SINE_TABLE[idx] as i32;

        // Envelope: linear decay over the full duration (simulates a bell strike)
        let env = ((DURATION_SAMPLES as usize - i) * 256 / DURATION_SAMPLES as usize) as i32;
        // Keep volume moderate (1/4 of full scale)
        let sample = ((raw * env) >> 10) as i16;

        samples[i * 2] = sample;
        samples[i * 2 + 1] = sample;
        phase = phase.wrapping_add(PHASE_INC);
    }

    let _ = audio::write_samples(&samples);
}

// ---------------------------------------------------------------------------
// Fart sound builtin (synthesized low-frequency rumble with noise)
// ---------------------------------------------------------------------------

/// Simple LCG PRNG for fart randomization.
struct FartRng {
    state: u64,
}

impl FartRng {
    fn new(seed: u64) -> Self {
        FartRng { state: seed.wrapping_add(1) }
    }

    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.state >> 33) as u32
    }

    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if lo >= hi {
            return lo;
        }
        lo + (self.next() % (hi - lo + 1))
    }
}

const FART_SAMPLE_RATE: u32 = 44100;

/// Overall pressure envelope: quick attack, sustain with decay, long tail-off.
fn fart_envelope(sample_idx: u32, total_samples: u32) -> i64 {
    let attack = FART_SAMPLE_RATE * 20 / 1000;
    let release = total_samples * 35 / 100;

    if sample_idx < attack {
        (sample_idx as i64 * 256) / attack as i64
    } else if sample_idx >= total_samples.saturating_sub(release) {
        let remaining = total_samples.saturating_sub(sample_idx);
        (remaining as i64 * 256) / release as i64
    } else {
        let sustain_pos = sample_idx - attack;
        let sustain_len = total_samples.saturating_sub(attack + release);
        if sustain_len == 0 { 256 }
        else { 256 - (sustain_pos as i64 * 70 / sustain_len as i64) }
    }
}

/// Synthesize and play one fart sound using source-filter model.
///
/// Based on acoustic research: sphincter pulses (100-200 Hz) excite rectal
/// cavity resonance (~270 Hz), producing perceived peak at 250-300 Hz.
/// Closed tube = odd harmonics. Pulsatile waveform. -10 dB/octave rolloff.
fn play_one_fart_sound(rng: &mut FartRng) {
    let base_freq = rng.range(100, 185) as i64;
    let duration_ms = rng.range(800, 2500);
    let total_samples = FART_SAMPLE_RATE * duration_ms / 1000;

    let start_freq = base_freq * rng.range(108, 135) as i64 / 100;
    let end_freq = base_freq * rng.range(50, 80) as i64 / 100;

    let duty_start = rng.range(30, 55) as i64;
    let duty_end = rng.range(40, 65) as i64;

    let resonant_freq = rng.range(230, 340) as i64;
    let resonance_mix: i64 = rng.range(80, 160) as i64;
    let resonance_decay: i64 = rng.range(252, 255) as i64;

    let jitter_hz = rng.range(8, 25) as i32;
    let jitter_interval = rng.range(80, 200);
    let shimmer_amount: i64 = rng.range(30, 80) as i64;

    let subharm_block = FART_SAMPLE_RATE * 50 / 1000;
    let subharm_chance = rng.range(5, 18);

    let sputter_block = FART_SAMPLE_RATE * 30 / 1000;
    let sputter_onset_pct = rng.range(55, 75) as i64;
    let sputter_ramp = FART_SAMPLE_RATE * 10 / 1000;

    let mut phase: u32 = 0;
    let mut resonant_phase: u32 = 0;
    let resonant_inc = fart_freq_to_inc(resonant_freq);
    let mut resonant_amp: i64 = 0;

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

            // Pitch contour
            let contour_freq = if progress < 38 {
                let t = progress * 256 / 38;
                start_freq + (base_freq - start_freq) * t / 256
            } else if progress < 166 {
                base_freq
            } else {
                let t = (progress - 166) * 256 / (256 - 166);
                base_freq + (end_freq - base_freq) * t / 256
            };

            // Frequency jitter
            jitter_counter += 1;
            if jitter_counter >= jitter_interval {
                jitter_counter = 0;
                jitter_target = (rng.next() as i32 % (jitter_hz * 2 + 1)) - jitter_hz;
            }
            jitter_offset += (jitter_target - jitter_offset + 4) / 8;
            let mut current_freq = contour_freq + jitter_offset as i64;

            // Sub-harmonic drops
            subharm_counter += 1;
            if subharm_counter >= subharm_block {
                subharm_counter = 0;
                if rng.range(0, 99) < subharm_chance {
                    subharm_active = !subharm_active;
                } else {
                    subharm_active = false;
                }
            }
            if subharm_active { current_freq /= 2; }

            let phase_inc = fart_freq_to_inc(current_freq);

            // Sphincter pulse waveform
            let duty = duty_start + (duty_end - duty_start) * progress / 256;
            let duty_threshold = (duty * 256 / 100) as u32;
            let cycle_pos = ((phase >> (FART_FP_SHIFT - 8)) & 0xFF) as u32;
            let in_pulse = cycle_pos < duty_threshold;

            if in_pulse && !prev_in_pulse {
                let var = (rng.next() % (shimmer_amount as u32 * 2 + 1)) as i64
                    - shimmer_amount;
                shimmer_current = (256 + var).max(80).min(256);
            }
            prev_in_pulse = in_pulse;

            let pulse = if in_pulse && duty_threshold > 0 {
                let remapped = (cycle_pos * 128 / duty_threshold) as usize;
                synth::SINE_TABLE[remapped.min(127)] as i64 * shimmer_current / 256
            } else {
                0i64
            };

            // Rectal cavity resonance
            if in_pulse {
                let excitation = pulse.abs() / 128;
                resonant_amp = resonant_amp + (excitation - resonant_amp) / 4;
                if resonant_amp > 300 { resonant_amp = 300; }
            } else {
                resonant_amp = resonant_amp * resonance_decay / 256;
            }
            let resonant_out = fart_sine_fp(resonant_phase) as i64 * resonant_amp / 256;

            // Mix source and resonance
            let mixed = pulse * (256 - resonance_mix) / 256
                + resonant_out * resonance_mix / 256;

            // Sputtering gate
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

            // Final output
            let env = fart_envelope(sample_idx, total_samples);
            let sample = (mixed * sputter_gate / 256 * env / 256) as i32;
            let sample = sample.max(-32767).min(32767) as i16;

            buf[i * 2] = sample;
            buf[i * 2 + 1] = sample;

            phase = phase.wrapping_add(phase_inc) % FART_FP_ONE;
            resonant_phase = resonant_phase.wrapping_add(resonant_inc) % FART_FP_ONE;
        }

        if audio::write_samples(&buf[..frames as usize * 2]).is_err() {
            break;
        }

        samples_written += frames;
    }
}

/// Play fart sound(s). Lazily initializes audio. `count` is how many farts.
fn play_fart(count: u32) {
    static mut AUDIO_OK: Option<bool> = None;

    let ok = unsafe {
        if let Some(v) = AUDIO_OK {
            v
        } else {
            let v = audio::init().is_ok();
            AUDIO_OK = Some(v);
            v
        }
    };
    if !ok {
        let _ = io::stderr().write_all(b"fart: audio unavailable\n");
        return;
    }

    let seed = match libbreenix::time::now_monotonic() {
        Ok(ts) => (ts.tv_sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.tv_nsec as u64),
        Err(_) => 42,
    };
    let mut rng = FartRng::new(seed);

    for i in 0..count {
        play_one_fart_sound(&mut rng);

        if i + 1 < count {
            let gap_ms = rng.range(200, 800);
            let gap_samples = FART_SAMPLE_RATE * gap_ms / 1000;
            let silence = vec![0i16; gap_samples as usize * 2];
            let _ = audio::write_samples(&silence);
        }
    }
}

// ---------------------------------------------------------------------------
// Native function implementations
// ---------------------------------------------------------------------------

/// exec(cmd, arg1, arg2, ...) -> { exitCode, stdout, stderr }
///
/// Forks a child process, executes the command, waits for it to finish,
/// and returns an object with the exit code and captured stdout/stderr.
fn native_exec(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(JsError::type_error("exec: expected at least one argument"));
    }

    // Collect command and arguments as strings
    let mut cmd_args: Vec<String> = Vec::new();
    for arg in args {
        if arg.is_string() {
            cmd_args.push(String::from(strings.get(arg.as_string_id())));
        } else if arg.is_number() {
            cmd_args.push(format!("{}", arg.to_number()));
        } else {
            cmd_args.push(String::from("undefined"));
        }
    }

    let cmd = &cmd_args[0];

    // Resolve command path
    let resolved = resolve_command(cmd);
    let exec_path = resolved.as_deref().unwrap_or(cmd.as_str());

    // Create pipes for stdout and stderr capture
    let (stdout_r, stdout_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => return Err(JsError::runtime("exec: pipe() failed")),
    };
    let (stderr_r, stderr_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => return Err(JsError::runtime("exec: pipe() failed")),
    };

    // Fork
    let fork_result = match libbreenix::process::fork() {
        Ok(r) => r,
        Err(_) => return Err(JsError::runtime("exec: fork() failed")),
    };

    match fork_result {
        libbreenix::process::ForkResult::Child => {
            // Child: redirect stdout/stderr to pipes, close read ends
            let _ = libbreenix::io::close(stdout_r);
            let _ = libbreenix::io::close(stderr_r);
            let _ = libbreenix::io::dup2(stdout_w, libbreenix::types::Fd::STDOUT);
            let _ = libbreenix::io::dup2(stderr_w, libbreenix::types::Fd::STDERR);
            let _ = libbreenix::io::close(stdout_w);
            let _ = libbreenix::io::close(stderr_w);

            // Build null-terminated argv
            let mut c_args: Vec<Vec<u8>> = Vec::new();
            for a in &cmd_args {
                let mut v: Vec<u8> = a.as_bytes().to_vec();
                v.push(0);
                c_args.push(v);
            }
            let argv_ptrs: Vec<*const u8> = c_args.iter().map(|a| a.as_ptr()).collect();

            // Build null-terminated path
            let mut path_bytes: Vec<u8> = exec_path.as_bytes().to_vec();
            path_bytes.push(0);

            // execv
            let mut argv_with_null: Vec<*const u8> = argv_ptrs;
            argv_with_null.push(core::ptr::null());
            let _ = libbreenix::process::execv(&path_bytes, argv_with_null.as_ptr());

            // If exec failed, exit with 127
            libbreenix::process::exit(127);
        }
        libbreenix::process::ForkResult::Parent(child_pid) => {
            // Parent: close write ends, read from pipes
            let _ = libbreenix::io::close(stdout_w);
            let _ = libbreenix::io::close(stderr_w);

            // Read stdout
            let stdout_str = read_fd_to_string(stdout_r);
            let _ = libbreenix::io::close(stdout_r);

            // Read stderr
            let stderr_str = read_fd_to_string(stderr_r);
            let _ = libbreenix::io::close(stderr_r);

            // Wait for child
            let mut status: i32 = 0;
            let _ = libbreenix::process::waitpid(
                child_pid.raw() as i32,
                &mut status as *mut i32,
                0,
            );

            let exit_code = if libbreenix::process::wifexited(status) {
                libbreenix::process::wexitstatus(status)
            } else {
                -1
            };

            // Build result object: { exitCode, stdout, stderr, pid }
            let mut obj = JsObject::new();
            let k_exit = strings.intern("exitCode");
            let k_stdout = strings.intern("stdout");
            let k_stderr = strings.intern("stderr");
            let k_pid = strings.intern("pid");

            obj.set(k_exit, JsValue::number(exit_code as f64));

            let stdout_id = strings.intern(&stdout_str);
            obj.set(k_stdout, JsValue::string(stdout_id));

            let stderr_id = strings.intern(&stderr_str);
            obj.set(k_stderr, JsValue::string(stderr_id));

            obj.set(k_pid, JsValue::number(child_pid.raw() as f64));

            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }
    }
}

/// cd(path) -> undefined
fn native_cd(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let path = if args.is_empty() {
        // cd with no args goes to "/"
        String::from("/")
    } else if args[0].is_string() {
        String::from(strings.get(args[0].as_string_id()))
    } else {
        return Err(JsError::type_error("cd: expected string path"));
    };

    let mut path_bytes: Vec<u8> = path.as_bytes().to_vec();
    path_bytes.push(0);

    match libbreenix::process::chdir(&path_bytes) {
        Ok(()) => Ok(JsValue::undefined()),
        Err(_) => Err(JsError::runtime(format!("cd: {}: No such directory", path))),
    }
}

/// pwd() -> string
fn native_pwd(
    _args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let mut buf = [0u8; 1024];
    match libbreenix::process::getcwd(&mut buf) {
        Ok(len) if len <= buf.len() => {
            let path = core::str::from_utf8(&buf[..len]).unwrap_or("/");
            let id = strings.intern(path);
            Ok(JsValue::string(id))
        }
        _ => {
            let id = strings.intern("/");
            Ok(JsValue::string(id))
        }
    }
}

/// which(cmd) -> string | null
fn native_which(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Ok(JsValue::null());
    }

    let cmd = String::from(strings.get(args[0].as_string_id()));

    match resolve_command(&cmd) {
        Some(path) => {
            let id = strings.intern(&path);
            Ok(JsValue::string(id))
        }
        None => Ok(JsValue::null()),
    }
}

/// readFile(path) -> string
fn native_read_file(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Err(JsError::type_error("readFile: expected string path"));
    }

    let path = String::from(strings.get(args[0].as_string_id()));

    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let id = strings.intern(&contents);
            Ok(JsValue::string(id))
        }
        Err(e) => Err(JsError::runtime(format!("readFile: {}: {}", path, e))),
    }
}

/// writeFile(path, data) -> undefined
fn native_write_file(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.len() < 2 || !args[0].is_string() || !args[1].is_string() {
        return Err(JsError::type_error(
            "writeFile: expected (path: string, data: string)",
        ));
    }

    let path = String::from(strings.get(args[0].as_string_id()));
    let data = String::from(strings.get(args[1].as_string_id()));

    match std::fs::write(&path, data.as_bytes()) {
        Ok(()) => Ok(JsValue::undefined()),
        Err(e) => Err(JsError::runtime(format!("writeFile: {}: {}", path, e))),
    }
}

/// exit(code?) -> never returns
fn native_exit(
    args: &[JsValue],
    _strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let code = if !args.is_empty() {
        args[0].to_number() as i32
    } else {
        0
    };
    std::process::exit(code);
}

/// pipe("cmd1 arg1", "cmd2 arg2", ...) -> { exitCode, stdout, stderr }
///
/// Creates a Unix pipeline connecting N commands via pipes. Each argument is
/// a string containing the command and its arguments, separated by whitespace.
/// Returns the result of the last command in the pipeline.
fn native_pipe(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(JsError::type_error("pipe: expected at least one command string"));
    }

    // Parse each argument into a command string
    let mut commands: Vec<Vec<String>> = Vec::new();
    for arg in args {
        if !arg.is_string() {
            return Err(JsError::type_error("pipe: each argument must be a command string"));
        }
        let cmd_str = String::from(strings.get(arg.as_string_id()));
        let parts: Vec<String> = cmd_str.split_whitespace().map(String::from).collect();
        if parts.is_empty() {
            return Err(JsError::type_error("pipe: empty command string"));
        }
        commands.push(parts);
    }

    let n = commands.len();

    // If only one command, just execute it directly (no pipes needed)
    if n == 1 {
        let mut exec_args = Vec::new();
        for part in &commands[0] {
            let id = strings.intern(part);
            exec_args.push(JsValue::string(id));
        }
        return native_exec(&exec_args, strings, heap);
    }

    // Create N-1 pipes for inter-process communication
    let mut pipes: Vec<(libbreenix::types::Fd, libbreenix::types::Fd)> = Vec::new();
    for _ in 0..(n - 1) {
        match libbreenix::io::pipe() {
            Ok(p) => pipes.push(p),
            Err(_) => {
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                return Err(JsError::runtime("pipe: pipe() syscall failed"));
            }
        }
    }

    // Create pipes to capture stdout and stderr of the last command
    let (last_stdout_r, last_stdout_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => {
            for (r, w) in &pipes {
                let _ = libbreenix::io::close(*r);
                let _ = libbreenix::io::close(*w);
            }
            return Err(JsError::runtime("pipe: pipe() syscall failed"));
        }
    };

    let (last_stderr_r, last_stderr_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => {
            for (r, w) in &pipes {
                let _ = libbreenix::io::close(*r);
                let _ = libbreenix::io::close(*w);
            }
            let _ = libbreenix::io::close(last_stdout_r);
            let _ = libbreenix::io::close(last_stdout_w);
            return Err(JsError::runtime("pipe: pipe() syscall failed"));
        }
    };

    // Fork each child in the pipeline
    let mut child_pids: Vec<i32> = Vec::new();
    for i in 0..n {
        let fork_result = match libbreenix::process::fork() {
            Ok(r) => r,
            Err(_) => {
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                let _ = libbreenix::io::close(last_stdout_r);
                let _ = libbreenix::io::close(last_stdout_w);
                let _ = libbreenix::io::close(last_stderr_r);
                let _ = libbreenix::io::close(last_stderr_w);
                for pid in &child_pids {
                    let mut status: i32 = 0;
                    let _ = libbreenix::process::waitpid(*pid, &mut status as *mut i32, 0);
                }
                return Err(JsError::runtime("pipe: fork() failed"));
            }
        };

        match fork_result {
            libbreenix::process::ForkResult::Child => {
                // Set up stdin: first child keeps original stdin,
                // others read from previous pipe
                if i > 0 {
                    let _ = libbreenix::io::dup2(pipes[i - 1].0, libbreenix::types::Fd::STDIN);
                }

                // Set up stdout: last child writes to capture pipe,
                // others write to next pipe
                if i < n - 1 {
                    let _ = libbreenix::io::dup2(pipes[i].1, libbreenix::types::Fd::STDOUT);
                } else {
                    let _ = libbreenix::io::dup2(last_stdout_w, libbreenix::types::Fd::STDOUT);
                    let _ = libbreenix::io::dup2(last_stderr_w, libbreenix::types::Fd::STDERR);
                }

                // Close all pipe fds in the child
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                let _ = libbreenix::io::close(last_stdout_r);
                let _ = libbreenix::io::close(last_stdout_w);
                let _ = libbreenix::io::close(last_stderr_r);
                let _ = libbreenix::io::close(last_stderr_w);

                // Resolve and exec the command
                let cmd = &commands[i][0];
                let resolved = resolve_command(cmd);
                let exec_path = resolved.as_deref().unwrap_or(cmd.as_str());

                let mut c_args: Vec<Vec<u8>> = Vec::new();
                for a in &commands[i] {
                    let mut v: Vec<u8> = a.as_bytes().to_vec();
                    v.push(0);
                    c_args.push(v);
                }
                let mut argv_ptrs: Vec<*const u8> = c_args.iter().map(|a| a.as_ptr()).collect();
                argv_ptrs.push(core::ptr::null());

                let mut path_bytes: Vec<u8> = exec_path.as_bytes().to_vec();
                path_bytes.push(0);

                let _ = libbreenix::process::execv(&path_bytes, argv_ptrs.as_ptr());
                libbreenix::process::exit(127);
            }
            libbreenix::process::ForkResult::Parent(child_pid) => {
                child_pids.push(child_pid.raw() as i32);
            }
        }
    }

    // Parent: close all inter-process pipe ends
    for (r, w) in &pipes {
        let _ = libbreenix::io::close(*r);
        let _ = libbreenix::io::close(*w);
    }
    let _ = libbreenix::io::close(last_stdout_w);
    let _ = libbreenix::io::close(last_stderr_w);

    // Read stdout and stderr of the last command
    let stdout_str = read_fd_to_string(last_stdout_r);
    let _ = libbreenix::io::close(last_stdout_r);

    let stderr_str = read_fd_to_string(last_stderr_r);
    let _ = libbreenix::io::close(last_stderr_r);

    // Wait for all children, capturing the exit code of the last one
    let last_pid = *child_pids.last().unwrap();
    let mut last_exit_code: i32 = -1;
    for pid in &child_pids {
        let mut status: i32 = 0;
        let _ = libbreenix::process::waitpid(*pid, &mut status as *mut i32, 0);
        if *pid == last_pid {
            last_exit_code = if libbreenix::process::wifexited(status) {
                libbreenix::process::wexitstatus(status)
            } else {
                -1
            };
        }
    }

    // Build result object: { exitCode, stdout, stderr }
    let mut obj = JsObject::new();
    let k_exit = strings.intern("exitCode");
    let k_stdout = strings.intern("stdout");
    let k_stderr = strings.intern("stderr");

    obj.set(k_exit, JsValue::number(last_exit_code as f64));

    let stdout_id = strings.intern(&stdout_str);
    obj.set(k_stdout, JsValue::string(stdout_id));

    let stderr_id = strings.intern(&stderr_str);
    obj.set(k_stderr, JsValue::string(stderr_id));

    let idx = heap.alloc(obj);
    Ok(JsValue::object(idx))
}

/// glob(pattern) -> array of matching file paths
///
/// Performs basic glob expansion on the given pattern string.
/// Supports `*` (match any sequence of chars) and `?` (match single char).
/// If the pattern has no wildcards, returns the pattern as-is in an array.
/// For patterns with a directory prefix (e.g., `/bin/*.rs`), splits into
/// directory + filename pattern.
fn native_glob(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Err(JsError::type_error("glob: expected string pattern"));
    }

    let pattern = String::from(strings.get(args[0].as_string_id()));

    // If no wildcards, return the pattern as a single-element array
    if !pattern.contains('*') && !pattern.contains('?') {
        let mut arr = JsObject::new_array();
        let id = strings.intern(&pattern);
        arr.push(JsValue::string(id));
        let idx = heap.alloc(arr);
        return Ok(JsValue::object(idx));
    }

    // Split pattern into directory and filename pattern
    let (dir, file_pattern) = if let Some(pos) = pattern.rfind('/') {
        let d = &pattern[..pos];
        let f = &pattern[pos + 1..];
        (if d.is_empty() { "/" } else { d }, f.to_string())
    } else {
        (".", pattern.clone())
    };

    let mut arr = JsObject::new_array();

    // Read directory entries and filter by pattern
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut names: Vec<String> = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy().to_string();
                    if glob_match(&file_pattern, &name_str) {
                        // Build full path
                        let full = if dir == "." {
                            name_str
                        } else if dir == "/" {
                            format!("/{}", name_str)
                        } else {
                            format!("{}/{}", dir, name_str)
                        };
                        names.push(full);
                    }
                }
            }
            names.sort();
            for name in &names {
                let id = strings.intern(name);
                arr.push(JsValue::string(id));
            }
        }
        Err(_) => {
            // Directory not readable: return empty array
        }
    }

    let idx = heap.alloc(arr);
    Ok(JsValue::object(idx))
}

/// Simple glob pattern matching supporting `*` and `?`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob_match_inner(&pat, &txt)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }

    pi == pat.len()
}

/// env() -> object with all env vars
/// env(name) -> get environment variable value
/// env(name, value) -> set environment variable, returns undefined
fn native_env(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    match args.len() {
        0 => {
            // Return an object with all environment variables
            let mut obj = JsObject::new();
            for (key, value) in std::env::vars() {
                let k = strings.intern(&key);
                let v_id = strings.intern(&value);
                obj.set(k, JsValue::string(v_id));
            }
            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }
        1 => {
            // Get environment variable
            if !args[0].is_string() {
                return Err(JsError::type_error("env: expected string name"));
            }
            let name = String::from(strings.get(args[0].as_string_id()));
            match std::env::var(&name) {
                Ok(val) => {
                    let id = strings.intern(&val);
                    Ok(JsValue::string(id))
                }
                Err(_) => Ok(JsValue::undefined()),
            }
        }
        _ => {
            // Set environment variable
            if !args[0].is_string() {
                return Err(JsError::type_error("env: expected string name"));
            }
            let name = String::from(strings.get(args[0].as_string_id()));
            if args[1].is_string() {
                let value = String::from(strings.get(args[1].as_string_id()));
                std::env::set_var(&name, &value);
            } else if args[1].is_undefined() || args[1].is_null() {
                std::env::remove_var(&name);
            } else {
                let value = format!("{}", args[1].to_number());
                std::env::set_var(&name, &value);
            }
            Ok(JsValue::undefined())
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Read all data from a file descriptor into a String.
fn read_fd_to_string(fd: libbreenix::types::Fd) -> String {
    let mut buf = [0u8; 4096];
    let mut result = Vec::new();
    loop {
        match libbreenix::io::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => result.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}

/// Return the default PATH, including /usr/local/test/bin if the kernel
/// is built with the `testing` feature (detected via /proc/breenix/testing).
fn default_path() -> String {
    if let Ok(content) = std::fs::read_to_string("/proc/breenix/testing") {
        if content.trim() == "1" {
            return String::from("/bin:/usr/bin:/usr/local/test/bin");
        }
    }
    String::from("/bin:/usr/bin")
}

/// Resolve a command name to a full path by searching PATH directories.
fn resolve_command(cmd: &str) -> Option<String> {
    // If cmd contains '/', use it directly
    if cmd.contains('/') {
        return Some(cmd.to_string());
    }

    // Search PATH
    let path_dirs = std::env::var("PATH").unwrap_or_else(|_| default_path());
    for dir in path_dirs.split(':') {
        let full_path = format!("{}/{}", dir, cmd);
        // Check if file exists and is executable
        let mut path_bytes: Vec<u8> = full_path.as_bytes().to_vec();
        path_bytes.push(0);
        let path_str = std::str::from_utf8(&path_bytes[..path_bytes.len() - 1]).unwrap_or("");
        if libbreenix::fs::access(path_str, libbreenix::fs::X_OK).is_ok() {
            return Some(full_path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// PATH-as-scope: command resolver and executor for JS global lookup
// ---------------------------------------------------------------------------

/// Command resolver: returns true if a command name exists in PATH.
fn command_resolver_fn(name: &str) -> bool {
    resolve_command(name).is_some()
}

/// Command executor: fork+exec with direct terminal output (no pipe capture).
/// Returns the exit code as a JsValue number.
fn command_executor_fn(
    cmd: &str,
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    // Build argument list: command name + JsValue args converted to strings
    let mut cmd_args: Vec<String> = Vec::new();
    cmd_args.push(String::from(cmd));
    for arg in args {
        if arg.is_string() {
            cmd_args.push(String::from(strings.get(arg.as_string_id())));
        } else if arg.is_number() {
            let n = arg.to_number();
            if n == (n as i64) as f64 && n.abs() < 1e15 {
                cmd_args.push(format!("{}", n as i64));
            } else {
                cmd_args.push(format!("{}", n));
            }
        } else if arg.is_boolean() {
            cmd_args.push(if arg.as_boolean() { String::from("true") } else { String::from("false") });
        } else {
            cmd_args.push(String::from("undefined"));
        }
    }

    // Resolve command path
    let resolved = resolve_command(cmd);
    let exec_path = resolved.as_deref().unwrap_or(cmd);

    // Fork
    let fork_result = match libbreenix::process::fork() {
        Ok(r) => r,
        Err(_) => return Err(JsError::runtime("command exec: fork() failed")),
    };

    match fork_result {
        libbreenix::process::ForkResult::Child => {
            // Child: inherits stdin/stdout/stderr directly (no pipe capture)
            let mut c_args: Vec<Vec<u8>> = Vec::new();
            for a in &cmd_args {
                let mut v: Vec<u8> = a.as_bytes().to_vec();
                v.push(0);
                c_args.push(v);
            }
            let mut argv_ptrs: Vec<*const u8> = c_args.iter().map(|a| a.as_ptr()).collect();
            argv_ptrs.push(core::ptr::null());

            let mut path_bytes: Vec<u8> = exec_path.as_bytes().to_vec();
            path_bytes.push(0);

            let _ = libbreenix::process::execv(&path_bytes, argv_ptrs.as_ptr());
            libbreenix::process::exit(127);
        }
        libbreenix::process::ForkResult::Parent(child_pid) => {
            let mut status: i32 = 0;
            let _ = libbreenix::process::waitpid(
                child_pid.raw() as i32,
                &mut status as *mut i32,
                0,
            );

            let exit_code = if libbreenix::process::wifexited(status) {
                libbreenix::process::wexitstatus(status)
            } else {
                -1
            };

            Ok(JsValue::number(exit_code as f64))
        }
    }
}

// ---------------------------------------------------------------------------
// Context setup
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Console native functions
// ---------------------------------------------------------------------------

/// console.log(...args) - print args to stdout separated by spaces, with newline
fn native_console_log(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let msg = format_console_args(args, strings, heap);
    let _ = io::stdout().write_all(msg.as_bytes());
    let _ = io::stdout().write_all(b"\n");
    let _ = io::stdout().flush();
    Ok(JsValue::undefined())
}

/// console.error(...args) - print args to stderr separated by spaces, with newline
fn native_console_error(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let msg = format_console_args(args, strings, heap);
    let _ = io::stderr().write_all(msg.as_bytes());
    let _ = io::stderr().write_all(b"\n");
    Ok(JsValue::undefined())
}

/// Format arguments for console.log/error/warn: space-separated string representations.
fn format_console_args(args: &[JsValue], strings: &mut StringPool, heap: &mut ObjectHeap) -> String {
    let mut parts: Vec<String> = Vec::new();
    for arg in args {
        if arg.is_string() {
            parts.push(String::from(strings.get(arg.as_string_id())));
        } else if arg.is_undefined() {
            parts.push(String::from("undefined"));
        } else if arg.is_null() {
            parts.push(String::from("null"));
        } else if arg.is_boolean() {
            parts.push(if arg.as_boolean() { String::from("true") } else { String::from("false") });
        } else if arg.is_number() {
            let n = arg.to_number();
            if n == (n as i64) as f64 && n.abs() < 1e15 {
                parts.push(format!("{}", n as i64));
            } else {
                parts.push(format!("{}", n));
            }
        } else if arg.is_object() {
            // Simple object representation
            let idx = arg.as_object_index();
            // Extract array info before releasing the borrow on heap
            let arr_info = heap.get(idx).map(|obj| {
                if obj.kind == ObjectKind::Array {
                    let len = obj.elements_len();
                    let elements: Vec<JsValue> = (0..len.min(10))
                        .map(|i| obj.get_index(i))
                        .collect();
                    Some((len, elements))
                } else {
                    None
                }
            });
            match arr_info {
                Some(Some((len, elements))) => {
                    let mut elems = Vec::new();
                    for v in &elements {
                        if !v.is_undefined() {
                            if v.is_string() {
                                elems.push(format!("'{}'", strings.get(v.as_string_id())));
                            } else {
                                elems.push(format_console_args(&[*v], strings, heap));
                            }
                        }
                    }
                    if len > 10 {
                        elems.push(String::from("..."));
                    }
                    parts.push(format!("[{}]", elems.join(", ")));
                }
                Some(None) => {
                    parts.push(String::from("[object Object]"));
                }
                None => {
                    parts.push(String::from("[object]"));
                }
            }
        } else {
            parts.push(format!("{}", arg.to_number()));
        }
    }
    parts.join(" ")
}

// ---------------------------------------------------------------------------
// Context setup
// ---------------------------------------------------------------------------

/// Create a new breenish-js Context with all shell builtins registered.
fn create_shell_context() -> Context {
    let mut ctx = Context::new();
    ctx.set_print_fn(print_fn);

    // Register PATH-as-scope callbacks
    ctx.set_command_resolver(command_resolver_fn);
    ctx.set_command_executor(command_executor_fn);

    // Register native shell functions
    ctx.register_native("exec", native_exec);
    ctx.register_native("cd", native_cd);
    ctx.register_native("pwd", native_pwd);
    ctx.register_native("which", native_which);
    ctx.register_native("readFile", native_read_file);
    ctx.register_native("writeFile", native_write_file);
    ctx.register_native("exit", native_exit);
    ctx.register_native("pipe", native_pipe);
    ctx.register_native("glob", native_glob);
    ctx.register_native("env", native_env);

    // Register console object (console.log, console.error, console.warn, console.info)
    ctx.register_native_object("console", &[
        ("log", native_console_log as breenish_js::NativeFn),
        ("error", native_console_error as breenish_js::NativeFn),
        ("warn", native_console_error as breenish_js::NativeFn),
        ("info", native_console_log as breenish_js::NativeFn),
    ]);

    // Register built-in objects
    ctx.register_promise_builtins();
    ctx.register_json_builtins();
    ctx.register_math_builtins();
    ctx.register_collection_builtins();

    ctx
}

fn print_fn(s: &str) {
    let _ = io::stdout().write_all(s.as_bytes());
    let _ = io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Line editor with history and cursor movement
// ---------------------------------------------------------------------------

/// Result of reading a single key press from the terminal.
enum Key {
    /// A printable ASCII character
    Char(u8),
    /// Enter / Return
    Enter,
    /// Backspace (0x7F or 0x08)
    Backspace,
    /// Escape sequence: arrow up
    Up,
    /// Escape sequence: arrow down
    Down,
    /// Escape sequence: arrow left
    Left,
    /// Escape sequence: arrow right
    Right,
    /// Home key (ESC [ H)
    Home,
    /// End key (ESC [ F)
    End,
    /// Delete key (ESC [ 3 ~)
    Delete,
    /// Ctrl+A - move to start of line
    CtrlA,
    /// Ctrl+C - cancel current line
    CtrlC,
    /// Ctrl+D - EOF on empty line
    CtrlD,
    /// Ctrl+E - move to end of line
    CtrlE,
    /// Ctrl+K - kill to end of line
    CtrlK,
    /// Ctrl+U - kill to start of line
    CtrlU,
    /// Ctrl+W - delete word before cursor
    CtrlW,
    /// Tab key - trigger completion
    Tab,
    /// End of file (read returned 0 bytes)
    Eof,
    /// Unknown or unhandled key
    Unknown,
}

/// Interactive line editor with history support, cursor movement, and editing.
///
/// Handles raw terminal I/O, ANSI escape sequences, and maintains command
/// history across invocations.
struct LineEditor {
    /// Current line buffer (ASCII bytes)
    buffer: Vec<u8>,
    /// Cursor position within the buffer (byte offset)
    cursor: usize,
    /// Command history (oldest first)
    history: Vec<String>,
    /// Current position in history during navigation.
    /// `history.len()` means we are editing a new line (not in history).
    history_pos: usize,
    /// Saved line content when the user navigates into history so we can
    /// restore it when they come back to the bottom.
    saved_line: String,
    /// Original terminal attributes, saved on entry to raw mode.
    orig_termios: Option<libbreenix::termios::Termios>,
}

impl LineEditor {
    fn new() -> Self {
        LineEditor {
            buffer: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: 0,
            saved_line: String::new(),
            orig_termios: None,
        }
    }

    /// Enter raw mode: disable canonical mode and echo so we get individual
    /// key presses and handle display ourselves.
    fn enable_raw_mode(&mut self) {
        let fd = libbreenix::types::Fd::STDIN;
        let mut termios = libbreenix::termios::Termios::default();
        if libbreenix::termios::tcgetattr(fd, &mut termios).is_ok() {
            self.orig_termios = Some(termios);
            let mut raw = termios;
            // Disable canonical mode (line buffering) and echo
            raw.c_lflag &= !(libbreenix::termios::lflag::ICANON
                | libbreenix::termios::lflag::ECHO
                | libbreenix::termios::lflag::ECHOE
                | libbreenix::termios::lflag::ECHOK
                | libbreenix::termios::lflag::ECHONL);
            // Disable signal generation for Ctrl+C/Ctrl+Z so we handle them
            raw.c_lflag &= !libbreenix::termios::lflag::ISIG;
            // Read one byte at a time, no timeout
            raw.c_cc[libbreenix::termios::cc::VMIN] = 1;
            raw.c_cc[libbreenix::termios::cc::VTIME] = 0;
            let _ = libbreenix::termios::tcsetattr(
                fd,
                libbreenix::termios::TCSAFLUSH,
                &raw,
            );
        }
    }

    /// Restore the original terminal attributes saved by `enable_raw_mode`.
    fn disable_raw_mode(&mut self) {
        if let Some(ref orig) = self.orig_termios {
            let fd = libbreenix::types::Fd::STDIN;
            let _ = libbreenix::termios::tcsetattr(
                fd,
                libbreenix::termios::TCSAFLUSH,
                orig,
            );
        }
    }

    /// Read a single byte from stdin. Returns `None` on EOF or error.
    fn read_byte() -> Option<u8> {
        let mut buf = [0u8; 1];
        match io::stdin().read(&mut buf) {
            Ok(1) => Some(buf[0]),
            _ => None,
        }
    }

    /// Read a single key press, decoding multi-byte escape sequences.
    fn read_key() -> Key {
        let byte = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Eof,
        };

        match byte {
            // Ctrl+A
            0x01 => Key::CtrlA,
            // Ctrl+C
            0x03 => Key::CtrlC,
            // Ctrl+D
            0x04 => Key::CtrlD,
            // Ctrl+E
            0x05 => Key::CtrlE,
            // Tab
            0x09 => Key::Tab,
            // Ctrl+K
            0x0B => Key::CtrlK,
            // Ctrl+U
            0x15 => Key::CtrlU,
            // Ctrl+W
            0x17 => Key::CtrlW,
            // Enter (carriage return)
            b'\r' | b'\n' => Key::Enter,
            // Backspace (DEL or BS)
            0x7F | 0x08 => Key::Backspace,
            // Escape - start of escape sequence
            0x1B => Self::read_escape_sequence(),
            // Printable ASCII
            0x20..=0x7E => Key::Char(byte),
            _ => Key::Unknown,
        }
    }

    /// Parse an escape sequence after the initial ESC byte.
    fn read_escape_sequence() -> Key {
        let b1 = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Unknown,
        };

        if b1 != b'[' {
            return Key::Unknown;
        }

        let b2 = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Unknown,
        };

        match b2 {
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => Key::Right,
            b'D' => Key::Left,
            b'H' => Key::Home,
            b'F' => Key::End,
            // ESC [ 1 ~ (Home alternate) or ESC [ 3 ~ (Delete) or ESC [ 4 ~ (End alternate)
            b'1' | b'3' | b'4' => {
                let b3 = match Self::read_byte() {
                    Some(b) => b,
                    None => return Key::Unknown,
                };
                if b3 == b'~' {
                    match b2 {
                        b'1' => Key::Home,
                        b'3' => Key::Delete,
                        b'4' => Key::End,
                        _ => Key::Unknown,
                    }
                } else {
                    Key::Unknown
                }
            }
            _ => Key::Unknown,
        }
    }

    /// Write raw bytes to stdout (used for terminal control).
    fn write_out(data: &[u8]) {
        let _ = io::stdout().write_all(data);
    }

    /// Flush stdout.
    fn flush_out() {
        let _ = io::stdout().flush();
    }

    /// Redraw the current line: prompt + buffer, then position the cursor
    /// correctly.
    fn refresh_line(&self, prompt: &str) {
        // Move to start of line
        Self::write_out(b"\r");
        // Print prompt and buffer
        Self::write_out(prompt.as_bytes());
        Self::write_out(&self.buffer);
        // Clear from cursor to end of line (in case text was deleted)
        Self::write_out(b"\x1b[K");
        // Move cursor back to the correct position if not at end of buffer
        let chars_after_cursor = self.buffer.len() - self.cursor;
        if chars_after_cursor > 0 {
            let seq = format!("\x1b[{}D", chars_after_cursor);
            Self::write_out(seq.as_bytes());
        }
        Self::flush_out();
    }

    /// Insert a character at the current cursor position.
    fn insert_char(&mut self, ch: u8) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// Delete the character before the cursor (backspace).
    fn delete_back(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    /// Delete the character at the cursor (delete key).
    fn delete_at(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    /// Move cursor one position to the left.
    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor one position to the right.
    fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    /// Move cursor to the start of the line.
    fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end of the line.
    fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    /// Clear everything before the cursor (Ctrl+U).
    fn kill_line_before(&mut self) {
        if self.cursor > 0 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }
    }

    /// Clear everything from the cursor to end of line (Ctrl+K).
    fn kill_line_after(&mut self) {
        self.buffer.truncate(self.cursor);
    }

    /// Delete the word before the cursor (Ctrl+W).
    /// Skips trailing whitespace, then deletes back to the previous whitespace
    /// boundary.
    fn delete_word_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let orig = self.cursor;
        // Skip whitespace before cursor
        while self.cursor > 0 && self.buffer[self.cursor - 1] == b' ' {
            self.cursor -= 1;
        }
        // Delete back to the next whitespace or start of line
        while self.cursor > 0 && self.buffer[self.cursor - 1] != b' ' {
            self.cursor -= 1;
        }
        self.buffer.drain(self.cursor..orig);
    }

    // ----- Tab completion -----

    /// Find the start of the word under the cursor by walking backwards
    /// from the cursor position, skipping non-whitespace characters.
    /// Handles quoted strings: if the cursor is inside quotes, the word
    /// starts just after the opening quote character.
    fn find_word_start(&self) -> usize {
        let mut pos = self.cursor;
        if pos == 0 {
            return 0;
        }

        // Check if we are inside a quoted string by counting quotes
        // before the cursor position.
        let before_cursor = &self.buffer[..pos];
        let single_quotes = before_cursor.iter().filter(|&&b| b == b'\'').count();
        let double_quotes = before_cursor.iter().filter(|&&b| b == b'"').count();

        // If inside an open single quote, walk back to just after the quote
        if single_quotes % 2 == 1 {
            while pos > 0 && self.buffer[pos - 1] != b'\'' {
                pos -= 1;
            }
            return pos;
        }

        // If inside an open double quote, walk back to just after the quote
        if double_quotes % 2 == 1 {
            while pos > 0 && self.buffer[pos - 1] != b'"' {
                pos -= 1;
            }
            return pos;
        }

        // Normal case: walk back over non-whitespace
        while pos > 0 && !self.buffer[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        pos
    }

    /// Perform tab completion. Determines whether we are completing a
    /// command (first word) or a filename (subsequent words), collects
    /// candidates, and inserts the completion into the buffer.
    fn complete(&mut self, prompt: &str, global_names: &[String]) {
        let word_start = self.find_word_start();
        let partial = String::from_utf8_lossy(&self.buffer[word_start..self.cursor]).to_string();

        // Determine if this is a command position: the text before word_start
        // is all whitespace (or empty), meaning this is the first word.
        let is_command = self.buffer[..word_start]
            .iter()
            .all(|b| b.is_ascii_whitespace());

        let completions = if is_command {
            self.complete_command(&partial, global_names)
        } else {
            self.complete_filename(&partial)
        };

        if completions.len() == 1 {
            // Single match: complete it fully
            let completion = &completions[0];
            if completion.len() > partial.len() {
                let suffix = &completion[partial.len()..];
                for b in suffix.bytes() {
                    self.insert_char(b);
                }
            }
            // Add a trailing space after command completion, or after a
            // filename that is not a directory (directories end with '/').
            if is_command {
                self.insert_char(b' ');
            } else if !completions[0].ends_with('/') {
                self.insert_char(b' ');
            }
            self.refresh_line(prompt);
        } else if completions.len() > 1 {
            // Multiple matches: bell + show them and complete the common prefix
            play_bell();
            Self::write_out(b"\r\n");
            for c in &completions {
                Self::write_out(c.as_bytes());
                Self::write_out(b"  ");
            }
            Self::write_out(b"\r\n");

            // Complete the longest common prefix
            let common = Self::find_common_prefix(&completions);
            if common.len() > partial.len() {
                let suffix = &common[partial.len()..];
                for b in suffix.bytes() {
                    self.insert_char(b);
                }
            }

            // Redraw the prompt and current line
            self.refresh_line(prompt);
        }
        // No matches: just bell
        if completions.is_empty() {
            play_bell();
        }
    }

    /// Complete a command name by matching JS globals/builtins and searching
    /// PATH directories for executables whose names start with `partial`.
    fn complete_command(&self, partial: &str, global_names: &[String]) -> Vec<String> {
        if partial.is_empty() {
            return Vec::new();
        }

        let mut matches: Vec<String> = Vec::new();

        // 1. Match JS globals and builtins
        for name in global_names {
            if name.starts_with(partial) && !matches.contains(name) {
                matches.push(name.clone());
            }
        }

        // 2. Match PATH executables
        let path_dirs =
            std::env::var("PATH").unwrap_or_else(|_| default_path());

        for dir in path_dirs.split(':') {
            if dir.is_empty() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with(partial) {
                        let name = name_str.to_string();
                        if !matches.contains(&name) {
                            matches.push(name);
                        }
                    }
                }
            }
        }

        matches.sort();
        matches
    }

    /// Complete a filename. The `partial` may contain a directory prefix
    /// (e.g. `/bin/ls` -> dir="/bin", prefix="ls"). If there is no slash,
    /// the current directory is used. Directory entries get a trailing '/'.
    fn complete_filename(&self, partial: &str) -> Vec<String> {
        let (dir, prefix) = if let Some(pos) = partial.rfind('/') {
            let d = &partial[..pos];
            let p = &partial[pos + 1..];
            (
                if d.is_empty() {
                    String::from("/")
                } else {
                    d.to_string()
                },
                p.to_string(),
            )
        } else {
            (String::from("."), partial.to_string())
        };

        let mut matches: Vec<String> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy().to_string();
                if name_str.starts_with(&prefix) {
                    // Check if the entry is a directory to append '/'
                    let is_dir = entry
                        .file_type()
                        .map(|ft| ft.is_dir())
                        .unwrap_or(false);

                    // Build the completion string: if the user typed a path
                    // prefix with '/', include it in the result; otherwise
                    // just return the filename.
                    let completed = if partial.contains('/') {
                        let dir_prefix = &partial[..partial.rfind('/').unwrap() + 1];
                        format!(
                            "{}{}{}",
                            dir_prefix,
                            name_str,
                            if is_dir { "/" } else { "" }
                        )
                    } else {
                        format!("{}{}", name_str, if is_dir { "/" } else { "" })
                    };

                    matches.push(completed);
                }
            }
        }

        matches.sort();
        matches
    }

    /// Find the longest common prefix among a list of strings.
    fn find_common_prefix(words: &[String]) -> String {
        if words.is_empty() {
            return String::new();
        }
        let first = &words[0];
        let mut prefix_len = first.len();
        for word in &words[1..] {
            prefix_len = prefix_len.min(word.len());
            for (i, (a, b)) in first.bytes().zip(word.bytes()).enumerate() {
                if a != b {
                    prefix_len = prefix_len.min(i);
                    break;
                }
            }
        }
        first[..prefix_len].to_string()
    }

    // ----- History and buffer management -----

    /// Replace the buffer with a history entry or the saved line.
    fn set_buffer_from_str(&mut self, s: &str) {
        self.buffer.clear();
        self.buffer.extend_from_slice(s.as_bytes());
        self.cursor = self.buffer.len();
    }

    /// Navigate up in history (older entries).
    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_pos == self.history.len() {
            // Save the current line before entering history
            self.saved_line = String::from_utf8_lossy(&self.buffer).into_owned();
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let entry = self.history[self.history_pos].clone();
            self.set_buffer_from_str(&entry);
        }
    }

    /// Navigate down in history (newer entries).
    fn history_down(&mut self) {
        if self.history_pos >= self.history.len() {
            return;
        }
        self.history_pos += 1;
        if self.history_pos == self.history.len() {
            // Back to the line being edited
            let saved = self.saved_line.clone();
            self.set_buffer_from_str(&saved);
        } else {
            let entry = self.history[self.history_pos].clone();
            self.set_buffer_from_str(&entry);
        }
    }

    /// Add a completed line to the history, avoiding consecutive duplicates.
    fn add_to_history(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        // Don't add if it's the same as the last entry
        if let Some(last) = self.history.last() {
            if last == line {
                return;
            }
        }
        self.history.push(line.to_string());
    }

    /// Read a complete line from the user with full editing support.
    ///
    /// Returns `Some(line)` when the user presses Enter, or `None` on
    /// EOF (Ctrl+D on an empty line).
    fn read_line(&mut self, prompt: &str, global_names: &[String]) -> Option<String> {
        self.buffer.clear();
        self.cursor = 0;
        self.history_pos = self.history.len();
        self.saved_line.clear();

        self.enable_raw_mode();

        // Print the prompt
        Self::write_out(prompt.as_bytes());
        Self::flush_out();

        let result = loop {
            match Self::read_key() {
                Key::Enter => {
                    // Print newline and return the line
                    Self::write_out(b"\r\n");
                    Self::flush_out();
                    let line = String::from_utf8_lossy(&self.buffer).into_owned();
                    break Some(line);
                }
                Key::Eof => {
                    Self::write_out(b"\r\n");
                    Self::flush_out();
                    break None;
                }
                Key::CtrlD => {
                    if self.buffer.is_empty() {
                        Self::write_out(b"\r\n");
                        Self::flush_out();
                        break None;
                    }
                    // Non-empty line: Ctrl+D does nothing (or could delete-at)
                }
                Key::CtrlC => {
                    // Clear current line, print ^C and a new prompt
                    Self::write_out(b"^C\r\n");
                    Self::flush_out();
                    self.buffer.clear();
                    self.cursor = 0;
                    Self::write_out(prompt.as_bytes());
                    Self::flush_out();
                }
                Key::Backspace => {
                    self.delete_back();
                    self.refresh_line(prompt);
                }
                Key::Delete => {
                    self.delete_at();
                    self.refresh_line(prompt);
                }
                Key::Left => {
                    self.move_left();
                    self.refresh_line(prompt);
                }
                Key::Right => {
                    self.move_right();
                    self.refresh_line(prompt);
                }
                Key::Home | Key::CtrlA => {
                    self.move_home();
                    self.refresh_line(prompt);
                }
                Key::End | Key::CtrlE => {
                    self.move_end();
                    self.refresh_line(prompt);
                }
                Key::Up => {
                    self.history_up();
                    self.refresh_line(prompt);
                }
                Key::Down => {
                    self.history_down();
                    self.refresh_line(prompt);
                }
                Key::CtrlU => {
                    self.kill_line_before();
                    self.refresh_line(prompt);
                }
                Key::CtrlK => {
                    self.kill_line_after();
                    self.refresh_line(prompt);
                }
                Key::CtrlW => {
                    self.delete_word_back();
                    self.refresh_line(prompt);
                }
                Key::Tab => {
                    self.complete(prompt, global_names);
                }
                Key::Char(ch) => {
                    self.insert_char(ch);
                    self.refresh_line(prompt);
                }
                Key::Unknown => {
                    // Ignore unrecognized keys
                }
            }
        };

        self.disable_raw_mode();
        result
    }
}

// ---------------------------------------------------------------------------
// Shell modes
// ---------------------------------------------------------------------------

fn run_repl() {
    // Set default PATH based on kernel testing mode
    if std::env::var("PATH").is_err() {
        std::env::set_var("PATH", default_path());
    }

    let mut ctx = create_shell_context();

    // Load startup scripts
    load_rc_file(&mut ctx, "/etc/bshrc");

    let _ = io::stdout().write_all(b"breenish v0.5.0 -- ECMAScript shell for Breenix\n");
    let _ = io::stdout().flush();

    let mut editor = LineEditor::new();

    loop {
        // Show current directory in prompt
        let prompt = match get_short_cwd() {
            Some(cwd) => format!("bsh {}> ", cwd),
            None => String::from("bsh> "),
        };

        let global_names = ctx.global_names();
        let line = match editor.read_line(&prompt, &global_names) {
            Some(line) => line,
            None => return, // EOF / Ctrl+D
        };

        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        editor.add_to_history(line);

        // Handle `source <path>` as a shell builtin
        if let Some(path) = parse_source_command(line) {
            source_file(&mut ctx, &path);
            continue;
        }

        // Handle shell builtins (cd, pwd, exit, which, help) in shell syntax
        if let Some(code) = builtin_wrap(line) {
            match ctx.eval(&code) {
                Ok(result) => {
                    if !result.is_undefined() {
                        let formatted = ctx.format_value(result);
                        let msg = format!("{}\n", formatted);
                        let _ = io::stdout().write_all(msg.as_bytes());
                        let _ = io::stdout().flush();
                    }
                }
                Err(e) => {
                    let msg = format!("{}\n", e);
                    let _ = io::stderr().write_all(msg.as_bytes());
                }
            }
            continue;
        }

        // Handle bare command shorthand: if it looks like a command (starts with
        // a letter, no JS operators), wrap it in exec() automatically
        let is_auto_exec = should_auto_exec(line);
        let code = if is_auto_exec {
            auto_exec_wrap(line)
        } else {
            line.to_string()
        };

        match ctx.eval(&code) {
            Ok(result) => {
                // Auto-print non-undefined expression results (like Node REPL),
                // but only for JS expressions -- auto-exec'd commands handle
                // their own output via auto_exec_wrap().
                if !is_auto_exec && !result.is_undefined() {
                    let formatted = ctx.format_value(result);
                    let msg = format!("{}\n", formatted);
                    let _ = io::stdout().write_all(msg.as_bytes());
                    let _ = io::stdout().flush();
                }
            }
            Err(e) => {
                let msg = format!("{}\n", e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
    }
}

/// Convert shell-style builtin commands to JS function calls.
///
/// Returns `Some(js_code)` if the line is a recognized builtin, `None` otherwise.
/// This handles commands like `cd /bin` -> `cd("/bin")`, `help` -> print help text, etc.
fn builtin_wrap(line: &str) -> Option<String> {
    let line = line.trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let args_str = parts.next().unwrap_or("").trim();

    match cmd {
        "cd" => {
            if args_str.is_empty() {
                Some(String::from("cd(\"/\")"))
            } else {
                let escaped = args_str.replace('\\', "\\\\").replace('"', "\\\"");
                Some(format!("cd(\"{}\")", escaped))
            }
        }
        "pwd" => Some(String::from("print(pwd())")),
        "exit" => {
            if args_str.is_empty() {
                Some(String::from("exit(0)"))
            } else {
                Some(format!("exit({})", args_str))
            }
        }
        "which" => {
            if args_str.is_empty() {
                Some(String::from("print(\"usage: which <command>\")"))
            } else {
                let escaped = args_str.replace('\\', "\\\\").replace('"', "\\\"");
                Some(format!("print(which(\"{}\") ?? \"not found\")", escaped))
            }
        }
        "fart" => {
            let count: u32 = if args_str.is_empty() {
                1
            } else {
                args_str.parse().unwrap_or(1).max(1)
            };
            play_fart(count);
            Some(String::from("undefined"))
        }
        "help" => {
            Some(String::from(r#"print("bsh -- Breenish ECMAScript Shell\n\nShell builtins:\n  cd <dir>       Change directory\n  pwd            Print working directory\n  exit [code]    Exit the shell\n  which <cmd>    Find command in PATH\n  source <file>  Execute a script file\n  fart [count]   Play fart sound(s)\n  help           Show this help\n\nProcess execution:\n  exec(cmd, ...args)    Run a command, returns {exitCode, stdout, stderr}\n  pipe(cmd1, cmd2, ...) Pipeline commands\n  bls /bin              Bare commands are auto-wrapped in exec()\n\nFile operations:\n  readFile(path)          Read file contents\n  writeFile(path, data)   Write to file\n  glob(pattern)           Wildcard expansion (*.rs)\n\nEnvironment:\n  env()              All environment variables\n  env(name)          Get variable\n  env(name, value)   Set variable\n\nJavaScript:\n  Full ECMAScript: let/const, functions, arrows, closures,\n  if/else, for/while, try/catch, async/await, template literals,\n  destructuring, spread, Map, Set, JSON, Math, Promise\n\nUse Tab for auto-completion. Up/Down for history.\nSee: docs/user-guide/bsh-shell-guide.md for full documentation.")"#))
        }
        _ => None,
    }
}

/// Check if a line looks like a shell command rather than JavaScript.
///
/// The heuristic: a line is treated as a command if it looks like
/// `command arg1 arg2 ...` — an unadorned identifier (possibly a path)
/// followed by whitespace-separated arguments. Anything that looks like
/// a JS expression (contains `.`, `(`, `=`, etc. in the first token) is
/// evaluated as JavaScript instead.
fn should_auto_exec(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }

    // Explicit JS constructs - don't auto-exec
    let js_starts = [
        "let ", "const ", "var ", "function ", "if ", "if(", "while ", "while(",
        "for ", "for(", "switch ", "switch(", "try ", "try{", "return ",
        "throw ", "class ", "import ", "export ", "async ", "await ",
        "new ", "delete ", "typeof ", "void ",
        "{", "[", "(", "//", "/*", "\"", "'", "`",
    ];
    for prefix in &js_starts {
        if line.starts_with(prefix) {
            return false;
        }
    }

    // Lines with assignment, arrow, or comparison syntax are JS
    if line.contains("=>") || line.contains("= ") || line.contains("==") {
        return false;
    }

    // Numeric literals are JS (e.g. "1+1", "42", "3.14")
    let first_char = line.chars().next().unwrap();
    if first_char.is_ascii_digit() {
        return false;
    }

    // Extract the first token (before whitespace). If it contains `.` or `(`
    // it's a JS expression (method call like `console.log(...)`, function call
    // like `foo(1)`), not a shell command. Exception: tokens starting with
    // `./` or `../` are relative paths (commands).
    let first_token = line.split_whitespace().next().unwrap_or("");
    if first_token.contains('(') {
        return false; // Function call: foo(), console.log("hi"), etc.
    }
    if first_token.contains('.')
        && !first_token.starts_with("./")
        && !first_token.starts_with("../")
    {
        return false; // Method chain: console.log, obj.method, etc.
    }

    // Lines that start with a valid command character: letter, '/', './'
    if first_char.is_ascii_alphabetic() || first_char == '/' || first_char == '.' {
        return true;
    }

    false
}

/// Wrap a bare command line in exec() and print the result.
fn auto_exec_wrap(line: &str) -> String {
    // Split the line into command and args by whitespace
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return line.to_string();
    }

    // Build exec call: exec("cmd", "arg1", "arg2")
    let mut call = String::from("let __r = exec(");
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            call.push_str(", ");
        }
        call.push('"');
        // Escape quotes in args
        for c in part.chars() {
            if c == '"' {
                call.push_str("\\\"");
            } else {
                call.push(c);
            }
        }
        call.push('"');
    }
    call.push_str("); if (__r.exitCode === 127) { console.error(\"");
    // Escape the command name for the error message
    for c in parts[0].chars() {
        if c == '"' {
            call.push_str("\\\"");
        } else if c == '\\' {
            call.push_str("\\\\");
        } else {
            call.push(c);
        }
    }
    call.push_str(": command not found\"); } else { if (__r.stdout.length > 0) print(__r.stdout); }");
    call
}

/// Load and evaluate an RC (startup) file, silently ignoring missing files.
/// Errors during evaluation are printed to stderr but do not abort the shell.
fn load_rc_file(ctx: &mut Context, path: &str) {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if let Err(e) = ctx.eval(&contents) {
                let msg = format!("bsh: error in {}: {}\n", path, e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
        Err(_) => {
            // File doesn't exist or can't be read -- silently ignore
        }
    }
}

/// Parse a `source <path>` command line. Returns the path if the line
/// is a source command, or None otherwise.
///
/// Accepted forms:
///   source path/to/file
///   source "path/to/file"
///   source 'path/to/file'
///   source("path/to/file")
fn parse_source_command(line: &str) -> Option<String> {
    let trimmed = line.trim();

    if let Some(rest) = trimmed.strip_prefix("source(") {
        // source("path") or source('path')
        let rest = rest.trim_end_matches(')').trim();
        let path = rest.trim_matches('"').trim_matches('\'');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    } else if let Some(rest) = trimmed.strip_prefix("source ") {
        // source path or source "path" or source 'path'
        let path = rest.trim().trim_matches('"').trim_matches('\'');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }

    None
}

/// Read a file and evaluate its contents in the given context.
fn source_file(ctx: &mut Context, path: &str) {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if let Err(e) = ctx.eval(&contents) {
                let msg = format!("{}\n", e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
        Err(e) => {
            let msg = format!("source: {}: {}\n", path, e);
            let _ = io::stderr().write_all(msg.as_bytes());
        }
    }
}

/// Get a shortened version of the current working directory.
fn get_short_cwd() -> Option<String> {
    let mut buf = [0u8; 1024];
    match libbreenix::process::getcwd(&mut buf) {
        Ok(len) if len <= buf.len() => {
            let cwd = std::str::from_utf8(&buf[..len]).ok()?;
            if cwd == "/" {
                Some(String::from("/"))
            } else {
                // Show only the last component
                cwd.rsplit('/').next().map(String::from)
            }
        }
        _ => None,
    }
}

fn run_string(code: &str) {
    let mut ctx = create_shell_context();

    match ctx.eval(code) {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("{}\n", e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    }
}

fn run_file(path: &str) {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("bsh: cannot open '{}': {}\n", path, e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    };

    let mut source = String::new();
    if let Err(e) = file.read_to_string(&mut source) {
        let msg = format!("bsh: cannot read '{}': {}\n", path, e);
        let _ = io::stderr().write_all(msg.as_bytes());
        std::process::exit(1);
    }

    let mut ctx = create_shell_context();

    match ctx.eval(&source) {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("{}\n", e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 1 {
        // No arguments: interactive REPL
        run_repl();
    } else if args.len() == 3 && args[1] == "-e" {
        // -e 'code': evaluate string
        run_string(&args[2]);
    } else if args.len() == 2 {
        // script file
        run_file(&args[1]);
    } else {
        let _ = io::stderr().write_all(b"Usage: bsh [script.js | -e 'code']\n");
        std::process::exit(1);
    }
}
