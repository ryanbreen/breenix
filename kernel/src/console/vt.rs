//! Six virtual terminals on the screen console (`docs/console/contract.md`,
//! sections 1, 3 and 4).
//!
//! A terminal has its own grid, scrollback, KD mode and VT mode, kept under
//! the console's lock (`super::CONSOLE`), with which terminal is in the
//! foreground and the one switch that may be pending. A switch is requested
//! here (an ioctl, and the keyboard from V5) and completed by the render
//! thread: it draws the new terminal, and the acquisition completes once that
//! frame is on the display and any `VT_PROCESS` owner has answered. This
//! module does not draw, sleep, signal or log with the lock held: the work a change
//! causes outside the lock is returned as `Effects` and run by the caller in
//! thread context.

use super::grid::{Cell, Grid, HISTORY_LINES};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// The number of virtual terminals (one constant, contract section 1.1).
pub const VT_COUNT: usize = 6;
/// How long a `VT_PROCESS` owner has to answer a release or an acquisition.
pub const VT_RELEASE_TIMEOUT_MS: u64 = 3000;
pub const VT_ACQUIRE_TIMEOUT_MS: u64 = 3000;

const EPERM: i32 = 1;
const ENXIO: i32 = 6;
const EBUSY: i32 = 16;
const EINVAL: i32 = 22;

/// `struct vt_mode`'s `mode`.
pub const VT_AUTO: u8 = 0;
pub const VT_PROCESS: u8 = 1;
/// `VT_RELDISP`'s acknowledgement of an acquisition.
pub const VT_ACKACQ: u64 = 2;
/// The highest signal number `VT_SETMODE` accepts.
const SIGNAL_MAX: i16 = 64;

/// What asked for a switch, as the switch's log line names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Ioctl,
    Chvt,
    /// A key binding: `alt-f2`, `ctrl-alt-f2`, ...
    Key(&'static str),
}

impl core::fmt::Display for Source {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Source::Ioctl => f.write_str("ioctl"),
            Source::Chvt => f.write_str("chvt"),
            Source::Key(chord) => write!(f, "key {}", chord),
        }
    }
}

/// `VT_SETMODE`'s `VT_PROCESS`: the owner and its signals.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ProcessMode {
    owner: u64,
    relsig: u32,
    acqsig: u32,
}

/// Terminal-input replies (DSR answers) waiting for the terminal's input
/// queue, in fixed storage: the writer may be echoing in interrupt context.
pub(super) struct Replies {
    bytes: [u8; 1024],
    head: usize,
    len: usize,
}

impl Replies {
    const fn new() -> Self {
        Self { bytes: [0; 1024], head: 0, len: 0 }
    }

    /// Keep a whole reply, or drop it when it does not fit; whether it was kept.
    pub(super) fn push(&mut self, reply: &[u8]) -> bool {
        let capacity = self.bytes.len();
        if reply.len() > capacity - self.len {
            return false;
        }
        for &byte in reply {
            self.bytes[(self.head + self.len) % capacity] = byte;
            self.len += 1;
        }
        true
    }

    /// Copy the oldest bytes into `out`, keeping them; how many.
    pub(super) fn peek(&self, out: &mut [u8]) -> usize {
        let n = self.len.min(out.len());
        for (index, byte) in out[..n].iter_mut().enumerate() {
            *byte = self.bytes[(self.head + index) % self.bytes.len()];
        }
        n
    }

    /// Forget the `n` oldest bytes.
    pub(super) fn consume(&mut self, n: usize) {
        let n = n.min(self.len);
        self.head = (self.head + n) % self.bytes.len();
        self.len -= n;
    }
}

/// One virtual terminal's screen and modes.
pub(super) struct Vt {
    pub(super) grid: Grid,
    pub(super) replies: Replies,
    /// `KD_GRAPHICS`, with its graphics owner's process ID.
    graphics: Option<u64>,
    /// `VT_PROCESS`, else `VT_AUTO`.
    process: Option<ProcessMode>,
    /// The scrollback could not be allocated (logged once).
    history_failed: bool,
}

impl Vt {
    pub(super) const fn new() -> Self {
        Self { grid: Grid::new(), replies: Replies::new(), graphics: None, process: None, history_failed: false }
    }
}

/// Where a pending switch is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the `VT_PROCESS` owner of `from` to release it.
    Releasing { deadline: u64 },
    /// The owner of `from` released it while the previous switch's read-back
    /// was pending: the acquiring step starts once that verdict is recorded.
    Released,
    /// `to` is in the foreground; waiting for its frame (`drawn`) and its
    /// `VT_PROCESS` owner's acknowledgement (`answered`).
    Acquiring { epoch: u64, drawn: bool, answered: bool, deadline: u64 },
}

#[derive(Clone, Copy)]
struct Pending {
    from: usize,
    to: usize,
    source: Source,
    phase: Phase,
}

