//! Window events (section 19): each window's ring of 64 and its overflow accounting.

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

use breengel_abi::{event_kind, limits, Plain, WindowEvent};

use crate::task::waitqueue::WaitQueueHead;

/// A window's event ring. A full ring keeps what it holds and counts what it could not
/// queue; the count is delivered as an `Overflow` event after the kept events and before
/// any later one.
pub struct Ring {
    queue: VecDeque<WindowEvent>,
    lost: u64,
}

impl Ring {
    /// An empty ring with room for its 64 events, or `None` when that cannot be allocated.
    pub fn new() -> Option<Ring> {
        let mut queue = VecDeque::new();
        queue.try_reserve_exact(limits::EVENT_RING).ok()?;
        Some(Ring { queue, lost: 0 })
    }

    fn free(&self) -> usize {
        limits::EVENT_RING.saturating_sub(self.queue.len())
    }

    /// Admit compositor input without charging refused events as lost (B48).
    pub fn try_push(&mut self, event: WindowEvent) -> bool {
        // A pending kernel Overflow must keep its place before later input.
        if self.lost > 0 { return false; }
        super::event_admission::push(&mut self.queue, limits::EVENT_RING, event)
    }

    /// Queue a kernel event. False when it was counted as lost instead.
    pub fn push(&mut self, event: WindowEvent) -> bool {
        if self.lost > 0 {
            if self.free() >= 2 {
                self.queue.push_back(overflow(self.lost));
                self.lost = 0;
                self.queue.push_back(event);
                true
            } else {
                self.lost += 1;
                false
            }
        } else if self.free() >= 1 {
            self.queue.push_back(event);
            true
        } else {
            self.lost = 1;
            false
        }
    }

    /// Move up to `capacity` events into `out`, oldest first; when the ring empties with
    /// events lost and room left, an `Overflow` event carrying the count follows them.
    /// Returns the lost count that `Overflow` reported.
    pub fn take(&mut self, out: &mut Vec<WindowEvent>, capacity: usize) -> u64 {
        while out.len() < capacity {
            match self.queue.pop_front() {
                Some(e) => out.push(e),
                None => break,
            }
        }
        if self.queue.is_empty() && self.lost > 0 && out.len() < capacity {
            let lost = self.lost;
            out.push(overflow(lost));
            self.lost = 0;
            return lost;
        }
        0
    }

    /// Put back what `take` removed, when it could not be delivered, ahead of anything queued
    /// since: the events were already queued, so they go back even past the ring's 64 (new
    /// events still wait for room). Only an allocation failure counts one as lost, which the
    /// next `Overflow` reports.
    pub fn untake(&mut self, mut taken: Vec<WindowEvent>, lost: u64) {
        if lost > 0 {
            taken.pop();
            self.lost += lost;
        }
        while let Some(e) = taken.pop() {
            if self.queue.try_reserve(1).is_ok() {
                self.queue.push_front(e);
            } else {
                self.lost += 1;
            }
        }
    }

    pub fn has_event(&self) -> bool {
        !self.queue.is_empty() || self.lost > 0
    }
}

/// The current monotonic time in nanoseconds.
pub fn now_ns() -> u64 {
    let (sec, nsec) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

/// A kernel-made event of `kind`.
pub fn kernel_event(kind: u16) -> WindowEvent {
    let mut e = WindowEvent::ZERO;
    e.kind = kind;
    e.time_ns = now_ns();
    e
}

fn overflow(lost: u64) -> WindowEvent {
    let mut e = kernel_event(event_kind::OVERFLOW);
    e.lost = lost.min(u32::MAX as u64) as u32;
    e
}

// claim-lint:ok: Event validation is normative in docs/gui/breengel.md section 19.2 and B41 (#2312).
/// Whether a compositor may write `event` (section 19.2): a kind of 1 to 15 other than
/// `FontChanged`, which Breengel makes, and the three the kernel makes: `Resized` and
/// `ResizeFailed` (only a `RESIZE` of the window's buffers reports them, so a size in one is
/// the buffers' actual size), `CompositorChanged` (each claim's epoch); the compositor reports its own `Overflow`; text of
/// at most 240 bytes; the reserved field zero.
pub fn writable(event: &WindowEvent) -> bool {
    use event_kind::{COMPOSITOR_CHANGED, FONT_CHANGED, RESIZED, RESIZE_FAILED, RESIZE_REQUESTED};
    (1..=RESIZE_REQUESTED).contains(&event.kind)
        && ![FONT_CHANGED, RESIZED, RESIZE_FAILED, COMPOSITOR_CHANGED].contains(&event.kind)
        && (event.kind != RESIZE_REQUESTED
            || ((1..=limits::MAX_DIMENSION).contains(&event.width) && (1..=limits::MAX_DIMENSION).contains(&event.height)))
        && event.text_len as usize <= limits::EVENT_TEXT
        && event.reserved == 0
}

/// A window's event descriptor (`EVENT_FD`): readable for poll, select and epoll while the
/// window has an event queued, hung up once the window is destroyed.
pub struct EventFile {
    window: u64,
    queue: Arc<WaitQueueHead>,
}

impl core::fmt::Debug for EventFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventFile").field("window", &self.window).finish()
    }
}

impl EventFile {
    pub fn new(window: u64, queue: Arc<WaitQueueHead>) -> EventFile {
        EventFile { window, queue }
    }
}

/// The compositor's scene descriptor (`EVENT_FD` of window 0): readable for poll, select and
/// epoll while the scene has changed since it was made or last read; a read of at least 8
/// bytes returns the count of changes as a `u64` and marks them read.
pub struct SceneFile {
    queue: Arc<WaitQueueHead>,
    /// The change count when it was made or last read.
    read: core::sync::atomic::AtomicU64,
}

impl core::fmt::Debug for SceneFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SceneFile").finish()
    }
}

impl SceneFile {
    pub fn new(queue: Arc<WaitQueueHead>) -> SceneFile {
        let read = core::sync::atomic::AtomicU64::new(super::REGISTRY.lock().present.changes);
        SceneFile { queue, read }
    }
}

impl crate::ipc::object::IpcFile for SceneFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        st.st_mode = 0o600;
        st.st_nlink = 1;
        st.st_blksize = 4096;
    }

    fn poll_events(&self) -> i16 {
        if super::REGISTRY.lock().present.changes != self.read.load(core::sync::atomic::Ordering::Acquire) {
            crate::ipc::poll::events::POLLIN
        } else {
            0
        }
    }

    fn poll_queue(&self) -> Option<Arc<WaitQueueHead>> {
        Some(self.queue.clone())
    }

    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        if count < 8 {
            return Err(breengel_abi::errno::EINVAL as u64);
        }
        let changes = super::REGISTRY.lock().present.changes;
        crate::syscall::userptr::copy_to_user(buf as *mut u64, &changes)
            .map_err(|_| breengel_abi::errno::EFAULT as u64)?;
        self.read.store(changes, core::sync::atomic::Ordering::Release);
        Ok(8)
    }
}

impl crate::ipc::object::IpcFile for EventFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        st.st_mode = 0o600;
        st.st_nlink = 1;
        st.st_blksize = 4096;
    }

    fn poll_events(&self) -> i16 {
        use crate::ipc::poll::events::{POLLHUP, POLLIN};
        match super::REGISTRY.lock().windows.get(&self.window) {
            Some(w) if w.ring.has_event() => POLLIN,
            Some(_) => 0,
            None => POLLHUP,
        }
    }

    fn poll_queue(&self) -> Option<Arc<WaitQueueHead>> {
        Some(self.queue.clone())
    }
}
