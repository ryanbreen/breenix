//! Causal app and completed-composite barrier (docs/gui/plan.md section 6.4).
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

/// A recorded completed composite and the registry/status observed around it.
/// An occluded window's entry accounts for its generation (docs/gui/plan.md 6.4).
pub struct Presentation {
    pub windows: Vec<libbreenix::window::abi::WinInfo>,
    pub header: libbreenix::window::abi::CompositeRecordHeader,
    pub entries: Vec<libbreenix::window::abi::CompositeEntry>,
    pub status: libbreenix::window::abi::CompositorStatus,
}
impl Presentation {
    pub fn waiting(&self, epoch: u64, wm_serial: u64, excluded: &[u64]) -> Option<String> {
        use libbreenix::window::abi::{compositor_ack_flags::NONE_NEEDED, win_flags};
        let s = &self.status;
        if s.claim_epoch == 0 || s.epoch < epoch {
            return Some(format!("waiting=compositor-epoch epoch={epoch}"));
        }
        if wm_serial != 0
            && libbreenix::input::abi::source_order(s.handled_serial)
                < libbreenix::input::abi::source_order(wm_serial)
        {
            return Some(format!("waiting=wm-handled serial={wm_serial}"));
        }
        let none = s.flags & NONE_NEEDED != 0;
        if s.flags & !NONE_NEEDED != 0 || (none && s.presented != 0) {
            return Some("waiting=compositor-ack".into());
        }
        let completed = self.header.serial != 0
            && self.header.flags == 0
            && self.header.count as usize == self.entries.len();
        // A newer completed frame supersedes an older acknowledged frame. A
        // NONE_NEEDED acknowledgment reuses available completed pixels.
        if !none
            && ((s.presented == 0 && (epoch != 0 || wm_serial != 0))
                || (s.presented != 0 && (!completed || self.header.serial < s.presented)))
        {
            return Some(format!("waiting=presentation composite={}", s.presented));
        }
        self.windows
            .iter()
            .filter(|w| {
                !excluded.contains(&w.id)
                    && w.flags & (win_flags::REGISTERED | win_flags::VISIBLE | win_flags::MINIMIZED)
                        == win_flags::REGISTERED | win_flags::VISIBLE
            })
            .find(|w| {
                !completed
                    || w.pixel_gen == 0
                    || !self
                        .entries
                        .iter()
                        .any(|e| e.id == w.id && e.pixel_gen >= w.pixel_gen)
            })
            .map(|w| {
                format!(
                    "waiting=presentation window={} pixel_gen={} composite={}",
                    w.id, w.pixel_gen, self.header.serial
                )
            })
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

    /// Combine presentation and app conditions under the same real deadline.
    pub fn observe_composite(
        &mut self,
        real_now_ns: u64,
        observation: &Observation,
        presentation: &Presentation,
        epoch: u64,
        wm_serial: u64,
        excluded: &[u64],
    ) -> Result<bool, String> {
        if let Some(waiting) = presentation.waiting(epoch, wm_serial, excluded) {
            if real_now_ns >= self.deadline {
                return Err(format!("settle-timeout {waiting}"));
            }
            self.previous = None;
            return Ok(false);
        }
        self.observe(real_now_ns, observation)
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

/// Wait for processed commits, predicates and completed presentation.
/// Session callers use `settle_session` for the current epoch and exclusions.
#[cfg(target_os = "breenix")]
pub fn settle(targets: &[(u64, u64)], predicates: &mut [Predicate<'_>]) -> Result<(), String> {
    settle_inner(targets, predicates, 0, 0, &[])
}

/// Settle the current stepped epoch, with a WM-handled serial when requested.
#[cfg(target_os = "breenix")]
pub fn settle_session(
    session: &crate::Session,
    targets: &[(u64, u64)],
    wm_serial: u64,
    predicates: &mut [Predicate<'_>],
) -> Result<(), String> {
    let page = libbreenix::ui_clock::ClockPage::open(
        &std::path::Path::new(session.config_root()).join("ui-clock"),
    )?;
    let mut excluded: Vec<u64> = page
        .participants()
        .iter()
        .map(|p| p.id)
        .filter(|id| session.is_excluded(*id))
        .collect();
    for w in registry()? {
        if session.is_excluded(w.id) && !excluded.contains(&w.id) {
            excluded.push(w.id);
        }
    }
    let epoch = if page
        .participants()
        .iter()
        .any(|p| p.flags & libbreenix::ui_clock::COMPOSITOR != 0)
    {
        page.stamp().epoch
    } else {
        0
    };
    settle_inner(targets, predicates, epoch, wm_serial, &excluded)
}

#[cfg(target_os = "breenix")]
fn settle_inner(
    targets: &[(u64, u64)],
    predicates: &mut [Predicate<'_>],
    epoch: u64,
    wm_serial: u64,
    excluded: &[u64],
) -> Result<(), String> {
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
        let presentation = presentation()?;
        if barrier.observe_composite(
            now()?,
            &Observation::new(windows, checks),
            &presentation,
            epoch,
            wm_serial,
            excluded,
        )? {
            return Ok(());
        }
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}

#[cfg(target_os = "breenix")]
fn registry() -> Result<Vec<libbreenix::window::abi::WinInfo>, String> {
    let mut windows = vec![libbreenix::window::abi::WinInfo::default(); 64];
    loop {
        let n = libbreenix::window::list(&mut windows).map_err(|e| e.to_string())?;
        if n <= windows.len() {
            windows.truncate(n);
            return Ok(windows);
        }
        windows.resize(n, Default::default());
    }
}

/// Read the registry, compositor status and retained completion record.
#[cfg(target_os = "breenix")]
pub fn presentation() -> Result<Presentation, String> {
    use libbreenix::{
        errno::Errno,
        error::Error,
        window::{self, abi},
    };
    let mut status = abi::CompositorStatus::default();
    window::call(abi::op::COMPOSITOR_STATUS, &mut status).map_err(|e| e.to_string())?;
    let windows = registry()?;
    let head = core::mem::size_of::<abi::CompositeRecordHeader>();
    let entry = core::mem::size_of::<abi::CompositeEntry>();
    // The retained frame can contain windows destroyed since LIST. Retry its
    // bounded sizing query instead of truncating the record to LIST's count.
    let mut bytes = vec![0u8; head + 64 * entry];
    loop {
        let mut query = abi::RecordQuery {
            serial: 0,
            ptr: bytes.as_mut_ptr() as u64,
            len: bytes.len() as u32,
            reserved: 0,
        };
        match window::call(abi::op::COMPOSITE_RECORD, &mut query) {
            Ok(_) => break,
            Err(Error::Os(Errno::ENOENT)) => {
                return Ok(Presentation {
                    windows,
                    status,
                    header: Default::default(),
                    entries: vec![],
                })
            }
            Err(Error::Os(Errno::EINVAL)) if bytes.len() < head + 4096 * entry => {
                bytes.resize((bytes.len() * 2).min(head + 4096 * entry), 0);
            }
            Err(e) => return Err(format!("settle composite record: {e}")),
        }
    }
    // SAFETY: the successful kernel copy writes plain ABI values; the byte
    // buffer may be unaligned. Check the count before reading its entries.
    let header =
        unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<abi::CompositeRecordHeader>()) };
    if header.count as usize > (bytes.len() - head) / entry {
        return Err("settle composite record truncated".into());
    }
    let entries = (0..header.count as usize)
        .map(|i| unsafe {
            core::ptr::read_unaligned(
                bytes
                    .as_ptr()
                    .add(head + i * entry)
                    .cast::<abi::CompositeEntry>(),
            )
        })
        .collect();
    Ok(Presentation {
        windows,
        status,
        header,
        entries,
    })
}
