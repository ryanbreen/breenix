//! Whole-scanout checkpoints: observation, canonical hashing and digest transport.
use crate::gpu::{self, abi::{record as r, Mode}};
use std::{string::String, vec::Vec};
use std::fmt::Write;
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};

fn word(rec: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(rec[at..at + 4].try_into().unwrap())
}
fn quad(rec: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(rec[at..at + 8].try_into().unwrap())
}

pub(super) fn validate(rec: &[u8; r::SIZE], mode: Mode, backend: u32, runner: u64) -> Result<(), &'static str> {
    if word(rec, r::VERSION_OFF) != r::VERSION || word(rec, r::SIZE_OFF) as usize != r::SIZE {
        return Err("record-version");
    }
    if mode.width == 0 || mode.height == 0
        || (word(rec, r::WIDTH), word(rec, r::HEIGHT)) != (mode.width, mode.height)
        || (word(rec, r::TARGET_WIDTH), word(rec, r::TARGET_HEIGHT)) != (mode.width, mode.height)
        || word(rec, r::X) != 0 || word(rec, r::Y) != 0 || quad(rec, r::TARGET) != 0 {
        return Err("not-whole-mode");
    }
    if word(rec, r::FLAGS) & 1 == 0 || quad(rec, r::PRESENT_FENCE) == 0
        || word(rec, r::UNWRITTEN) != 0 || word(rec, r::BACKEND) != backend {
        return Err("invalid-scanout");
    }
    if quad(rec, r::CONTEXT) == 0 || quad(rec, r::CONTEXT) == runner {
        return Err("runner-context");
    }
    Ok(())
}

pub(super) fn canonical_hash(pixels: &mut [u8], mode: Mode) -> Result<[u8; 32], &'static str> {
    let len = (mode.width as usize).checked_mul(mode.height as usize).and_then(|n| n.checked_mul(4));
    if len != Some(pixels.len()) || pixels.is_empty() { return Err("pixel-length"); }
    for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
    Ok(libpixcheck::sha256(pixels))
}

pub(super) fn named(name: &str) -> bool {
    !matches!(name, "end" | "intent") && name.split('-').all(|word|
        !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

pub(super) fn format_record(suite: &str, case: &str, name: &str, result: Result<([u8; 32], Mode), &str>) -> String {
    let mut line = std::format!("SUITE {suite} SCREEN {case} {name}");
    match result {
        Ok((hash, mode)) => {
            line.push_str(" sha256=");
            for byte in hash { let _ = write!(line, "{byte:02x}"); }
            let _ = write!(line, " mode={}x{}", mode.width, mode.height);
        }
        Err(reason) => { let _ = write!(line, " error={reason}"); }
    }
    line.push('\n');
    line
}

pub(super) fn pack(body: &str) -> Vec<u8> {
    let mut bytes = libpixcheck::sha256(body.as_bytes()).to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

pub(super) fn unpack<'a>(bytes: &'a [u8], suite: &str, case: &str) -> Result<&'a str, &'static str> {
    if bytes.len() < 32 || libpixcheck::sha256(&bytes[32..]) != bytes[..32] { return Err("side-file-hash"); }
    let body = std::str::from_utf8(&bytes[32..]).map_err(|_| "side-file-text")?;
    let mut names = Vec::new();
    for line in body.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let ["SUITE", id, "SCREEN", owner, name, fields @ ..] = words.as_slice() else { return Err("side-file-record"); };
        if *id != suite || *owner != case || (!named(name) && *name != "end") || names.contains(name) {
            return Err("side-file-case");
        }
        match fields {
            [hash, mode] => {
                let hash = hash.strip_prefix("sha256=").ok_or("side-file-digest")?;
                if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
                    return Err("side-file-digest");
                }
                let (w, h) = mode.strip_prefix("mode=").and_then(|v| v.split_once('x')).ok_or("side-file-mode")?;
                if [w, h].iter().any(|v| v.parse::<u32>().ok().is_none_or(|n| n == 0)) { return Err("side-file-mode"); }
            }
            [error] if error.strip_prefix("error=").is_some_and(named) => {}
            _ => return Err("side-file-fields"),
        }
        names.push(*name);
    }
    Ok(body)
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static COMPOSITOR: AtomicBool = AtomicBool::new(false);
static RUNNER: AtomicU64 = AtomicU64::new(0);
static LINES: Mutex<String> = Mutex::new(String::new());

pub(super) fn begin(suite: &str, gui: bool) {
    ENABLED.store(matches!(suite, "gpu" | "breengel"), Ordering::Relaxed);
    if !ENABLED.load(Ordering::Relaxed) { return; }
    COMPOSITOR.store(gui, Ordering::Relaxed);
    // CONTEXT in a record is a device identity, not the opaque userspace handle.
    // Identify the between-case runner binding without copying a whole frame.
    let mut pixel = [0; 4];
    let mut rec = [0; r::SIZE];
    let status = gpu::readback(0, [0, 0, 1, 1], &mut pixel, 4, &mut rec);
    RUNNER.store(if status == 0 { quad(&rec, r::CONTEXT) } else { 0 }, Ordering::Relaxed);
    LINES.lock().unwrap().clear();
    let _ = std::fs::remove_file(std::format!("{}.screen", super::judged_path()));
}

fn snapshot() -> Result<([u8; 32], Mode), &'static str> {
    if RUNNER.load(Ordering::Relaxed) == 0 { return Err("runner-unknown"); }
    let (status, caps) = gpu::caps();
    if status != 0 { return Err("caps-failed"); }
    let mode = gpu::current_mode().map_err(|_| "mode-failed")?;
    let len = (mode.width as usize).checked_mul(mode.height as usize).and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= i32::MAX as usize).ok_or("mode-size")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(len).map_err(|_| "allocation-failed")?;
    pixels.resize(len, 0);
    let mut rec = [0u8; r::SIZE];
    let status = gpu::readback(0, [0, 0, mode.width, mode.height], &mut pixels, mode.width * 4, &mut rec);
    if status != 0 {
        if !COMPOSITOR.load(Ordering::Relaxed) { return Err("readback-failed"); }
        rec = crate::suite_stage::observe(&mut pixels).map_err(|_| "readback-failed")?.record;
    }
    validate(&rec, mode, caps.backend, RUNNER.load(Ordering::Relaxed))?;
    Ok((canonical_hash(&mut pixels, mode)?, mode))
}

