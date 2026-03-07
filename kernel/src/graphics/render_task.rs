//! Framebuffer render thread.
//!
//! This module provides a kernel thread that drains the render queue and
//! draws to the framebuffer. By running rendering on a dedicated thread with
//! its own 512 KiB stack, we avoid stack overflow in syscall/interrupt context.

use super::render_queue;
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
use super::log_capture;
use crate::task::kthread::{kthread_run, kthread_should_stop, KthreadHandle};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Flag indicating if the render thread is running
static RENDER_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

/// Flag to signal the render thread to check for work
static RENDER_WAKE: AtomicBool = AtomicBool::new(false);

/// Flag set when a userspace process (BWM) takes display ownership.
/// When set, the render thread skips framebuffer flushing and cursor
/// updates — the display owner handles all GPU operations directly.
static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);

/// Handle to the render kthread (for potential cleanup/stopping)
static RENDER_KTHREAD: Mutex<Option<KthreadHandle>> = Mutex::new(None);

/// Spawn the render thread.
///
/// This should be called during kernel initialization when interactive mode is enabled.
/// Returns Ok(thread_id) on success, or Err if the thread couldn't be spawned.
pub fn spawn_render_thread() -> Result<u64, &'static str> {
    if RENDER_THREAD_RUNNING.load(Ordering::SeqCst) {
        return Err("Render thread already running");
    }

    // Use kthread API - it passes the function via RDI register, which works correctly
    // The kthread infrastructure provides 512 KiB stacks which is sufficient
    let handle = kthread_run(render_thread_main_kthread, "render")
        .map_err(|_| "Failed to spawn render kthread")?;

    let tid = handle.tid();

    // Store handle for potential future use (shutdown, etc.)
    *RENDER_KTHREAD.lock() = Some(handle);

    RENDER_THREAD_RUNNING.store(true, Ordering::SeqCst);
    log::info!(
        "Render thread spawned with ID {} using kthread API (512KB stack)",
        tid
    );

    Ok(tid)
}

/// Kthread entry point wrapper for the render thread.
///
/// This function is called via the kthread API (passed via RDI register).
/// It contains the main rendering loop and checks for shutdown signals.
///
/// CRITICAL: No logging allowed in this function! The render thread processes
/// the render queue. Logging here could cause deadlocks if the logger tries
/// to write to the render queue while this thread holds locks.
fn render_thread_main_kthread() {
    // Main rendering loop - runs until kthread_stop() is called
    while !kthread_should_stop() {
        // Always update the mouse cursor, even when BWM owns the display.
        // The cursor is drawn directly into the framebuffer VRAM; SVGA3 VRAM
        // traces detect the pixel writes and update the display automatically.
        #[cfg(target_arch = "aarch64")]
        update_mouse_cursor();

        // When BWM owns the display, the render thread only handles cursor.
        // BWM handles text drawing, terminal content, and GPU flushing via
        // fbdraw syscalls. Flush any cursor dirty rect, then sleep.
        if DISPLAY_TAKEN.load(Ordering::Acquire) {
            flush_framebuffer();
            arch_halt();
            continue;
        }

        let mut did_work = false;

        // Process all pending render queue data (terminal text, log capture)
        while render_queue::has_pending_data() {
            let rendered = render_queue::drain_and_render();
            if rendered == 0 {
                break; // Queue was empty or locked
            }
            did_work = true;
        }

        // Flush dirty regions to the GPU. Returns true if a flush happened.
        if flush_framebuffer() {
            did_work = true;
        }

        if did_work {
            // Work was done — loop immediately to check for more.
            // When bounce submits frames at 60fps, the render thread must
            // flush each one promptly. Yielding here caused the thread to
            // wait for rescheduling, dropping frames.
            // Timer preemption (200Hz) still prevents CPU starvation.
            continue;
        }

        // No work was done — yield CPU to other threads
        crate::task::scheduler::yield_current();

        // Check one more time for data before halting. We use WFI/HLT
        // to sleep until the next interrupt (timer at 200Hz = 5ms max).
        //
        // NOTE: We intentionally do NOT use kthread_park/unpark here.
        // There is a fundamental race between checking for data and parking:
        // if data arrives after the check but before park sets parked=true,
        // wake_render_thread's kthread_unpark is lost. Instead, WFI wakes
        // on any interrupt (timer tick) and we re-check all data sources.
        if !RENDER_WAKE.swap(false, Ordering::Acquire)
            && !render_queue::has_pending_data()
            && !log_capture::has_pending_data()
            && !has_pending_flush()
        {
            arch_halt();
        }
    }

    RENDER_THREAD_RUNNING.store(false, Ordering::SeqCst);
}

