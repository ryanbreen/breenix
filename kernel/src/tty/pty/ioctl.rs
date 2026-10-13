//! Pseudo-terminal ioctls: window size and the master's TIOC*PT* requests.

use super::PtyPair;
use crate::syscall::errno::{EFAULT, ENOTTY};
use crate::tty::ioctl::{Side, Winsize, TIOCGPTLCK, TIOCGPTN, TIOCGWINSZ, TIOCSPTLCK, TIOCSWINSZ};
use crate::signal::constants::SIGWINCH;
use crate::syscall::userptr::{copy_from_user, copy_to_user};
use crate::tty::ctty::TtyRef;
use crate::tty::jobctl;
use alloc::sync::Arc;
use core::sync::atomic::Ordering;

fn put<T: Copy>(arg: u64, value: T) -> Result<i32, i32> {
    copy_to_user(arg as *mut T, &value).map_err(|_| EFAULT)?;
    Ok(0)
}

fn get<T: Copy>(arg: u64) -> Result<T, i32> {
    copy_from_user(arg as *const T).map_err(|_| EFAULT)
}

/// TIOCGWINSZ, TIOCSWINSZ, TIOCGPTN, TIOCSPTLCK and TIOCGPTLCK.
pub fn ioctl(pair: &Arc<PtyPair>, side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    match request {
        TIOCGWINSZ => {
            let winsize = *pair.winsize.lock();
            put(arg, winsize)
        }
        TIOCSWINSZ => {
            let new: Winsize = get(arg)?;
            let changed = {
                let mut winsize = pair.winsize.lock();
                let old = *winsize;
                *winsize = new;
                (old.ws_row, old.ws_col, old.ws_xpixel, old.ws_ypixel)
                    != (new.ws_row, new.ws_col, new.ws_xpixel, new.ws_ypixel)
            };
            // A size change, and only a change, signals the foreground group.
            if changed {
                jobctl::signal_foreground(&TtyRef::Pty(pair.clone()), SIGWINCH);
            }
            Ok(0)
        }
        // The TIOC*PT* requests are the master's (Linux's ptm driver); a
        // slave fails them with ENOTTY.
        TIOCGPTN | TIOCSPTLCK | TIOCGPTLCK if side != Side::Master => Err(ENOTTY),
        TIOCGPTN => put(arg, pair.pty_num),
        TIOCSPTLCK => {
            let lock: i32 = get(arg)?;
            pair.locked.store(lock != 0, Ordering::SeqCst);
            Ok(0)
        }
        TIOCGPTLCK => put(arg, pair.locked.load(Ordering::SeqCst) as i32),
        _ => Err(ENOTTY),
    }
}
