//! Revoking a terminal from the descriptions already open on it (vhangup).
//!
//! getty asks for this when it serves a line again, so that a process left
//! from the session that ended keeps no way to read the next login's input or
//! to write to its screen (`docs/users/login.md`, "Terminals"). Each open file
//! description records the open clock's value when it is created
//! (`FileDescriptor::opened`). Revoking a terminal advances the clock and
//! records the new value on the terminal, so the terminal's descriptions
//! created before then are revoked: a read returns end of file, and a write,
//! a terminal control or a metadata change fails with EIO; close succeeds.
//! Descriptions created afterwards, getty's reopened line among them, are
//! unaffected.
//!
//! A virtual terminal's stamp is here (tty1 is the console); a
//! pseudo-terminal's is its pair's (`PtyPair::revoked_since`).
use super::ctty::TtyRef;
use core::sync::atomic::{AtomicU64, Ordering};

static CLOCK: AtomicU64 = AtomicU64::new(0);

/// When each of tty1 to tty6 was last revoked.
static VT_REVOKED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// The open clock: the stamp a description created now carries.
pub fn now() -> u64 {
    CLOCK.load(Ordering::SeqCst)
}

/// Whether virtual terminal `number` (1-based) was revoked after a
/// description stamped `opened` was created.
pub fn vt_revoked_since(number: usize, opened: u64) -> bool {
    number
        .checked_sub(1)
        .and_then(|index| VT_REVOKED.get(index))
        .is_some_and(|at| at.load(Ordering::SeqCst) > opened)
}

/// Revoke `terminal` from the descriptions open on it now, then wake its
/// blocked readers and writers so they see it. Callers hold no lock.
pub fn revoke(terminal: &TtyRef) {
    let at = CLOCK.fetch_add(1, Ordering::SeqCst) + 1;
    match terminal {
        TtyRef::Pty(pair) => {
            pair.revoke(at);
            pair.slave_readers.wake_up();
            pair.slave_writers.wake_up();
        }
        TtyRef::Vt(tty) => {
            let number = tty.vt();
            if let Some(stamp) = VT_REVOKED.get(number - 1) {
                stamp.fetch_max(at, Ordering::SeqCst);
            }
            match crate::ipc::stdin::vt_input(number - 1) {
                Some(input) => input.readers.wake_up(),
                None => crate::ipc::stdin::INPUT_READERS.wake_up(),
            }
        }
    }
}
