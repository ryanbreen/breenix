//! Fixed-capacity min-heap of actual wake requests, owned by one CPU.
// A full heap refuses admission; the caller keeps its coarse scheduler timeout.
pub struct Deadlines { values: [u64; 64], len: usize }
impl Deadlines {
    pub const fn new() -> Self { Self { values: [0; 64], len: 0 } }
    pub fn next(&self) -> u64 { if self.len == 0 { u64::MAX } else { self.values[0] } }
    pub fn insert(&mut self, deadline: u64) -> bool {
        if self.values[..self.len].contains(&deadline) { return true; }
        if self.len == self.values.len() { return false; }
        let mut i = self.len;
        self.len += 1;
        while i > 0 {
            let parent = (i - 1) / 2;
            if self.values[parent] <= deadline { break; }
            self.values[i] = self.values[parent];
            i = parent;
        }
        self.values[i] = deadline;
        true
    }
    pub fn expire(&mut self, now: u64) {
        while self.len > 0 && self.values[0] <= now {
            self.len -= 1;
            let value = self.values[self.len];
            let mut i = 0;
            while i * 2 + 1 < self.len {
                let mut child = i * 2 + 1;
                if child + 1 < self.len && self.values[child + 1] < self.values[child] { child += 1; }
                if value <= self.values[child] { break; }
                self.values[i] = self.values[child];
                i = child;
            }
            self.values[i] = value;
        }
    }
}
