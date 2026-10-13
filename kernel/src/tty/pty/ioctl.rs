//! Pseudo-terminal ioctls: window size and the master's TIOC*PT* requests.

use super::PtyPair;
use crate::syscall::errno::{EFAULT, ENOTTY};
use crate::tty::ioctl::{Side, Winsize, TIOCGPTLCK, TIOCGPTN, TIOCGWINSZ, TIOCSPTLCK, TIOCSWINSZ};
use alloc::sync::Arc;
use core::sync::atomic::Ordering;

fn put<T: Copy>(arg: u64, value: T) -> Result<i32, i32> {
    if arg == 0 {
        return Err(EFAULT);
    }
    unsafe { core::ptr::write_volatile(arg as *mut T, value) };
    Ok(0)
}

fn get<T: Copy>(arg: u64) -> Result<T, i32> {
    if arg == 0 {
        return Err(EFAULT);
    }
    Ok(unsafe { core::ptr::read_volatile(arg as *const T) })
}

/// TIOCGWINSZ, TIOCSWINSZ, TIOCGPTN, TIOCSPTLCK and TIOCGPTLCK.
pub fn ioctl(pair: &Arc<PtyPair>, _side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    match request {
        TIOCGWINSZ => put(arg, *pair.winsize.lock()),
        TIOCSWINSZ => {
            let winsize: Winsize = get(arg)?;
            *pair.winsize.lock() = winsize;
            Ok(0)
        }
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
