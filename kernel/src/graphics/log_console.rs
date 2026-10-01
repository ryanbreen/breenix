//! On-screen kernel log console (ARM64), opt-in with
//! `-fw_cfg name=opt/breenix/fbconsole,string=log`.
//!
//! Kernel log lines are drawn on the screen only in this mode; by default the
//! screen shows the boot screen and serial alone carries the log. Serial output
//! is tee'd into `log_capture`, and the render thread drains it into a
//! full-screen terminal pane here. The console stops when userspace starts
//! drawing, like the boot screen.

#![cfg(target_arch = "aarch64")]

use super::arm64_fb;
use super::log_capture;
use super::terminal::TerminalPane;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CONSOLE: Mutex<Option<TerminalPane>> = Mutex::new(None);

/// Start the log console over the whole framebuffer.
pub fn start() {
    let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return;
    };
    let mut guard = fb.lock();
    let mut pane = TerminalPane::new(4, 4, guard.width() - 8, guard.height() - 8);
    pane.clear(&mut *guard);
    drop(guard);
    *CONSOLE.lock() = Some(pane);
    log_capture::init();
    ACTIVE.store(true, Ordering::Release);
}

/// Whether the log console is drawing kernel log lines.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Stop drawing. Returns whether the console had been active.
pub fn stop() -> bool {
    ACTIVE.swap(false, Ordering::AcqRel)
}

/// Draw pending log bytes. Called by the render thread; returns whether it drew.
pub fn pump() -> bool {
    if !is_active() || !log_capture::has_pending_data() {
        return false;
    }
    let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return false;
    };
    let Some(mut console) = CONSOLE.try_lock() else {
        return false;
    };
    let Some(pane) = console.as_mut() else {
        return false;
    };
    let Some(mut guard) = fb.try_lock() else {
        return false;
    };
    let mut buf = [0u8; 512];
    let mut drew = false;
    while is_active() {
        let n = log_capture::drain(&mut buf);
        if n == 0 {
            break;
        }
        pane.write_bytes(&mut *guard, &buf[..n]);
        drew = true;
    }
    drew
}