/// Which terminal is in the foreground, and the switch under way.
pub(super) struct Switching {
    /// The foreground terminal (0-based).
    pub(super) fg: usize,
    /// The foreground terminal's acquisition is complete.
    acquired: bool,
    /// Hold the displayed frame until its read-back verdict is recorded.
    check: Option<usize>,
    pending: Option<Pending>,
    /// A switch requested while one was being acquired; the latest wins.
    next: Option<(usize, Source)>,
    /// Counts acquisitions, so the render thread's frame is tied to one.
    pub(super) epoch: u64,
    /// The process holding the display (`sys_take_over_display`) and its terminal.
    display_owner: Option<(u64, usize)>,
    display_stack: Vec<(u64, usize)>,
    /// When the last acquisition began (monotonic seconds and nanoseconds).
    switched: (u64, u64),
}

impl Switching {
    pub(super) const fn new() -> Self {
        Self { fg: 0, acquired: true, check: None, pending: None, next: None, epoch: 0, display_owner: None, display_stack: Vec::new(), switched: (0, 0) }
    }
}

/// The foreground terminal, published for lock-free readers.
static FOREGROUND: AtomicUsize = AtomicUsize::new(0);
/// The foreground terminal is in `KD_GRAPHICS`: the kernel does not draw it.
static FG_GRAPHICS: AtomicBool = AtomicBool::new(false);
/// The display owner's terminal is not in the foreground: what it presents
/// does not reach the screen (contract section 4.2).
static PRESENTATION_SUSPENDED: AtomicBool = AtomicBool::new(false);
/// Display-owner presentations in progress, which a switch away waits out.
static PRESENTING: AtomicU32 = AtomicU32::new(0);
/// A switch is pending: the render thread passes at its short interval.
static SWITCHING: AtomicBool = AtomicBool::new(false);
/// A switch a key asked for, for the render thread to request: the terminal
/// (1-based) in the low byte and the chord's index in `keymap::CHORDS` above
/// it; 0 while no key has asked. A later key replaces one not yet taken.
static KEY_SWITCH: AtomicU32 = AtomicU32::new(0);
/// Threads blocked in `VT_WAITACTIVE`.
static WAITACTIVE: crate::task::waitqueue::WaitQueueHead = crate::task::waitqueue::WaitQueueHead::new();

/// A log line a change asks for, printed once the lock is released.
#[derive(Clone, Copy)]
enum Note {
    Foreground { vt: usize, source: Source },
    Refused { to: usize, from: usize, source: Source },
    RefusedByOwner { to: usize, from: usize },
    Cancelled { to: usize, from: usize },
    OwnerGone { vt: usize },
}

/// The work a change leaves for after the console's lock: signals to send,
/// `VT_WAITACTIVE` waiters to wake, lines to log and a switch to check.
#[derive(Default)]
pub(super) struct Effects {
    signals: [Option<(u64, u32)>; 4],
    notes: [Option<Note>; 4],
    wake: bool,
    /// The foreground terminal must be redrawn from its grid.
    pub(super) redraw: bool,
    /// The foreground changed, and with it the lock state keyboards show.
    leds: bool,
    /// Restore the display owner after its terminal returns to the foreground.
    #[cfg(target_arch = "aarch64")]
    restore_display: bool,
}

impl Effects {
    fn signal(&mut self, pid: u64, sig: u32) {
        if let Some(slot) = self.signals.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some((pid, sig));
        }
    }

    fn note(&mut self, note: Note) {
        if let Some(slot) = self.notes.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(note);
        }
    }

    /// Log, signal and wake, in thread context with no console lock held.
    pub(super) fn run(self) {
        for note in self.notes.into_iter().flatten() {
            match note {
                Note::Foreground { vt, source } => log::info!("[console] foreground tty{} ({})", vt + 1, source),
                Note::Refused { to, from, source } => log::info!(
                    "[console] switch to tty{} refused: tty{} display owned ({})",
                    to + 1,
                    from + 1,
                    source
                ),
                Note::RefusedByOwner { to, from } => {
                    log::info!("[console] switch to tty{} refused by tty{} owner", to + 1, from + 1)
                }
                Note::Cancelled { to, from } => log::info!(
                    "[console] switch to tty{} cancelled: tty{} owner did not release",
                    to + 1,
                    from + 1
                ),
                Note::OwnerGone { vt } => log::info!("[console] tty{} owner gone: mode reset", vt + 1),
            }
        }
        #[cfg(target_arch = "aarch64")]
        if self.restore_display { crate::gpu::vt_resumed(); }
        if self.redraw {
            super::REDRAW.store(true, Ordering::Release);
        }
        if self.wake {
            WAITACTIVE.wake_up();
        }
        // The keyboard driver sends the new LED state from its own thread
        // (the x86 xHCI boot keyboard's; ARM64's USB keyboard is V7's).
        if self.leds {
            #[cfg(target_arch = "x86_64")]
            crate::drivers::xhci::lock_state_changed();
        }
        for (pid, sig) in self.signals.into_iter().flatten() {
            crate::syscall::signal::signal_process_from_kernel(crate::process::ProcessId::new(pid), sig);
        }
    }
}

