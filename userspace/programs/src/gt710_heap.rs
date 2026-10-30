//! Bounded, parent-owned heap observations; only the final G12 payload carries them.
use libbreenix::gpu::abi::heap_trace as wire;
use std::{fmt::Write, io::Read, sync::Mutex};
const BOUNDARIES: usize = 171;
const MAX_NAME: usize = 80;
const MAX_BYTES: usize = 512 * 1024;
#[derive(Clone, Copy)]
struct Row { name: [u8; MAX_NAME], len: usize, values: [u64; wire::WORDS] }
impl Row { const EMPTY: Self = Self { name: [0; MAX_NAME], len: 0, values: [0; wire::WORDS] }; }
struct Store { rows: [Row; BOUNDARIES], used: usize, expected: usize, bad: bool }
impl Store {
    const fn new() -> Self { Self { rows: [Row::EMPTY; BOUNDARIES], used: 0, expected: 0, bad: false } }
    fn record(&mut self, index: usize, total: usize, name: &str, values: Result<[u64; wire::WORDS], i32>) {
        if index == 0 { self.expected = total.saturating_add(1); }
        if index != self.used || self.expected != total.saturating_add(1) || self.expected > BOUNDARIES
            || index >= BOUNDARIES || name.len() > MAX_NAME
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
            for (field, (&value, key)) in row.values.iter().zip(wire::NAMES).enumerate() {
                write!(text, "{}\"{key}\":{value}", if field == 0 { "" } else { "," }).map_err(|_| "case heap formatting")?;
            }
            text.push_str("}}\n");
        }
        if text.len() > MAX_BYTES { return Err("case heap evidence exceeds bound".into()); }
        Ok(text.into_bytes())
    }
}
static STORE: Mutex<Store> = Mutex::new(Store::new());
pub(super) fn boundary(index: usize, total: usize, name: &str) {
    let values = (|| {
        let mut file = std::fs::File::open("/proc/gt710-case-heap").map_err(|_| -5)?;
        let mut bytes = [0; wire::SIZE];file.read_exact(&mut bytes).map_err(|_| -5)?;
        let mut extra = [0];if file.read(&mut extra).map_err(|_| -5)? != 0 { return Err(-5); }
        wire::decode(&bytes).ok_or(-5)
    })();
    if let Ok(mut store) = STORE.lock() { store.record(index, total, name, values); }
}
pub(super) fn encode() -> Result<Vec<u8>, String> {
    STORE.lock().map_err(|_| "case heap evidence lock")?.encode()
}
