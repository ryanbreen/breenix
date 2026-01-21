//! Render task - dedicated kernel thread for framebuffer rendering.
//!
//! This module spawns a kernel thread with a large stack (1MB) specifically
//! for handling the deep call stack of framebuffer rendering operations.
//!
//! The rendering call chain can be 500KB+ deep:
//! ```text
//! drain_and_render()
//!   → write_char()
//!     → terminal_pane::write_char()
//!       → draw_char()
//!         → draw_glyph()
//!           → font bitmap rendering
//!             → pixel-by-pixel framebuffer writes
//! ```
//!
//! By running this on a dedicated thread with a 1MB stack, we avoid
//! stack overflow on the main kernel stack (512KB) or IRQ stacks.

use crate::task::kthread::{kthread_run, kthread_should_stop, kthread_park, KthreadHandle};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::render_queue;

/// Handle to the render thread (for shutdown)
static RENDER_THREAD: Mutex<Option<KthreadHandle>> = Mutex::new(None);

/// Flag indicating render task is initialized
static RENDER_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Spawn the render thread.
///
/// This creates a dedicated kernel thread for framebuffer rendering.
/// The thread has a 1MB stack to handle the deep rendering call chain.
///
/// # Panics
/// Panics if called more than once.
pub fn spawn_render_thread() {
    if RENDER_INITIALIZED.swap(true, Ordering::AcqRel) {
        log::warn!("RENDER_TASK: Already initialized");
        return;
    }

    log::info!("RENDER_TASK: Spawning render thread...");

    match kthread_run(render_thread_main, "render") {
        Ok(handle) => {
            log::info!("RENDER_TASK: Render thread spawned successfully");
            *RENDER_THREAD.lock() = Some(handle);
        }
        Err(e) => {
            log::error!("RENDER_TASK: Failed to spawn render thread: {:?}", e);
            RENDER_INITIALIZED.store(false, Ordering::Release);
        }
    }
}

/// Main function for the render thread.
///
/// This runs in a loop, checking for pending render data and processing it.
/// The thread parks itself when there's no work, and is woken by producers.
fn render_thread_main() {
    log::info!("RENDER_TASK: Render thread started on dedicated stack");

    // Enable interrupts so we can be scheduled
    x86_64::instructions::interrupts::enable();

    // Mark render system as ready to accept bytes
    render_queue::set_ready();

    while !kthread_should_stop() {
        // Check for pending render data
        if render_queue::has_pending_data() {
            // Drain and render all pending bytes
            let rendered = render_queue::drain_and_render();

            if rendered > 0 {
                // Log occasionally for debugging (not every render to avoid spam)
                static RENDER_COUNT: core::sync::atomic::AtomicU64 =
                    core::sync::atomic::AtomicU64::new(0);
                let count = RENDER_COUNT.fetch_add(rendered as u64, Ordering::Relaxed);
                if count % 1000 == 0 && count > 0 {
                    log::debug!("RENDER_TASK: Total bytes rendered: {}", count + rendered as u64);
                }
            }
        } else {
            // No work - park until woken
            // Use a short yield instead of full park to be responsive
            // The kthread will be scheduled again on next timer tick
            crate::task::scheduler::yield_current();
            x86_64::instructions::hlt();
        }
    }

    log::info!("RENDER_TASK: Render thread stopping");
}

/// Shutdown the render thread.
#[allow(dead_code)]
pub fn shutdown_render_thread() {
    if !RENDER_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    log::info!("RENDER_TASK: Shutting down render thread");

    if let Some(handle) = RENDER_THREAD.lock().take() {
        let _ = crate::task::kthread::kthread_stop(&handle);
        let _ = crate::task::kthread::kthread_join(&handle);
    }

    RENDER_INITIALIZED.store(false, Ordering::Release);
}

/// Check if render task is initialized.
pub fn is_initialized() -> bool {
    RENDER_INITIALIZED.load(Ordering::Acquire)
}

/// Wake the render thread if it's parked.
///
/// Called by producers after queuing data to ensure timely rendering.
#[allow(dead_code)]
pub fn wake_render_thread() {
    if let Some(ref handle) = *RENDER_THREAD.lock() {
        crate::task::kthread::kthread_unpark(handle);
    }
}
