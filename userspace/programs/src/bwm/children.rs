//! Launch bookkeeping independent of process and window system calls.

#[derive(Default)]
pub struct Children { pending: Vec<(u32, u64)> }

pub struct Listed { pub id: u64, pub owner: u32, pub commit: u64 }
pub struct Composed { pub id: u64, pub commit: u64, pub pixels: u64 }
pub enum Wait { Exited(u32), Interrupted, Empty }

impl Children {
    pub fn track(&mut self, pid: u32, now: u64) {
        self.pending.push((pid, now.saturating_add(5_000_000_000)));
    }
    pub fn is_empty(&self) -> bool { self.deadline().is_none() }
    pub fn deadline(&self) -> Option<u64> { self.pending.iter().map(|p| p.1).min() }
    pub fn reap(&mut self, mut wait: impl FnMut() -> Wait) {
        loop {
            match wait() {
                Wait::Exited(pid) => self.pending.retain(|p| p.0 != pid),
                Wait::Interrupted => continue,
                Wait::Empty => break,
            }
        }
    }
    pub fn acknowledge(&mut self, windows: &[Listed], frame: &[Composed]) -> Vec<u32> {
        let mut acknowledged = Vec::new();
        self.pending.retain(|(pid, _)| {
            let found = windows.iter().any(|w| w.owner == *pid && w.commit != 0 &&
                frame.iter().any(|e| e.id == w.id && e.commit != 0 && e.pixels != 0));
            if found { acknowledged.push(*pid); }
            !found
        });
        acknowledged
    }
}
