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
        // Process all pending data before yielding to ensure responsive UI
        // This batches multiple render operations per scheduling quantum
        let mut total_rendered = 0;
        while render_queue::has_pending_data() {
            let rendered = render_queue::drain_and_render();
            if rendered == 0 {
                break; // Queue was empty or locked
            }
            total_rendered += rendered;
        }

        // Update mouse cursor position from tablet input device
        #[cfg(target_arch = "aarch64")]
        update_mouse_cursor();

        // Always flush — the render thread is the sole owner of GPU flushing.
        // Besides render queue text, the particle thread and log capture also
        // write pixels that need to be transferred to the VirtIO GPU.
        flush_framebuffer();

        // Yield to give other threads a chance to run
        crate::task::scheduler::yield_current();

        if total_rendered == 0 {
            // No work was done this iteration. Clear the wake flag and check
            // one more time for data before halting.
            //
            // NOTE: We intentionally do NOT use kthread_park/unpark here.
            // There is a fundamental race between checking for data and parking:
            // if data arrives after the check but before park sets parked=true,
            // wake_render_thread's kthread_unpark is lost (it sets parked=false
            // which park immediately overwrites with true). RENDER_WAKE then
            // stays stuck at true, so all future wakes are no-ops, permanently
            // freezing the render thread. Instead, we use WFI/HLT to sleep
            // until the next interrupt (timer at 200Hz = 5ms max latency).
            if !RENDER_WAKE.swap(false, Ordering::Acquire)
                && !render_queue::has_pending_data()
                && !log_capture::has_pending_data()
            {
                arch_halt();
            }
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
/// checks this flag, so worst-case latency is ~5ms. This avoids the
/// lost-wakeup race that existed with the previous kthread_park/unpark
/// approach.
pub fn wake_render_thread() {
    RENDER_WAKE.store(true, Ordering::Release);
}

/// Update the mouse cursor on the framebuffer if the tablet device is active.
///
/// Reads the current mouse position from the input driver atomics and
/// redraws the cursor sprite if the position has changed. This runs on
/// the render thread's stack, not in interrupt context.
#[cfg(target_arch = "aarch64")]
fn update_mouse_cursor() {
    if !crate::drivers::virtio::input_mmio::is_tablet_initialized() {
        return;
    }

    let (mx, my) = crate::drivers::virtio::input_mmio::mouse_position();

    if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        if let Some(mut fb_guard) = fb.try_lock() {
            super::cursor::update_cursor(&mut *fb_guard, mx as usize, my as usize);
        }
    }
}

/// Flush the framebuffer's double buffer if present.
fn flush_framebuffer() {
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(fb) = crate::logger::SHELL_FRAMEBUFFER.get() {
            if let Some(mut guard) = fb.try_lock() {
                if let Some(db) = guard.double_buffer_mut() {
                    db.flush_if_dirty();
                }
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
            // Use blocking lock — the render thread is not in interrupt context,
            // so it's safe to wait. try_lock() caused silent flush drops when the
            // particle animation thread held the lock, leaving text undrawn on screen.
            let guard = fb.lock();
            if let Err(e) = guard.flush_result() {
                // Log GPU flush failures to serial — these would otherwise be
                // silently swallowed, leaving the display stale.
                crate::serial_println!("[render] GPU flush failed: {}", e);
            }
        }
    }
}
