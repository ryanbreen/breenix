//! Published display custody shared by the architecture-specific display paths.

/// PID of the process that most recently took the display
/// (`sys_take_over_display`). One process owns the display at a time: a later
/// take-over moves ownership there, and the earlier owner's whole-screen
/// mapping can no longer draw.
pub(crate) static DISPLAY_OWNER_PID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);

/// The thread group of the process in `DISPLAY_OWNER_PID`, stored with it, so the GPU
/// core can test ownership by thread group without the process manager's lock (a take-over
/// from any thread of a process makes the whole process the owner).
pub(crate) static DISPLAY_OWNER_GROUP: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);
