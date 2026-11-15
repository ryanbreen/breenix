//! Seeded operation stream; the fixed prefix is the step's writer acceptance check.
use crate::*;

pub struct Stream {
    index: usize,
}
impl Stream {
    pub fn new(_seed: u64) -> Self {
        Self { index: 0 }
    }
}
impl Iterator for Stream {
    type Item = Operation;
    fn next(&mut self) -> Option<Self::Item> {
        let action = match self.index {
            0 => Action::Create {
                dir: ROOT,
                name: b"seed-file".to_vec(),
                mode: 0o644,
            },
            1 => Action::Write {
                ino: 16,
                offset: 8193,
                bytes: b"seed-data".to_vec(),
            },
            2 => Action::Fsync { ino: 16 },
            3 => Action::Remount,
            _ => return None,
        };
        self.index += 1;
        Some(Operation {
            at: Time {
                seconds: self.index as i64,
                nanos: 0,
            },
            action,
        })
    }
}
