//! Decision over a registry snapshot and the shared completed-composite history.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowReady {
    pub id: u64,
    pub composite_serial: u64,
    pub presented_ns: u64,
    pub commit_id: u64,
    pub pixel_generation: u64,
}

pub struct WaitWindow<'a> {
    title: &'a str,
    deadline_ns: u64,
}
impl<'a> WaitWindow<'a> {
    pub fn new(title: &'a str, deadline_ns: u64) -> Self { Self { title, deadline_ns } }

    /// Registry titles are counted bytes. History rows are the five u64 words
    /// emitted by kernel/src/window/status_pages.rs, on scene::finish success.
    pub fn observe(&self, registry: &[(u64, &[u8])], records: &str, now_ns: u64) -> Result<Option<WindowReady>, String> {
        let timeout = || format!("wait-window-timeout title={:?} deadline_ns={}", self.title, self.deadline_ns);
        if now_ns > self.deadline_ns { return Err(timeout()); }
        let mut ids = registry.iter().filter(|(id, title)| *id != 0 && *title == self.title.as_bytes()).map(|(id, _)| *id);
        let id = ids.next();
        if ids.next().is_some() { return Err(format!("wait-window ambiguous title={:?}", self.title)); }
        let mut ready: Option<WindowReady> = None;
        for row in records.lines().filter(|row| !row.trim().is_empty()) {
            let mut fields = row.split_whitespace();
            let mut words = [0u64; 5];
            for word in &mut words {
                *word = fields.next().and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("wait-window invalid composite record title={:?}", self.title))?;
            }
            if fields.next().is_some() { return Err(format!("wait-window invalid composite record title={:?}", self.title)); }
            let [serial, presented_ns, window, commit_id, pixel_generation] = words;
            if Some(window) == id && serial != 0 && presented_ns != 0 && presented_ns <= now_ns
                && commit_id != 0 && pixel_generation != 0
                && ready.is_none_or(|r| serial < r.composite_serial) {
                ready = Some(WindowReady { id: window, composite_serial: serial, presented_ns, commit_id, pixel_generation });
            }
        }
        if ready.is_none() && now_ns == self.deadline_ns { return Err(timeout()); }
        Ok(ready)
    }
}