/// The console's terminals and switching state, as one borrow.
pub(super) struct Terminals<'a> {
    pub(super) vts: &'a mut [Vt; VT_COUNT],
    pub(super) sw: &'a mut Switching,
}

fn now_ms() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds * 1000 + nanos / 1_000_000
}

impl Terminals<'_> {
    /// Publish what lock-free readers see: the foreground, whether it is
    /// drawn by the kernel, and whether the display owner may present.
    fn publish(&mut self) {
        let fg = self.sw.fg;
        FOREGROUND.store(fg, Ordering::Release);
        let graphics = self.vts[fg].graphics.is_some();
        #[cfg(target_arch = "aarch64")]
        if graphics {
            crate::graphics::boot_ribbon::retire();
        }
        FG_GRAPHICS.store(graphics, Ordering::Release);
        let suspended = self.sw.display_owner.is_some_and(|(_, vt)| vt != fg);
        PRESENTATION_SUSPENDED.store(suspended, Ordering::SeqCst);
    }

    /// The foreground terminal belongs to a display owner that did not choose
    /// `VT_PROCESS`.
    fn display_owned_auto(&self) -> bool {
        let fg = self.sw.fg;
        self.sw.display_owner.is_some_and(|(owner, vt)| {
            vt == fg && self.vts[fg].process.is_none_or(|mode| mode.owner != owner)
        })
    }

    /// A switch request (section 4.4): `VT_ACTIVATE`, `chvt` or a key.
    pub(super) fn request(&mut self, to: usize, source: Source, effects: &mut Effects) -> Result<(), i32> {
        if to >= VT_COUNT {
            return Err(ENXIO);
        }
        // The previous switch's read-back holds the displayed frame. A release
        // leaves the foreground as it is, so it is asked for at once; any
        // other request waits for the verdict.
        if self.sw.check.is_some() && !self.releases_first(to) {
            self.sw.next = Some((to, source));
            return Ok(());
        }
        // This request supersedes one held during acquisition, including when
        // it starts a release before the previous frame's read-back finishes.
        self.sw.next = None;
        match self.sw.pending.as_mut() {
            Some(pending) => match pending.phase {
                Phase::Releasing { .. } if to == pending.from => {
                    // The releasing terminal asked for again: cancel, and tell an
                    // owner that already stopped drawing to resume.
                    let from = pending.from;
                    self.sw.pending = None;
                    if let Some(mode) = self.vts[from].process {
                        effects.signal(mode.owner, mode.acqsig);
                    }
                }
                Phase::Releasing { .. } => {
                    pending.to = to;
                    pending.source = source;
                }
                Phase::Acquiring { .. } | Phase::Released => self.sw.next = Some((to, source)),
            },
            None => self.start(to, source, effects)?,
        }
        Ok(())
    }

    /// A request that would start by asking the foreground terminal's
    /// `VT_PROCESS` owner to release it, or that acts on a switch pending.
    fn releases_first(&self, to: usize) -> bool {
        let fg = self.sw.fg;
        self.sw.pending.is_some()
            || (to != fg && to < VT_COUNT && !self.display_owned_auto() && self.vts[fg].process.is_some())
    }

    /// A request with no switch pending.
    fn start(&mut self, to: usize, source: Source, effects: &mut Effects) -> Result<(), i32> {
        let fg = self.sw.fg;
        if to == fg {
            return Ok(());
        }
        if self.display_owned_auto() {
            effects.note(Note::Refused { to, from: fg, source });
            return Err(EBUSY);
        }
        match self.vts[fg].process {
            Some(mode) => {
                let deadline = now_ms() + VT_RELEASE_TIMEOUT_MS;
                self.sw.pending = Some(Pending { from: fg, to, source, phase: Phase::Releasing { deadline } });
                effects.signal(mode.owner, mode.relsig);
            }
            None => self.acquire(fg, to, source, effects),
        }
        Ok(())
    }

    /// The acquiring step: `to` becomes the foreground terminal. Its frame is
    /// drawn by the render thread unless it is in `KD_GRAPHICS`, and its
    /// `VT_PROCESS` owner is told with `acqsig`.
    fn acquire(&mut self, from: usize, to: usize, source: Source, effects: &mut Effects) {
        if self.sw.check.is_some() {
            self.sw.pending = Some(Pending { from, to, source, phase: Phase::Released });
            return;
        }
        self.sw.switched = crate::time::get_monotonic_time_ns();
        self.sw.acquired = false;
        self.sw.epoch = self.sw.epoch.wrapping_add(1);
        let text = self.vts[to].graphics.is_none();
        let owner = self.vts[to].process;
        self.sw.pending = Some(Pending {
            from,
            to,
            source,
            phase: Phase::Acquiring {
                epoch: self.sw.epoch,
                drawn: !text,
                answered: owner.is_none(),
                deadline: now_ms() + VT_ACQUIRE_TIMEOUT_MS,
            },
        });
        self.sw.fg = to;
        effects.leds = true;
        #[cfg(target_arch = "aarch64")]
        { effects.restore_display = !text && self.sw.display_owner.is_some_and(|(_, vt)| vt == to); }
        // The screen goes to `to`: a process drawing on it is no longer shown.
        if text {
            super::DISPLAY_TAKEN.store(false, Ordering::SeqCst);
            effects.redraw = true;
        }
        self.publish();
        if let Some(mode) = owner {
            effects.signal(mode.owner, mode.acqsig);
        }
        self.settle(effects);
    }

    /// Complete the acquisition once its frame and its owner's answer are in,
    /// then start the switch held meanwhile.
    fn settle(&mut self, effects: &mut Effects) {
        let Some(pending) = self.sw.pending else {
            return;
        };
        let Phase::Acquiring { drawn: true, answered: true, .. } = pending.phase else {
            return;
        };
        self.sw.pending = None;
        self.sw.acquired = true;
        effects.wake = true;
        effects.note(Note::Foreground { vt: pending.to, source: pending.source });
        if self.vts[pending.to].graphics.is_none() {
            self.sw.check = Some(pending.to);
            return;
        }
        if let Some((to, source)) = self.sw.next.take() {
            let _ = self.request(to, source, effects);
        }
    }

    /// The render thread put frame `epoch` of the foreground terminal on the
    /// display, written and flushed.
    pub(super) fn drawn(&mut self, epoch: u64, effects: &mut Effects) {
        if let Some(Pending { phase: Phase::Acquiring { epoch: e, drawn, .. }, .. }) = self.sw.pending.as_mut() {
            if *e == epoch {
                *drawn = true;
            }
        }
        self.settle(effects);
    }

    /// Deadlines: an owner that does not release keeps its terminal; one that
    /// does not acknowledge an acquisition is taken as having done so.
    pub(super) fn tick(&mut self, effects: &mut Effects) {
        let Some(pending) = self.sw.pending.as_mut() else {
            return;
        };
        let now = now_ms();
        match &mut pending.phase {
            Phase::Releasing { deadline } if now >= *deadline => {
                let (to, from) = (pending.to, pending.from);
                self.sw.pending = None;
                effects.note(Note::Cancelled { to, from });
            }
            Phase::Acquiring { answered, deadline, .. } if !*answered && now >= *deadline => {
                *answered = true;
                self.settle(effects);
            }
            _ => {}
        }
    }

    /// `VT_RELDISP` on terminal `vt` from process `pid`.
    pub(super) fn reldisp(&mut self, vt: usize, pid: u64, answer: u64, effects: &mut Effects) -> Result<(), i32> {
        if self.vts[vt].process.is_none_or(|mode| mode.owner != pid) {
            return Err(EPERM);
        }
        let Some(pending) = self.sw.pending else {
            return Err(EINVAL);
        };
        match (answer, pending.phase) {
            (0 | 1, Phase::Releasing { .. }) if pending.from == vt => {
                if answer == 1 {
                    self.acquire(pending.from, pending.to, pending.source, effects);
                } else {
                    self.sw.pending = None;
                    effects.note(Note::RefusedByOwner { to: pending.to, from: vt });
                }
                Ok(())
            }
            (VT_ACKACQ, Phase::Acquiring { answered: false, .. }) if pending.to == vt => {
                if let Some(Pending { phase: Phase::Acquiring { answered, .. }, .. }) = self.sw.pending.as_mut() {
                    *answered = true;
                }
                self.settle(effects);
                Ok(())
            }
            _ => Err(EINVAL),
        }
    }

    /// `VT_SETMODE` on terminal `vt` from process `pid`.
    pub(super) fn set_mode(&mut self, vt: usize, pid: u64, mode: u8, relsig: i16, acqsig: i16) -> Result<(), i32> {
        let owner = self.vts[vt].process.map(|mode| mode.owner);
        match mode {
            VT_PROCESS => {
                let valid = |sig: i16| (1..=SIGNAL_MAX).contains(&sig);
                if !valid(relsig) || !valid(acqsig) {
                    return Err(EINVAL);
                }
                if owner.is_some_and(|owner| owner != pid)
                    || self.sw.display_owner.is_some_and(|(owner, owned)| owned == vt && owner != pid)
                {
                    return Err(EBUSY);
                }
                self.vts[vt].process = Some(ProcessMode { owner: pid, relsig: relsig as u32, acqsig: acqsig as u32 });
                Ok(())
            }
            VT_AUTO => {
                if owner.is_some_and(|owner| owner != pid) {
                    return Err(EPERM);
                }
                self.vts[vt].process = None;
                Ok(())
            }
            _ => Err(EINVAL),
        }
    }

    /// `VT_GETMODE`: mode, relsig and acqsig.
    pub(super) fn mode(&self, vt: usize) -> (u8, i16, i16) {
        match self.vts[vt].process {
            Some(mode) => (VT_PROCESS, mode.relsig as i16, mode.acqsig as i16),
            None => (VT_AUTO, 0, 0),
        }
    }

    /// `KDSETMODE` on terminal `vt` from process `pid`: `KD_GRAPHICS` (true)
    /// makes it the graphics owner, `KD_TEXT` redraws a foreground terminal.
    /// The display owner's own `KD_TEXT` gives the display back
    /// (`tty::ioctl`) before it reaches here.
    pub(super) fn set_kd(&mut self, vt: usize, pid: u64, graphics: bool, effects: &mut Effects) -> Result<(), i32> {
        if self.sw.display_owner.is_some_and(|(owner, owned)| owned == vt && owner != pid) {
            return Err(EBUSY);
        }
        let changed = self.vts[vt].graphics.is_some() != graphics;
        if graphics {
            self.vts[vt].graphics = Some(pid);
        } else {
            self.vts[vt].graphics = None;
            if vt == self.sw.fg {
                super::DISPLAY_TAKEN.store(false, Ordering::SeqCst);
                effects.redraw = true;
            }
        }
        if changed && vt == self.sw.fg {
            self.sw.epoch = self.sw.epoch.wrapping_add(1);
            if let Some(Pending { to, phase: Phase::Acquiring { epoch, drawn, .. }, .. }) = self.sw.pending.as_mut() {
                if *to == vt {
                    *epoch = self.sw.epoch;
                    *drawn = graphics;
                }
            }
            self.settle(effects);
        }
        self.publish();
        Ok(())
    }

    /// The render thread recorded the verdict before another VT may draw.
    pub(super) fn checked(&mut self, vt: usize, effects: &mut Effects) {
        if self.sw.check != Some(vt) {
            return;
        }
        self.sw.check = None;
        if let Some(Pending { from, to, source, phase: Phase::Released }) = self.sw.pending {
            self.acquire(from, to, source, effects);
            return;
        }
        if let Some((to, source)) = self.sw.next.take() {
            let _ = self.request(to, source, effects);
        }
    }

    pub(super) fn kd_graphics(&self, vt: usize) -> bool {
        self.vts[vt].graphics.is_some()
    }

    /// `sys_take_over_display` by process `pid`, whose terminal is `vt`.
    pub(super) fn take_display(&mut self, pid: u64, vt: usize, effects: &mut Effects) -> Result<(), i32> {
        if vt != self.sw.fg {
            return Err(EBUSY);
        }
        self.sw.display_owner = Some((pid, vt));
        self.set_kd(vt, pid, true, effects)?;
        self.publish();
        Ok(())
    }

    /// Stack a GPU owner, retaining the previous owner's terminal for give-back.
    #[cfg(any(test, target_arch = "aarch64"))]
    pub(super) fn take_display_nested(&mut self, pid: u64, vt: usize, effects: &mut Effects) -> Result<(), i32> {
        let previous = self.sw.display_owner;
        if previous == Some((pid, vt)) {
            return Ok(());
        }
        self.take_display(pid, vt, effects)?;
        if let Some(owner) = previous {
            self.sw.display_stack.push(owner);
        }
        Ok(())
    }

    /// The display is given back (by its owner, or for it by root).
    pub(super) fn give_back_display(&mut self, effects: &mut Effects) {
        if let Some((owner, vt)) = self.sw.display_owner.take() {
            if self.vts[vt].graphics == Some(owner) {
                let _ = self.set_kd(vt, owner, false, effects);
            }
        }
        // A saved owner may have released its VT to the foreground terminal. Restore
        // custody without requiring that VT to be foreground again.
        self.sw.display_owner = self.sw.display_stack.pop();
        if let Some((owner, vt)) = self.sw.display_owner {
            self.set_kd(vt, owner, true, effects).expect("restored display custody owns its VT");
        }
        effects.redraw = true;
        self.publish();
    }

    /// Process `pid` exited: the terminals it owned return to `VT_AUTO` and
    /// `KD_TEXT`, and a switch waiting for it proceeds (sections 4.3, 4.4).
    pub(super) fn exited(&mut self, pid: u64, effects: &mut Effects) -> bool {
        self.sw.display_stack.retain(|(owner, _)| *owner != pid);
        let mut display_released = false;
        if self.sw.display_owner.is_some_and(|(owner, _)| owner == pid) {
            self.sw.display_owner = self.sw.display_stack.pop();
            display_released = true;
        }
        for vt in 0..VT_COUNT {
            if self.vts[vt].process.is_some_and(|mode| mode.owner == pid) {
                self.vts[vt].process = None;
                if let Some(pending) = self.sw.pending {
                    match pending.phase {
                        // Its exit is a release.
                        Phase::Releasing { .. } if pending.from == vt => {
                            self.acquire(pending.from, pending.to, pending.source, effects);
                        }
                        Phase::Acquiring { .. } if pending.to == vt => {
                            if let Some(Pending { phase: Phase::Acquiring { answered, .. }, .. }) =
                                self.sw.pending.as_mut()
                            {
                                *answered = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if self.vts[vt].graphics == Some(pid) {
                self.vts[vt].graphics = None;
                effects.note(Note::OwnerGone { vt });
                if vt == self.sw.fg {
                    super::DISPLAY_TAKEN.store(false, Ordering::SeqCst);
                    effects.redraw = true;
                    // An acquisition that needed no frame now needs one.
                    self.sw.epoch = self.sw.epoch.wrapping_add(1);
                    let epoch_now = self.sw.epoch;
                    if let Some(Pending { phase: Phase::Acquiring { epoch, drawn, .. }, .. }) =
                        self.sw.pending.as_mut()
                    {
                        *epoch = epoch_now;
                        *drawn = false;
                    }
                }
            }
        }
        if display_released {
            if let Some((owner, vt)) = self.sw.display_owner {
                self.set_kd(vt, owner, true, effects).expect("restored display custody owns its VT");
            }
        }
        self.publish();
        self.settle(effects);
        display_released
    }

    /// `VT_WAITACTIVE`'s condition: `vt` is the foreground and acquired.
    /// A release in progress leaves `fg` acquired until the owner answers.
    pub(super) fn active(&self, vt: usize) -> bool {
        self.sw.fg == vt && self.sw.acquired
    }
}

// ---------------------------------------------------------------------------
// The interface the TTY layer, the syscalls and the render thread use.
// ---------------------------------------------------------------------------

/// Run `f` on the terminals under the console's lock, waiting for the lock;
/// then run what it left to do.
fn with_terminals<R>(f: impl FnOnce(&mut Terminals<'_>, &mut Effects) -> R) -> R {
    let mut f = Some(f);
    loop {
        let mut effects = Effects::default();
        let result = super::with_grid(|console| {
            let super::Console { vts, sw, .. } = console;
            let result = (f.take().expect("run once"))(&mut Terminals { vts, sw }, &mut effects);
            SWITCHING.store(sw.pending.is_some(), Ordering::Release);
            result
        });
        if let Some(result) = result {
            effects.run();
            return result;
        }
        // The lock stayed held through `with_grid`'s spins, before `f` ran.
        crate::task::scheduler::yield_current();
    }
}

/// The foreground terminal (1-based).
pub fn foreground() -> usize {
    FOREGROUND.load(Ordering::Acquire) + 1
}

/// A switch is pending.
pub(super) fn switching() -> bool {
    SWITCHING.load(Ordering::Acquire)
}

/// The kernel does not draw the foreground terminal: it is in `KD_GRAPHICS`.
pub(super) fn foreground_graphics() -> bool {
    FG_GRAPHICS.load(Ordering::Acquire)
}

/// `VT_ACTIVATE`: ask for a switch to terminal `vt` (1-based).
pub fn activate(vt: u64, source: Source) -> Result<(), i32> {
    if !(1..=VT_COUNT as u64).contains(&vt) {
        return Err(ENXIO);
    }
    with_terminals(|terminals, effects| terminals.request(vt as usize - 1, source, effects))
}

/// A key asks for a switch to terminal `vt` (1-based). Interrupt-safe: the
/// render thread makes the request (`take_key_switch`), so the switch, its log
/// line and any signal happen in thread context.
pub(super) fn key_switch(vt: usize, chord: u8) {
    KEY_SWITCH.store(vt as u32 | (chord as u32) << 8, Ordering::Release);
}

/// The terminal (1-based) a key asked for that the render thread has not taken yet.
pub(super) fn key_switch_pending() -> Option<usize> {
    match KEY_SWITCH.load(Ordering::Acquire) & 0xff {
        0 => None,
        vt => Some(vt as usize),
    }
}

/// Make the switch request a key left, from the render thread. A refusal is
/// logged by the request (section 4.4); the key is consumed either way.
pub(super) fn take_key_switch() {
    let switch = KEY_SWITCH.swap(0, Ordering::AcqRel);
    if switch == 0 {
        return;
    }
    let (vt, chord) = ((switch & 0xff) as usize, (switch >> 8) as usize);
    let source = Source::Key(super::keymap::CHORDS[chord]);
    let _ = with_terminals(|terminals, effects| terminals.request(vt - 1, source, effects));
}

/// `VT_WAITACTIVE`: block until terminal `vt` (1-based) is the foreground and
/// its acquisition is complete. Interruptible by signals.
pub fn wait_active(vt: u64) -> Result<(), i32> {
    use crate::task::waitqueue::PrepareOutcome;
    if !(1..=VT_COUNT as u64).contains(&vt) {
        return Err(ENXIO);
    }
    let index = vt as usize - 1;
    loop {
        // The condition is tested and the waiter published under the
        // console's lock, which a completion takes before it wakes waiters.
        let outcome = with_terminals(|terminals, _| {
            if terminals.active(index) {
                return None;
            }
            Some(WAITACTIVE.prepare_to_wait_checked(crate::task::thread::ThreadState::BlockedOnIO, None, || {
                !terminals.active(index)
            }))
        });
        let Some(outcome) = outcome else {
            return Ok(());
        };
        if outcome == PrepareOutcome::Mismatch {
            WAITACTIVE.finish_wait();
            return Ok(());
        }
        crate::syscall::blocking_io::wait_prepared(&WAITACTIVE, outcome)?;
    }
}

/// `VT_RELDISP` on terminal `vt` (1-based) from process `pid`.
pub fn reldisp(vt: usize, pid: u64, answer: u64) -> Result<(), i32> {
    with_terminals(|terminals, effects| terminals.reldisp(vt - 1, pid, answer, effects))
}

/// `VT_SETMODE` on terminal `vt` (1-based) from process `pid`.
pub fn set_mode(vt: usize, pid: u64, mode: u8, relsig: i16, acqsig: i16) -> Result<(), i32> {
    with_terminals(|terminals, _| terminals.set_mode(vt - 1, pid, mode, relsig, acqsig))
}

/// `VT_GETMODE` on terminal `vt` (1-based): mode, relsig, acqsig.
pub fn mode(vt: usize) -> (u8, i16, i16) {
    with_terminals(|terminals, _| terminals.mode(vt - 1))
}

/// `KDSETMODE` on terminal `vt` (1-based) from process `pid`. A terminal
/// that leaves the screen to its owner waits until the render thread is not
/// drawing, so the owner's first frame is not drawn over.
pub fn set_kd(vt: usize, pid: u64, graphics: bool) -> Result<(), i32> {
    with_terminals(|terminals, effects| terminals.set_kd(vt - 1, pid, graphics, effects))?;
    if graphics && foreground() == vt {
        super::wait_until_not_presenting();
    }
    Ok(())
}

/// `KDGETMODE` on terminal `vt` (1-based).
pub fn kd_graphics(vt: usize) -> bool {
    with_terminals(|terminals, _| terminals.kd_graphics(vt - 1))
}

/// `sys_take_over_display` by process `pid` on its terminal `vt` (1-based):
/// EBUSY unless that terminal is in the foreground.
pub fn take_display(pid: u64, vt: usize) -> Result<(), i32> {
    with_terminals(|terminals, effects| terminals.take_display(pid, vt - 1, effects))
}

/// The display owner's process ID and terminal (1-based).
pub fn display_owner() -> Option<(u64, usize)> {
    with_terminals(|terminals, _| terminals.sw.display_owner.map(|(pid, vt)| (pid, vt + 1)))
}

/// GPU takeover with nested VT custody retained until give-back.
#[cfg(target_arch = "aarch64")]
pub fn take_display_nested(pid: u64, vt: usize) -> Result<(), i32> {
    with_terminals(|terminals, effects| terminals.take_display_nested(pid, vt - 1, effects))
}

/// The display owner's terminal is not in the foreground: the owner keeps the display
/// but not the screen, which the kernel draws (contract section 4.2).
pub fn presentation_suspended() -> bool {
    PRESENTATION_SUSPENDED.load(Ordering::SeqCst)
}

/// `sys_give_back_display`.
pub fn give_back_display() {
    with_terminals(|terminals, effects| terminals.give_back_display(effects));
}

/// Process `pid` exited: reset the terminals it owned. True when it held the
/// display, which the caller then releases.
pub fn process_exited(pid: u64) -> bool {
    with_terminals(|terminals, effects| terminals.exited(pid, effects))
}

/// Deadlines and acquisitions, from the render thread.
pub(super) fn tick() {
    with_terminals(|terminals, effects| terminals.tick(effects));
}

/// The render thread put frame `epoch` on the display.
pub(super) fn drawn(epoch: u64) {
    with_terminals(|terminals, effects| terminals.drawn(epoch, effects));
}

pub(super) fn readback_pending() -> Option<usize> {
    with_terminals(|terminals, _| terminals.sw.check)
}

/// When the last acquisition began, for its read-back's report.
pub(super) fn switched_at() -> (u64, u64) {
    with_terminals(|terminals, _| terminals.sw.switched)
}

pub(super) fn checked(vt: usize) {
    with_terminals(|terminals, effects| terminals.checked(vt, effects));
}

/// A display owner's presentation, counted while it runs so a switch away
/// waits it out (`wait_for_presentations`). Refused while the owner's
/// terminal is not in the foreground: the caller then reports success and
/// does not present.
pub struct Presentation {
    counted: bool,
}

impl Presentation {
    /// A cursor pass shares the display owner's authority. Check and count it
    /// under the terminal lock so releasing ownership cannot race admission.
    pub fn begin_cursor() -> Option<Presentation> {
        with_terminals(|terminals, _| {
            let (owner, vt) = terminals.sw.display_owner?;
            // Syscall ownership is published only after the console and boot
            // screen have quiesced; VT ownership alone precedes that barrier.
            if crate::syscall::display::DISPLAY_OWNER_PID.load(Ordering::Acquire) == u64::MAX
                || crate::syscall::display::DISPLAY_OWNER_GROUP.load(Ordering::Acquire) != owner
                || vt != terminals.sw.fg {
                return None;
            }
            Self::begin(true)
        })
    }

    pub fn begin(owner: bool) -> Option<Presentation> {
        if !owner {
            return Some(Presentation { counted: false });
        }
        PRESENTING.fetch_add(1, Ordering::SeqCst);
        if PRESENTATION_SUSPENDED.load(Ordering::SeqCst) {
            PRESENTING.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Presentation { counted: true })
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        if self.counted {
            PRESENTING.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Wait until no display-owner or cursor presentation is still writing,
/// including a pass admitted before ownership was released.
pub(super) fn wait_for_presentations() -> bool {
    PRESENTING.load(Ordering::SeqCst) != 0
}

/// What the render thread copies for a frame: the foreground terminal's view.
pub(super) struct Frame {
    pub(super) fg: usize,
    pub(super) epoch: u64,
}

/// The foreground terminal, its acquisition epoch, and whether its grid
/// changed since `generation`.
pub(super) fn frame(terminals: &Terminals<'_>) -> Frame {
    Frame { fg: terminals.sw.fg, epoch: terminals.sw.epoch }
}

/// Reserve terminal `index`'s scrollback outside the console lock, before
/// keyboard input is enabled. An installed history is reused.
pub(super) fn ensure_history(index: usize) {
    static READY: [AtomicBool; VT_COUNT] = [const { AtomicBool::new(false) }; VT_COUNT];
    if READY[index].load(Ordering::Acquire) || crate::per_cpu::in_interrupt() {
        return;
    }
    let Some((cols, installed)) = super::with_grid(|console| {
        let grid = &console.vts[index].grid;
        (grid.cols(), grid.has_history())
    }) else {
        return;
    };
    if cols == 0 || installed {
        return;
    }
    if READY[index].swap(true, Ordering::AcqRel) {
        return;
    }
    let mut history = Vec::new();
    if history.try_reserve_exact(HISTORY_LINES * cols).is_err() {
        let first = super::with_grid(|console| !core::mem::replace(&mut console.vts[index].history_failed, true));
        if first == Some(true) {
            log::warn!("[console] tty{} scrollback unavailable", index + 1);
        }
        return;
    }
    history.resize(HISTORY_LINES * cols, super::grid::BLANK);
    let history: &'static mut [Cell] = history.leak();
    super::with_grid(move |console| console.vts[index].grid.set_history(history));
}

/// One read of a `vcs` device: rows, columns, cursor column and row, and
/// each cell of terminal `vt` (1-based; 0 for the foreground) as displayed.
pub fn screen(vt: usize) -> Option<(usize, usize, usize, usize, Vec<Cell>)> {
    let index = if vt == 0 { FOREGROUND.load(Ordering::Acquire) } else { vt - 1 };
    let (cols, rows) = super::with_grid(|console| (console.vts[index].grid.cols(), console.vts[index].grid.rows()))?;
    let mut cells = Vec::new();
    cells.try_reserve_exact(cols * rows).ok()?;
    cells.resize(cols * rows, super::grid::BLANK);
    super::with_grid(|console| {
        let grid = &console.vts[index].grid;
        if grid.cols() != cols || grid.rows() != rows {
            return None;
        }
        for row in 0..rows {
            cells[row * cols..(row + 1) * cols].copy_from_slice(grid.row(row));
        }
        let (cursor_row, cursor_col) = grid.cursor();
        Some((rows, cols, cursor_col, cursor_row, cells))
    })
    .flatten()
}

/// The grid of terminal `vt` (1-based): columns, rows.
pub fn size(vt: usize) -> (usize, usize) {
    super::with_grid(|console| (console.vts[vt - 1].grid.cols(), console.vts[vt - 1].grid.rows())).unwrap_or((0, 0))
}

/// Move terminal `vt`'s view (1-based) `lines` back (positive) or forward
/// (negative); the lines it is then back from the live screen.
pub fn scroll_view(vt: usize, lines: isize) -> usize {
    super::with_grid(|console| {
        let grid = &mut console.vts[vt - 1].grid;
        grid.scroll_view(lines);
        grid.view()
    })
    .unwrap_or(0)
}

/// Keyboard input reached terminal `vt` (1-based): its view returns to the
/// live screen (section 1.6).
pub fn input_returns_view(vt: usize) {
    super::with_grid(|console| {
        console.vts[vt - 1].grid.view_live();
    });
}
