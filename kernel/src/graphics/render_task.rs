//! Framebuffer render thread.
//!
//! This module provides a kernel thread that drains the render queue and
//! draws to the framebuffer. By running rendering on a dedicated thread with
//! its own stack, we avoid stack overflow in syscall/interrupt context.
//!
//! The deep call stack through terminal_manager → terminal_pane → font rendering
//! requires approximately 500KB of stack space. Running this on a separate thread
//! isolates this from the main kernel stack. The kthread API provides 512 KiB stacks
//! which is sufficient for the rendering workload.

use super::render_queue;
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
/// the render queue which routes to terminal_manager. If we log here, we'd try
/// to write to the logs terminal while the render thread holds locks, causing
/// a deadlock via IN_TERMINAL_CALL. Use raw serial output for debugging only.
fn render_thread_main_kthread() {
    // Raw serial character output - NO LOCKS
    use x86_64::instructions::port::Port;
    fn raw_char(c: u8) {
        unsafe {
            let mut port: Port<u8> = Port::new(0x3F8);
            port.write(c);
        }
    }

    // DEBUG: R = render thread started
    raw_char(b'R');
    raw_char(b'1');
    raw_char(b' ');

    // Main rendering loop - runs until kthread_stop() is called
    let mut iter_count = 0u32;
    while !kthread_should_stop() {
        // DEBUG: every 1000 iterations, print 'L'
        iter_count = iter_count.wrapping_add(1);
        if iter_count % 10000 == 0 {
            raw_char(b'L');
        }

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

        // Flush the framebuffer if we rendered anything
        if total_rendered > 0 {
            raw_char(b'F'); // DEBUG: F = flushing
            flush_framebuffer();
            // Yield after doing work to give other threads a chance
            crate::task::scheduler::yield_current();
        } else {
            // No work - park until woken by wake_render_thread()
            // This prevents busy-polling which would starve other kthreads like ksoftirqd
            RENDER_WAKE.store(false, Ordering::SeqCst);
            raw_char(b'P'); // DEBUG: P = parking
            crate::task::kthread::kthread_park();
            raw_char(b'U'); // DEBUG: U = unparked
        }
    }

    raw_char(b'X'); // DEBUG: X = shutting down
    RENDER_THREAD_RUNNING.store(false, Ordering::SeqCst);
}

/// Signal the render thread to wake up and check for work.
pub fn wake_render_thread() {
    RENDER_WAKE.store(true, Ordering::Release);
    // Unpark the render thread if it's parked
    if let Some(ref handle) = *RENDER_KTHREAD.lock() {
        crate::task::kthread::kthread_unpark(handle);
    }
}

/// Flush the framebuffer's double buffer if present.
fn flush_framebuffer() {
    if let Some(fb) = crate::logger::SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            if let Some(db) = guard.double_buffer_mut() {
                db.flush_if_dirty();
            }
        }
    }
}
