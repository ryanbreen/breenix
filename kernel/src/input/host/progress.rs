//! Fixed-storage witnesses for #2572. Recording never waits or formats output.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Count {
    Push,
    Pop,
    StreamPoll,
    StreamReturn,
    TaskPoll,
    TaskReturn,
    Enqueue,
    Unpark,
    UnparkReturn,
    AcceptEnter,
    AcceptReturn,
    Accepted,
    DrainEnter,
    DrainReturn,
    AckEnter,
    AckReturn,
    AcceptBusy,
    AcceptFull,
    AcceptRejected,
}
pub const COUNT: usize = Count::AcceptRejected as usize + 1;
pub const NAMES: [&str; COUNT] = [
    "push",
    "pop",
    "stream_poll",
    "stream_return",
    "task_poll",
    "task_return",
    "enqueue",
    "unpark",
    "unpark_return",
    "accept_enter",
    "accept_return",
    "accepted",
    "drain_enter",
    "drain_return",
    "ack_enter",
    "ack_return",
    "accept_busy",
    "accept_full",
    "accept_rejected",
];

pub struct Progress([AtomicU64; COUNT]);
impl Progress {
    pub const fn new() -> Self {
        Self([const { AtomicU64::new(0) }; COUNT])
    }
    #[inline]
    pub fn note(&self, count: Count) {
        self.0[count as usize].fetch_add(1, Ordering::Relaxed);
    }
    pub fn sample(&self) -> [u64; COUNT] {
        core::array::from_fn(|i| self.0[i].load(Ordering::Relaxed))
    }
}

#[derive(Clone, Copy)]
#[repr(u64)]
pub enum Site {
    Idle,
    Stream,
    Command,
    Accept,
    AcceptScreen,
    AcceptNotify,
    Service,
    Hardware,
    ServiceWait,
    Receiver,
    Ack,
    Decode,
    Publish,
    InputLock,
    InputPublish,
    InputWake,
    ConsoleKey,
    Tty,
    VtSwitch,
    Render,
    Restore,
    Replies,
    KeyboardDrain,
    Switch,
    Tick,
    VtLock,
    VtEffects,
    VtLog,
    VtRestore,
    VtWake,
    VtSignal,
    RenderSleep,
}
pub const SITES: [&str; Site::RenderSleep as usize + 1] = [
    "idle",
    "stream-await",
    "command",
    "accept",
    "accept-screen",
    "accept-notify",
    "service-lock",
    "hardware",
    "service-await",
    "receiver-try",
    "ack-output",
    "decode",
    "publish",
    "input-lock",
    "input-publish",
    "input-wake",
    "console-key",
    "tty",
    "vt-request",
    "render",
    "display-restore",
    "replies",
    "keyboard-drain",
    "vt-switch",
    "vt-tick",
    "vt-lock",
    "vt-effects",
    "vt-log",
    "vt-restore",
    "vt-wake",
    "vt-signal",
    "render-sleep",
];

pub struct Lane(AtomicU64);
impl Lane {
    pub const fn new() -> Self {
        Self(AtomicU64::new(Site::Idle as u64))
    }
    #[inline]
    pub fn enter(&self, site: Site) -> At<'_> {
        At {
            lane: self,
            previous: self.0.swap(site as u64, Ordering::Relaxed),
        }
    }
    pub fn site(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}
pub struct At<'a> {
    lane: &'a Lane,
    previous: u64,
}
impl At<'_> {
    #[inline]
    pub fn at(&self, site: Site) {
        self.lane.0.store(site as u64, Ordering::Relaxed);
    }
}
impl Drop for At<'_> {
    fn drop(&mut self) {
        self.lane.0.store(self.previous, Ordering::Relaxed);
    }
}

/// One writer claims 0 -> 1, publishes 2, and one reader consumes 2 -> 3.
/// A second overflow never waits for the writer or overwrites its evidence.
pub struct FirstFailure<T: Copy> {
    state: AtomicU8,
    value: UnsafeCell<T>,
}
// SAFETY: the state protocol gives one writer exclusive access until Release
// publication. The sole reader acquires that publication; storage is never reused.
unsafe impl<T: Copy + Send> Sync for FirstFailure<T> {}
impl<T: Copy> FirstFailure<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU8::new(0),
            value: UnsafeCell::new(value),
        }
    }
    pub fn capture(&self, sample: impl FnOnce(&mut T)) -> bool {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }
        // SAFETY: only the winner of 0 -> 1 writes, before publishing state 2.
        // Fill the slot directly: an IRQ caller must not build T on its stack.
        unsafe {
            sample(&mut *self.value.get());
        }
        self.state.store(2, Ordering::Release);
        true
    }
    pub fn take(&self) -> Option<T> {
        self.state
            .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        // SAFETY: state 2 publishes an initialized value; it is never modified again.
        Some(unsafe { *self.value.get() })
    }
}
