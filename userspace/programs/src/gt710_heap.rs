//! Bounded, parent-owned heap observations; only the final G12 payload carries them.
use libbreenix::gpu::abi::heap_trace as wire;
use std::{fmt::Write, io::Read, sync::Mutex, time::{Duration, Instant}};
const MAX_NAME: usize = 80;
const MAX_BYTES: usize = 1024 * 1024;
#[derive(Clone, Copy)]
struct Row { name: [u8; MAX_NAME], len: usize, values: [u64; wire::WORDS] }
impl Row { const EMPTY: Self = Self { name: [0; MAX_NAME], len: 0, values: [0; wire::WORDS] }; }
struct Store { rows: Vec<Row>, used: usize, expected: usize, bad: bool }
impl Store {
    const fn new() -> Self { Self { rows: Vec::new(), used: 0, expected: 0, bad: false } }
    fn prepare(&mut self, total: usize) -> Result<(), String> {
        let expected = total.checked_add(1)
            .filter(|n| *n <= MAX_BYTES / core::mem::size_of::<Row>())
            .ok_or("case heap boundary count exceeds bound")?;
        if self.used != 0 || (self.expected != 0 && self.expected != expected) {
            return Err("case heap boundary count changed".into());
        }
        if self.rows.capacity() < expected {
            self.rows.try_reserve_exact(expected).map_err(|_| "case heap boundary allocation")?;
        }
        self.rows.resize(expected, Row::EMPTY);
        Ok(())
    }
    fn record(&mut self, index: usize, total: usize, name: &str, values: Result<[u64; wire::WORDS], i32>) {
        let Some(expected) = total.checked_add(1) else { self.bad = true; return; };
        if index == 0 {
            if self.prepare(total).is_err() { self.bad = true; return; }
            self.expected = expected;
        }
        if index != self.used || self.expected != expected || index >= expected || name.len() > MAX_NAME
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_/.".contains(&b)) {
            self.bad = true;
            return;
        }
        let Ok(values) = values else { self.bad = true; return; };
        if values[0] != 1 { self.bad = true; return; }
        let row = &mut self.rows[index];
        row.name[..name.len()].copy_from_slice(name.as_bytes());row.len = name.len();row.values = values;
        self.used += 1;
    }
    fn encode(&self) -> Result<Vec<u8>, String> {
        if self.bad || self.expected == 0 || self.used != self.expected { return Err("incomplete native case heap evidence".into()); }
        let mut text = String::new();
        text.try_reserve_exact(MAX_BYTES).map_err(|_| "case heap evidence allocation")?;
        for (index, row) in self.rows[..self.used].iter().enumerate() {
            let name = std::str::from_utf8(&row.name[..row.len]).map_err(|_| "case heap name encoding")?;
            write!(text, "GT710_CASE_HEAP {{\"schema\":1,\"boundary\":{index},\"case\":\"{name}\",\"values\":{{").map_err(|_| "case heap formatting")?;
            for (field, (&value, key)) in row.values[..wire::COST_START].iter().zip(wire::NAMES).enumerate() {
                write!(text, "{}\"{key}\":{value}", if field == 0 { "" } else { "," }).map_err(|_| "case heap formatting")?;
            }
            // The ABI's ordered tail avoids repeating phase names at
            // each boundary. The host expands it only after digest checking.
            text.push_str(",\"costs_v1\":[");
            for (field, value) in row.values[wire::COST_START..].iter().enumerate() {
                write!(text, "{}{value}", if field == 0 { "" } else { "," }).map_err(|_| "case cost formatting")?;
            }
            text.push_str("]}}\n");
            if text.len() > MAX_BYTES { return Err("case heap evidence exceeds bound".into()); }
        }
        Ok(text.into_bytes())
    }
}
static STORE: Mutex<Store> = Mutex::new(Store::new());
static LAST_COST: Mutex<Option<(Instant, [u64; wire::WORDS])>> = Mutex::new(None);

/// Scalar diagnostics survive a later refused picture. Full heap records still
/// require the complete final G12 payload; these lines cannot certify a boot.
fn cost_report(name: &str, before: &[u64; wire::WORDS], after: &[u64; wire::WORDS], elapsed: Duration) -> Option<String> {
    if name.is_empty() || elapsed < Duration::from_secs(1) { return None; }
    let mut text = format!("GT710_CASE_COSTS case={name} sample_ms={}", elapsed.as_millis());
    for (index, key) in wire::NAMES.iter().enumerate().skip(wire::COST_START) {
        match after[index].checked_sub(before[index]) {
            Some(delta) => write!(text, " {key}={delta}").ok()?,
            None => write!(text, " {key}=reset").ok()?,
        }
    }
    Some(text)
}
// Called before suite entry, so the dynamically sized parent store is in the
// memory baseline rather than appearing as a case leak.
pub(super) fn prepare(total: usize) -> Result<(), String> {
    STORE.lock().map_err(|_| "case heap evidence lock")?.prepare(total)
}
pub(super) fn boundary(index: usize, total: usize, name: &str) {
    if index == 0 && prepare(total).is_err() {
        if let Ok(mut store) = STORE.lock() { store.bad = true; }
        return;
    }
    let values = (|| {
        let mut file = std::fs::File::open("/proc/gt710-case-heap").map_err(|_| -5)?;
        let mut bytes = [0; wire::SIZE];file.read_exact(&mut bytes).map_err(|_| -5)?;
        let mut extra = [0];if file.read(&mut extra).map_err(|_| -5)? != 0 { return Err(-5); }
        wire::decode(&bytes).ok_or(-5)
    })();
    if let Ok(after) = &values {
        let now = Instant::now();
        if let Ok(mut last) = LAST_COST.lock() {
            if index != 0 {
                if let Some((at, before)) = last.as_ref() {
                    if let Some(text) = cost_report(name, before, after, now.duration_since(*at)) {
                        println!("{text}");
                    }
                }
            }
            *last = Some((now, *after));
        }
    }
    if let Ok(mut store) = STORE.lock() { store.record(index, total, name, values); }
}
pub(super) fn encode() -> Result<Vec<u8>, String> {
    STORE.lock().map_err(|_| "case heap evidence lock")?.encode()
}
