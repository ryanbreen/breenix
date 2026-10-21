//! The window interface, version 2 (`docs/gui/breengel.md` sections 17 to 19).
//!
//! One registry, under one lock, holds every window: its buffer set and their states (free,
//! front, retired), the commit protocol's bases and damage log, the window's geometry, its
//! event ring, the compositor claim and the evidence holders' remnants. Nothing is copied to
//! or from user memory, and nothing waits, while the registry lock is held: pixel copies run
//! outside it on buffers held by a pin, and waits use each window's own wait queues.
//!
//! No path in this module writes to the serial log or any other kernel output (section 17.5).

pub mod events;
pub(crate) mod input_ring;
pub mod mem;
pub mod ops;
pub mod present;

use alloc::{collections::BTreeMap, collections::VecDeque, sync::Arc, vec::Vec};

use breengel_abi::errno::{EPERM, ESRCH};
use breengel_abi::{limits, Meta, Rect};

use crate::irq_safe_mutex::IrqSafeMutex;
use crate::task::waitqueue::WaitQueueHead;

use mem::Mem;
use crate::gpu::window_targets::Target;

/// The process making a call: the thread, its thread group (the owner identity a window
/// records) and its effective uid.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub tid: u64,
    pub tgid: u64,
    pub euid: u32,
}

impl Caller {
    pub fn current() -> Result<Caller, i32> {
        let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
        crate::process::with_process_manager(|manager| {
            manager.find_process_by_thread(tid).map(|(pid, process)| Caller {
                tid,
                tgid: process.thread_group_id.unwrap_or(pid.as_u64()),
                euid: process.cred.euid,
            })
        })
        .flatten()
        .ok_or(ESRCH)
    }

    pub fn is_root(&self) -> bool {
        self.euid == 0
    }
}

/// Where a buffer is in the commit protocol (section 17.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufState {
    /// Owned by the client.
    Free,
    /// Committed and immutable: the window's newest pixels.
    Front,
    /// Replaced, but still read by a composite, a capture copy or a replay.
    Retired,
}

/// One buffer of a window's set.
pub struct Buffer {
    /// Never reused; pins name their buffer by it.
    pub uid: u64,
    pub mem: Arc<Target>,
    /// The owner's GPU resource handle.
    pub addr: u64,
    pub state: BufState,
    /// Handed out by ACQUIRE and not committed since.
    pub acquired: bool,
    /// Taken by an ACQUIRE that is still replaying into it: neither free nor committable.
    pub replaying: bool,
    /// A COMMIT of this buffer is between its checks and its publication.
    pub committing: bool,
    /// The commit id whose pixels the buffer holds, 0 for none.
    pub base: u64,
    /// Composites, captures and replays reading the buffer now.
    pub pins: u32,
}

/// A commit's damage, in window coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    Rects(Vec<Rect>),
    Full,
}

impl Damage {
    pub fn none() -> Damage {
        Damage::Rects(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Damage::Rects(r) if r.iter().all(|r| r.is_empty()))
    }

    /// Add `other`, keeping at most `limits::MAX_DAMAGE` rectangles: past that the union
    /// is replaced by its bounding rectangle.
    pub fn add(&mut self, other: &Damage) {
        match (&mut *self, other) {
            (Damage::Full, _) => {}
            (_, Damage::Full) => *self = Damage::Full,
            (Damage::Rects(mine), Damage::Rects(theirs)) => {
                for r in theirs.iter().filter(|r| !r.is_empty()) {
                    if mine.try_reserve(1).is_err() {
                        *self = Damage::Full;
                        return;
                    }
                    mine.push(*r);
                }
                if mine.len() > limits::MAX_DAMAGE {
                    let b = bounds(mine);
                    mine.clear();
                    mine.push(b);
                }
            }
        }
    }

    /// The rectangles of the damage in a `width` x `height` window.
    pub fn rects(&self, width: u32, height: u32) -> Vec<Rect> {
        match self {
            Damage::Full => alloc::vec![Rect::new(0, 0, width as i32, height as i32)],
            Damage::Rects(r) => r.iter().copied().filter(|r| !r.is_empty()).collect(),
        }
    }
}

/// The bounding rectangle of `rects` (none empty).
pub fn bounds(rects: &[Rect]) -> Rect {
    let mut it = rects.iter().filter(|r| !r.is_empty());
    let Some(first) = it.next() else {
        return Rect::default();
    };
    let (mut x0, mut y0) = (first.x as i64, first.y as i64);
    let (mut x1, mut y1) = (x0 + first.w as i64, y0 + first.h as i64);
    for r in it {
        x0 = x0.min(r.x as i64);
        y0 = y0.min(r.y as i64);
        x1 = x1.max(r.x as i64 + r.w as i64);
        y1 = y1.max(r.y as i64 + r.h as i64);
    }
    Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
}

