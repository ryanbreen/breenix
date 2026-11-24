//! POSIX.1-2024 make, "Asynchronous Events". E25 ruling 31 permits this FFI.

#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    io,
    sync::atomic::{AtomicI32, Ordering},
};

const IGNORE: usize = 1;
const ERROR: usize = usize::MAX;
const SIGNALS: [i32; 4] = [1, 2, 3, 15]; // HUP, INT, QUIT and TERM on supported hosts.
const KILL: i32 = 9;
static PENDING: AtomicI32 = AtomicI32::new(0);

extern "C" {
    fn signal(number: i32, handler: usize) -> usize;
    fn kill(pid: i32, number: i32) -> i32;
}

extern "C" fn remember(number: i32) {
    // Signal context may only record the first interruption. The runner
    // deals with children, tokens, diagnostics and the filesystem.
    let _ = PENDING.compare_exchange(0, number, Ordering::Relaxed, Ordering::Relaxed);
}

pub(crate) fn pending() -> Option<i32> {
    let number = PENDING.load(Ordering::Relaxed);
    (number != 0).then_some(number)
}

pub(crate) fn interrupts_recipe(number: i32) -> bool {
    SIGNALS.contains(&number)
}

pub(crate) struct Handlers {
    previous: Vec<(i32, usize)>,
}

impl Handlers {
    pub(crate) fn install() -> io::Result<Self> {
        PENDING.store(0, Ordering::Relaxed);
        let mut handlers = Self {
            previous: Vec::new(),
        };
        for number in SIGNALS {
            let old = set_handler(number, remember as *const () as usize)?;
            handlers.previous.push((number, old));
            if old == IGNORE {
                set_handler(number, old)?;
            }
        }
        Ok(handlers)
    }
}

impl Drop for Handlers {
    fn drop(&mut self) {
        for (number, handler) in self.previous.drain(..) {
            let _ = set_handler(number, handler);
        }
        PENDING.store(0, Ordering::Relaxed);
    }
}

fn set_handler(number: i32, handler: usize) -> io::Result<usize> {
    // SAFETY: handler is a POSIX disposition or an extern "C" callback with
    // the signal handler signature. The pinned Unix ABIs use pointer-sized values.
    let previous = unsafe { signal(number, handler) };
    if previous == ERROR {
        Err(io::Error::last_os_error())
    } else {
        Ok(previous)
    }
}

pub(crate) fn interrupt_child(
    pid: u32,
    own_group: bool,
    number: i32,
    force: bool,
) -> io::Result<()> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    let destination = if own_group { -pid } else { pid };
    let number = if force { KILL } else { number };
    // SAFETY: the caller owns the unreaped child and, when requested, its
    // process group. kill accepts only the numeric destination and signal.
    let result = unsafe { kill(destination, number) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
