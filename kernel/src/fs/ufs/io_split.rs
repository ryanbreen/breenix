//! Pure transfer and Linux dirent64 packing helpers.
pub struct Transfer {
    remaining: usize,
    limit: usize,
    requested: usize,
    stopped: bool,
}
impl Transfer {
    pub fn new(remaining: usize, limit: usize) -> Self {
        Self { remaining, limit, requested: 0, stopped: false }
    }
    pub fn next(&mut self) -> Option<usize> {
        if self.stopped || self.remaining == 0 || self.limit == 0 { return None; }
        self.requested = self.remaining.min(self.limit);
        Some(self.requested)
    }
    pub fn complete(&mut self, actual: usize) -> Result<bool, ()> {
        if self.requested == 0 || actual > self.requested { return Err(()); }
        self.remaining -= actual;
        self.stopped = actual < self.requested;
        self.requested = 0;
        Ok(!self.stopped && self.remaining != 0)
    }
}

pub fn pack_dirent(out: &mut [u8], ino: u64, cookie: u64, kind: u8, name: &[u8]) -> Option<usize> {
    let len = 19usize.checked_add(name.len())?.checked_add(1)?.checked_add(7)? & !7;
    if len > out.len() || len > u16::MAX as usize || name.contains(&0) { return None; }
    out[..len].fill(0);
    out[..8].copy_from_slice(&ino.to_ne_bytes());
    out[8..16].copy_from_slice(&cookie.to_ne_bytes());
    out[16..18].copy_from_slice(&(len as u16).to_ne_bytes());
    out[18] = kind;
    out[19..19 + name.len()].copy_from_slice(name);
    Some(len)
}