fn append(name: &str, end: bool) -> Result<(), &'static str> {
    let case = super::running_case().ok_or("outside-case")?;
    let (prefix, _) = super::RECORD_NAMES.get().ok_or("outside-case")?;
    let suite = prefix.strip_prefix("SUITE ").and_then(|s| s.strip_suffix(' ')).ok_or("outside-case")?;
    let result = snapshot();
    // An end observation is optional when the case no longer owns a scanout.
    if end && result.is_err() { return Ok(()); }
    let line = format_record(suite, case, name, result);
    let mut lines = LINES.lock().unwrap();
    if lines.lines().any(|line| line.split_whitespace().nth(4) == Some(name)) { return Err("duplicate-checkpoint"); }
    lines.push_str(&line);
    Ok(())
}

pub(super) fn checkpoint(name: &str) -> super::CaseResult {
    super::check(ENABLED.load(Ordering::Relaxed), "screen checkpoints are not enabled for this suite")?;
    super::check(named(name), "invalid or reserved screen checkpoint name")?;
    super::check(!super::running_case().is_some_and(|case| case.starts_with("performance/")),
        "performance cases cannot declare screen checkpoints")?;
    append(name, false).map_err(|e| super::CaseError::Fail(e.into()))
}

pub(super) fn finish() {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    let _ = append("end", true);
    let lines = LINES.lock().unwrap();
    // A missing or corrupt handoff is an additive SCREEN error, without changing CASE.
    let path = std::format!("{}.screen", super::judged_path());
    let _ = std::fs::write(path, pack(&lines));
}

pub(super) fn collect(suite: &str, case: &str) -> Vec<String> {
    if !ENABLED.load(Ordering::Relaxed) { return Vec::new(); }
    let path = std::format!("{}.screen", super::judged_path());
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return vec![format_record(suite, case, "end", Err("side-file-read"))],
    };
    let _ = std::fs::remove_file(path);
    match unpack(&bytes, suite, case) {
        Ok(body) => body.lines().map(String::from).collect(),
        Err(e) => vec![format_record(suite, case, "end", Err(e))],
    }
}