/// Architecture-specific halt (wait for interrupt).
#[inline(always)]
fn arch_halt() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi");
    }
    #[cfg(target_arch = "x86_64")]
    x86_64::instructions::hlt();
}

/// Signal the render thread to wake up and check for work.
///
/// Sets RENDER_WAKE so the render thread skips the WFI halt on its next
/// idle check. The render thread wakes on timer interrupts (200 Hz) and
/// checks this flag, so worst-case latency is ~1ms. This avoids the
/// lost-wakeup race that existed with the previous kthread_park/unpark
/// approach.
pub fn wake_render_thread() {
    RENDER_WAKE.store(true, Ordering::Release);
}

/// Mark that a userspace process has taken over display ownership.
/// The render thread will stop flushing the framebuffer and updating
/// the cursor — the display owner (BWM) handles GPU operations directly.
pub fn set_display_taken() {
    DISPLAY_TAKEN.store(true, Ordering::Release);
}

/// Check whether a userspace process owns the display.
pub fn is_display_taken() -> bool {
    DISPLAY_TAKEN.load(Ordering::Acquire)
}

/// Update the mouse cursor on the framebuffer if a pointing device is active.
///
/// Reads the current mouse position from whichever input driver is available:
/// VirtIO tablet (QEMU/Parallels) or XHCI USB HID mouse (VMware/Parallels).
/// Redraws the cursor sprite if the position has changed. This runs on
/// the render thread's stack, not in interrupt context.
#[cfg(target_arch = "aarch64")]
fn update_mouse_cursor() {
    let (mx, my) = if crate::drivers::virtio::input_mmio::is_tablet_initialized() {
        crate::drivers::virtio::input_mmio::mouse_position()
    } else if crate::drivers::usb::xhci::is_initialized() {
        crate::drivers::usb::hid::mouse_position()
    } else {
        return;
    };

    if mx == 0 && my == 0 {
        return;
    }

    if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        if let Some(mut fb_guard) = fb.try_lock() {
            if super::cursor::update_cursor(&mut *fb_guard, mx as usize, my as usize) {
                crate::graphics::arm64_fb::mark_dirty(
                    mx.saturating_sub(16),
                    my.saturating_sub(16),
                    32,
                    32,
                );
            }
        }
    }
}

/// Check if there's a pending GPU flush without consuming the dirty state.
fn has_pending_flush() -> bool {
    #[cfg(target_arch = "aarch64")]
    { crate::graphics::arm64_fb::has_dirty_rect() }
    #[cfg(not(target_arch = "aarch64"))]
    { false }
}

/// Flush the framebuffer if dirty. Returns true if a flush was performed.
fn flush_framebuffer() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(fb) = crate::logger::SHELL_FRAMEBUFFER.get() {
            if let Some(mut guard) = fb.try_lock() {
                if let Some(db) = guard.double_buffer_mut() {
                    db.flush_if_dirty();
                    // x86_64 double buffer handles its own dirty tracking;
                    // return false to avoid tight-looping on x86.
                    return false;
                }
            }
        }
        false
    }
    #[cfg(target_arch = "aarch64")]
    {
        // First, flush the double buffer (shadow → BAR0 copy) if present.
        // This must happen while holding SHELL_FRAMEBUFFER so the shadow
        // buffer isn't modified mid-copy.
        if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
            if let Some(mut fb_guard) = fb.try_lock() {
                if let Some(db) = fb_guard.double_buffer_mut() {
                    db.flush_if_dirty();
                }
            }
        }

        // Then flush dirty regions to the display (DSB + optional VirtIO hint).
        // No SHELL_FRAMEBUFFER lock needed here — we're not touching the pixel
        // buffer, just submitting GPU commands.
        if let Some((x, y, w, h)) = crate::graphics::arm64_fb::take_dirty_rect() {
            if let Err(e) = crate::graphics::arm64_fb::flush_dirty_rect(x, y, w, h) {
                crate::serial_println!("[render] GPU flush failed: {}", e);
            }
            true
        } else {
            false
        }
    }
}