/// A window's wait queues, shared with the threads waiting on them, so a wait outlives the
/// window's destruction.
pub struct Waits {
    /// ACQUIRE waiting for a free buffer.
    pub acquire: WaitQueueHead,
    /// READ_EVENTS and the event descriptor waiting for an event.
    pub events: Arc<WaitQueueHead>,
    /// COMMIT, COMMIT_NO_CHANGE and RESIZE waiting for the window's commit gate.
    pub gate: WaitQueueHead,
}

/// One window.
pub struct Window {
    pub id: u64,
    /// The owner's thread group id: its kernel identity.
    pub owner: u64,
    pub registered: bool,
    pub title: [u8; limits::MAX_TITLE],
    pub title_len: usize,
    pub width: u32,
    pub height: u32,
    pub buffers: Vec<Buffer>,
    /// Captures copying each buffer index's tree page now: a buffer whose tree page is being
    /// copied is not handed out, which would make that page writable, even after a resize
    /// has replaced the buffer the capture pinned.
    pub tree_pins: [u32; limits::MAX_BUFFERS as usize],
    pub tree: Arc<Mem>,
    pub tree_addr: u64,
    pub tree_kib: u32,
    /// Index into `buffers` of the front, if any commit has been made.
    pub front: Option<usize>,
    /// The newest commit id (COMMIT and COMMIT_NO_CHANGE alike), 0 before the first.
    pub commit_id: u64,
    /// The commit whose pixels the front holds.
    pub front_commit: u64,
    pub pixel_gen: u64,
    /// The damage of the window's last `limits::DAMAGE_LOG` pixel commits, oldest first.
    pub log: VecDeque<(u64, Damage)>,
    /// The newest pixel commit that has left the log, 0 for none.
    pub log_floor: u64,
    pub x: i32,
    pub y: i32,
    pub z: u32,
    /// `breengel_abi::geometry_flags` bits.
    pub geometry: u32,
    pub meta: Meta,
    pub ring: events::Ring,
    pub waits: Arc<Waits>,
    /// The root process holding this window's evidence, if any.
    pub holder: Option<u64>,
    /// A COMMIT, COMMIT_NO_CHANGE or RESIZE is between its checks and its publication. They
    /// take turns, so each knows the commit id and pixel generation it will publish, and
    /// copies them out, before it publishes anything.
    pub gate: bool,
    pub present: present::WindowPresent,
}

impl Window {
    pub fn stride(&self) -> u32 {
        self.width * 4
    }

    /// Bytes of one buffer's tree page: buffer `i`'s starts `i * tree_len()` into `tree`.
    pub fn tree_len(&self) -> u64 {
        self.tree_kib as u64 * 1024
    }

    /// The buffer with pin key `uid`.
    pub fn buffer_mut(&mut self, uid: u64) -> Option<&mut Buffer> {
        self.buffers.iter_mut().find(|b| b.uid == uid)
    }

    /// Whether the window must be on screen: registered, visible and not minimized.
    pub fn presentation_required(&self) -> bool {
        use breengel_abi::geometry_flags::{MINIMIZED, VISIBLE};
        self.registered && self.geometry & VISIBLE != 0 && self.geometry & MINIMIZED == 0
    }

    /// The window on screen, `(x, y, w, h)`, when presentation-required.
    pub fn screen_rect(&self) -> Option<Rect> {
        self.presentation_required()
            .then(|| Rect::new(self.x, self.y, self.width as i32, self.height as i32))
    }
}

/// The compositor claim (section 18).
#[derive(Clone, Copy, Debug)]
pub struct Claim {
    pub holder: u64,
    pub epoch: u64,
}

/// A dead owner's last front and tree, kept for its evidence holder (section 17.3).
pub struct Remnant {
    pub holder: u64,
    pub width: u32,
    pub height: u32,
    pub commit_id: u64,
    pub pixels: Option<Arc<Target>>,
    pub tree: Arc<Mem>,
    /// Where in `tree` the front buffer's tree page starts, and its length.
    pub tree_offset: u64,
    pub tree_len: u64,
    /// Bytes charged to the holder's `RLIMIT_AS`.
    pub charge: u64,
}

pub struct Registry {
    pub windows: BTreeMap<u64, Window>,
    pub next_window: u64,
    pub next_uid: u64,
    pub claim: Option<Claim>,
    pub epoch: u64,
    pub remnants: BTreeMap<u64, Remnant>,
    pub present: present::State,
}

