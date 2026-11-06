//! Per-window close escalation, timed by the caller's UI clock.

const KILL_DELAY_NS: u64 = 2_000_000_000;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Request,
    Term(u32),
    Kill(u32),
    None,
}

struct Pending {
    id: u64,
    owner: u32,
    deadline: Option<u64>,
    killed: bool,
}

#[derive(Default)]
pub(super) struct Close {
    own: u32,
    pending: Vec<Pending>,
}

impl Close {
    /// bwm's own windows (its launch error) close by event only, never by signal.
    pub(super) fn for_process(own: u32) -> Self {
        Self { own, pending: Vec::new() }
    }

    pub(super) fn request(&mut self, id: u64, owner: u32, now: u64) -> Action {
        if let Some(pending) = self.pending.iter_mut().find(|p| p.id == id) {
            if pending.owner == owner && owner != 0 && owner != self.own && pending.deadline.is_none() {
                pending.deadline = Some(now.saturating_add(KILL_DELAY_NS));
                return Action::Term(owner);
            }
            return Action::None;
        }
        self.pending.push(Pending { id, owner, deadline: None, killed: false });
        Action::Request
    }

    pub(super) fn retain(&mut self, mut present: impl FnMut(u64, u32) -> bool) {
        self.pending.retain(|p| present(p.id, p.owner));
    }

    pub(super) fn has_due(&self, now: u64) -> bool {
        self.pending.iter().any(|p| !p.killed && p.deadline.is_some_and(|d| now >= d))
    }

    pub(super) fn due(&mut self, now: u64, mut send: impl FnMut(u32, Action)) {
        for pending in &mut self.pending {
            if !pending.killed && pending.deadline.is_some_and(|deadline| now >= deadline) {
                pending.killed = true;
                send(pending.owner, Action::Kill(pending.owner));
            }
        }
    }

    pub(super) fn timeout_ms(&self, now: u64, timeout: i32) -> i32 {
        self.pending.iter().filter(|p| !p.killed).filter_map(|p| p.deadline)
            .map(|deadline| deadline.saturating_sub(now).saturating_add(999_999) / 1_000_000)
            .fold(timeout, |bound, ms| bound.min(ms.min(i32::MAX as u64) as i32))
    }
}
