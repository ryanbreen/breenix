//! Launch bookkeeping independent of process and window system calls.

#[derive(Default)]
pub struct Children { pending: Vec<(u32, u64, Vec<u8>)> }

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError { Missing, Fork, Other, NoWindow }

pub struct Listed { pub id: u64, pub owner: u32, pub commit: u64 }
pub struct Composed { pub id: u64, pub commit: u64, pub pixels: u64 }
pub enum Wait { Exited(u32), Interrupted, Empty }

impl Children {
    pub fn track(&mut self, pid: u32, now: u64) {
        self.pending.push((pid, now.saturating_add(5_000_000_000), Vec::new()));
    }
    pub fn track_path(&mut self, pid: u32, now: u64, path: &[u8]) {
        self.track(pid, now);
        self.pending.last_mut().unwrap().2 = path.to_vec();
    }
    pub fn spawn_error(errno: i32) -> LaunchError {
        match errno { 2 => LaunchError::Missing, 11 | 12 => LaunchError::Fork, _ => LaunchError::Other }
    }
    pub fn expire(&mut self, now: u64) -> Vec<(Vec<u8>, LaunchError)> {
        let mut errors = Vec::new();
        while let Some(index) = self.pending.iter().position(|p| now >= p.1) {
            let (_, _, path) = self.pending.remove(index);
            errors.push((path, LaunchError::NoWindow));
        }
        errors
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
        self.pending.retain(|(pid, _, _)| {
            let found = windows.iter().any(|w| w.owner == *pid && w.commit != 0 &&
                frame.iter().any(|e| e.id == w.id && e.commit != 0 && e.pixels != 0));
            if found { acknowledged.push(*pid); }
            !found
        });
        acknowledged
    }
}
