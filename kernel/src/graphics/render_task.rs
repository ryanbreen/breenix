//! Framebuffer render thread.
//!
//! This module provides a kernel thread that drains the render queue and
//! draws to the framebuffer. By running rendering on a dedicated thread with
//! its own large stack (1MB), we avoid stack overflow in syscall/interrupt context.
//!
//! The deep call stack through terminal_manager → terminal_pane → font rendering
//! requires approximately 500KB of stack space. Running this on a separate thread
//! isolates this from the main kernel stack.

use super::render_queue;
use core::sync::atomic::{AtomicBool, Ordering};

/// Stack size for the render thread (1MB - enough for deep font rendering)
const RENDER_STACK_SIZE: usize = 1024 * 1024;

/// Flag indicating if the render thread is running
static RENDER_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

/// Flag to signal the render thread to check for work
static RENDER_WAKE: AtomicBool = AtomicBool::new(false);

/// Spawn the render thread.
///
/// This should be called during kernel initialization when interactive mode is enabled.
/// Returns Ok(thread_id) on success, or Err if the thread couldn't be spawned.
pub fn spawn_render_thread() -> Result<u64, &'static str> {
    if RENDER_THREAD_RUNNING.load(Ordering::SeqCst) {
        return Err("Render thread already running");
    }

    // Spawn with a large stack
    let stack = crate::memory::stack::allocate_stack_with_privilege(
        RENDER_STACK_SIZE,
        crate::task::thread::ThreadPrivilege::Kernel,
    )?;

    let tls_block = {
        let thread_id = crate::tls::allocate_thread_tls()
            .map_err(|_| "Failed to allocate TLS for render thread")?;
        crate::tls::get_thread_tls_block(thread_id)
            .ok_or("Failed to get TLS block for render thread")?
    };

    let thread = alloc::boxed::Box::new(crate::task::thread::Thread::new(
        alloc::string::String::from("render"),
        render_thread_main,
        stack.top(),
        stack.bottom(),
        tls_block,
        crate::task::thread::ThreadPrivilege::Kernel,
    ));

    let tid = thread.id();

    // Leak the stack to keep it alive for the thread's lifetime
    core::mem::forget(stack);

    // Add to scheduler
    crate::task::scheduler::spawn(thread);

    RENDER_THREAD_RUNNING.store(true, Ordering::SeqCst);
    log::info!(
        "Render thread spawned with ID {} ({}KB stack)",
        tid,
        RENDER_STACK_SIZE / 1024
    );

    Ok(tid)
}

/// Main function for the render thread.
///
/// This runs forever, polling the render queue and rendering to framebuffer.
fn render_thread_main() {
    log::info!("Render thread started on dedicated 1MB stack");

    loop {
        // Check if there's work to do
        if render_queue::has_pending_data() {
            // Drain and render - this is where the deep stack usage happens
            // But we're on our own 1MB stack, so it's safe
            let rendered = render_queue::drain_and_render();
            if rendered > 0 {
                // Flush the framebuffer if we rendered anything
                flush_framebuffer();
            }
        }

        // Clear the wake flag
        RENDER_WAKE.store(false, Ordering::SeqCst);

        // Yield to let other threads run
        // The scheduler will come back to us eventually
        crate::task::scheduler::yield_current();
    }
}

/// Signal the render thread to wake up and check for work.
#[allow(dead_code)]
pub fn wake_render_thread() {
    RENDER_WAKE.store(true, Ordering::Release);
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
