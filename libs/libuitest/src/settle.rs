//! App-side causal barrier (docs/gui/plan.md section 6.4).
//! A poll is not an acknowledgment. Only accepted commit metadata advances it.

pub const BOUND_NS: u64 = 2_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub commit: u64,
    pub processed_serial: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    windows: Vec<Window>,
    predicates: Vec<(String, bool)>,
}
impl Observation {
    pub fn new(windows: Vec<Window>, predicates: Vec<(String, bool)>) -> Self {
        Self {
            windows,
            predicates,
        }
    }
}

pub struct Barrier {
    deadline: u64,
    targets: Vec<(u64, u64)>,
    predicates: Vec<String>,
    previous: Option<Observation>,
}
impl Barrier {
    /// Serial=0 requests committed state without an input watermark; see
    /// tests/ui_settle.rs::idle_requires_a_committed_window_and_changes_restart_stability.
    pub fn new(real_now_ns: u64, targets: Vec<(u64, u64)>, predicates: Vec<String>) -> Self {
        Self {
            deadline: real_now_ns.saturating_add(BOUND_NS),
            targets,
            predicates,
            previous: None,
        }
    }

    /// Require two consecutive matching observations, without imposing a quiet
    /// interval or advancing UI time. Future animation deadlines are irrelevant.
    pub fn observe(&mut self, real_now_ns: u64, observation: &Observation) -> Result<bool, String> {
        let waiting_window = self.targets.iter().find(|&&(id, serial)| {
            !observation.windows.iter().any(|w| {
                w.id == id
                    && w.commit != 0
                    && (serial == 0 || {
                        let wanted = libbreenix::input::abi::source_order(serial);
                        wanted.is_some()
                            && libbreenix::input::abi::source_order(w.processed_serial) >= wanted
                    })
            })
        });
        let waiting_predicate = self.predicates.iter().find(|name| {
            !observation
                .predicates
                .iter()
                .any(|(n, holds)| n == *name && *holds)
        });
        // Normalize only the requested state. Registry/ledger ordering and other
        // session windows cannot make an otherwise idle target appear unstable.
        let current = Observation::new(
            self.targets
                .iter()
                .filter_map(|(id, _)| observation.windows.iter().find(|w| w.id == *id).cloned())
                .collect(),
            self.predicates
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        observation
                            .predicates
                            .iter()
                            .any(|(n, holds)| n == name && *holds),
                    )
                })
                .collect(),
        );
        if real_now_ns >= self.deadline {
            let waiting = if let Some((id, serial)) = waiting_window {
                format!("waiting=processed-commit window={id} serial={serial}")
            } else if let Some(name) = waiting_predicate {
                format!("waiting=predicate predicate={name:?}")
            } else {
                format!(
                    "waiting=stability windows={:?} predicates={:?}",
                    self.targets, self.predicates
                )
            };
            // Keep a singular window field on stability timeouts as well.
            let target = self
                .targets
                .first()
                .map_or_else(String::new, |(id, serial)| {
                    format!(" window={id} serial={serial}")
                });
            return Err(format!("settle-timeout {waiting}{target}"));
        }
        if waiting_window.is_some() || waiting_predicate.is_some() {
            self.previous = None;
            return Ok(false);
        }
        let stable = self.previous.as_ref() == Some(&current);
        self.previous = Some(current);
        Ok(stable)
    }
}

/// Named nonblocking checks over committed trees or an app's side channel.
#[cfg(target_os = "breenix")]
pub struct Predicate<'a> {
    pub name: &'a str,
    pub check: &'a mut dyn FnMut() -> Result<bool, String>,
}

/// Wait for app-side conditions. Callers combine this with the epoch and
/// presentation acknowledgments of section 6.4.
#[cfg(target_os = "breenix")]
pub fn settle(targets: &[(u64, u64)], predicates: &mut [Predicate<'_>]) -> Result<(), String> {
    use libbreenix::{errno::Errno, error::Error, time, window};
    let now = || {
        time::now_monotonic()
            .map(|t| t.as_nanos() as u64)
            .map_err(|e| e.to_string())
    };
    let mut barrier = Barrier::new(
        now()?,
        targets.to_vec(),
        predicates.iter().map(|p| p.name.into()).collect(),
    );
    loop {
        let mut windows = Vec::with_capacity(targets.len());
        for &(id, _) in targets {
            match window::read_meta(id) {
                Ok((commit, meta)) => windows.push(Window {
                    id,
                    commit,
                    processed_serial: meta.processed_serial,
                }),
                // A not-yet-created/destroyed target still gets its named wait.
                Err(Error::Os(Errno::ENOENT)) => {}
                Err(error) => return Err(format!("settle window={id} metadata: {error}")),
            }
        }
        let mut checks = Vec::with_capacity(predicates.len());
        for predicate in predicates.iter_mut() {
            let holds = (predicate.check)()
                .map_err(|e| format!("settle predicate={:?}: {e}", predicate.name))?;
            checks.push((predicate.name.into(), holds));
        }
        // Stamp after reads/callbacks: a late acknowledgment cannot rescue the bound.
        if barrier.observe(now()?, &Observation::new(windows, checks))? {
            return Ok(());
        }
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