impl Registry {
    const fn new() -> Registry {
        Registry {
            windows: BTreeMap::new(),
            next_window: 1,
            next_uid: 1,
            claim: None,
            epoch: 0,
            remnants: BTreeMap::new(),
            present: present::State::new(),
        }
    }

    pub fn is_compositor(&self, caller: &Caller) -> bool {
        self.claim.is_some_and(|c| c.holder == caller.tgid)
    }

    /// The window `id` and whether `caller` may use it: `allow` is told whether the caller
    /// is the owner and whether it is the compositor. ENOENT for a stale id, EPERM when
    /// `allow` refuses.
    pub fn window(
        &mut self,
        id: u64,
        caller: &Caller,
        allow: impl FnOnce(bool, bool, &Caller) -> bool,
    ) -> Result<&mut Window, i32> {
        let compositor = self.is_compositor(caller);
        let w = self
            .windows
            .get_mut(&id)
            .ok_or(breengel_abi::errno::ENOENT)?;
        if allow(w.owner == caller.tgid, compositor, caller) {
            Ok(w)
        } else {
            Err(EPERM)
        }
    }

    pub fn uid(&mut self) -> u64 {
        let uid = self.next_uid;
        self.next_uid += 1;
        uid
    }
}

/// The registry. Taken for short sections only: no user copy, allocation of frames, wait or
/// process-manager acquisition happens while it is held.
pub static REGISTRY: IrqSafeMutex<Registry> = IrqSafeMutex::new(Registry::new());

/// The authority rules of section 17.4, as `allow` arguments of `Registry::window`.
pub mod who {
    use super::Caller;

    pub fn owner(owner: bool, _: bool, _: &Caller) -> bool {
        owner
    }
    pub fn compositor(_: bool, compositor: bool, _: &Caller) -> bool {
        compositor
    }
    pub fn owner_compositor_or_root(owner: bool, compositor: bool, caller: &Caller) -> bool {
        owner || compositor || caller.is_root()
    }
}

/// A thread is leaving: when it is the last running thread of its group, the group's windows
/// are destroyed (kept as remnants for their evidence holders), its compositor claim ends,
/// and the remnants it holds are freed. Called at the top of both exit paths, before the row
/// becomes a zombie its parent can reap, with no lock held.
pub fn thread_exit(thread_id: u64) {
    let Some((tgid, last)) = crate::process::with_process_manager(|manager| {
        let (pid, process) = manager.find_process_by_thread(thread_id)?;
        let tgid = process.thread_group_id.unwrap_or(pid.as_u64());
        let others_running = manager.thread_group_peers(pid).into_iter().any(|peer| {
            manager.get_process(peer).is_some_and(|row| {
                !row.is_terminated() && !row.exit_started && row.group_exit_code.is_none()
            })
        });
        Some((tgid, !others_running))
    })
    .flatten() else {
        return;
    };
    if last {
        ops::group_gone(tgid, true);
    }
}

/// The row `pid` is exiting by a kill or fault (`process::exit_process_and_retire`).
pub fn row_exit(pid: crate::process::ProcessId) {
    let Some((tgid, last)) = crate::process::with_process_manager(|manager| {
        let process = manager.get_process(pid)?;
        let tgid = process.thread_group_id.unwrap_or(pid.as_u64());
        let others_running = manager.thread_group_peers(pid).into_iter().any(|peer| {
            manager.get_process(peer).is_some_and(|row| {
                !row.is_terminated() && !row.exit_started && row.group_exit_code.is_none()
            })
        });
        Some((tgid, !others_running))
    })
    .flatten() else {
        return;
    };
    if last {
        ops::group_gone(tgid, true);
    }
}

/// The calling thread's group has replaced its program image: its windows are destroyed.
/// Its address space is new, so nothing is unmapped.
pub fn exec_done_current() {
    if let Ok(caller) = Caller::current() {
        ops::group_gone(caller.tgid, false);
    }
}

/// Registry charges beyond the caller's mapped tree pages: tree mapping holes and remnants.
/// GPU target backing is reserved by the GPU core. PROCESS_MANAGER held.
pub fn charged_bytes(tgid: u64, vmas: &[crate::memory::vma::Vma]) -> u64 {
    let reg = REGISTRY.lock();
    let remnants: u64 = reg.remnants.values().filter(|r| r.holder == tgid).map(|r| r.charge).sum();
    let unmapped: u64 = reg
        .windows
        .values()
        .filter(|w| w.owner == tgid)
        .map(|w| mem::unmapped_len(&w.tree, w.tree_addr, vmas))
        .sum();
    remnants + unmapped
}
