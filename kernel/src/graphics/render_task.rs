//! Framebuffer render thread (ARM64, displays without VirGL).
//!
//! The thread copies the shell framebuffer's double buffer to the screen and
//! flushes its dirty rectangle to the display, for the boot screen and for a
//! process that owns the display, and draws the mouse cursor for that owner.
//! The screen console draws with its own thread (`crate::console`).

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
/// No logging in this function: it runs with interrupts enabled (see
/// `boot_screen::presenter_main`).
fn render_thread_main_kthread() {
    // Main rendering loop - runs until kthread_stop() is called
    while !kthread_should_stop() {
        // When BWM owns the display, the render thread draws the mouse cursor
        // into the framebuffer VRAM (SVGA3 VRAM traces detect the pixel writes
        // and update the display), flushes its dirty rect, then sleeps. BWM
        // handles the rest of the drawing and GPU flushing via fbdraw syscalls.
        // Until then the screen is the kernel's, and a cursor would be drawn
        // over the console's pixels.
        if DISPLAY_TAKEN.load(Ordering::Acquire) {
            update_mouse_cursor();
            flush_framebuffer();
            wait_for_next_tick();
            continue;
        }

        // Flush dirty regions to the GPU. Returns true if a flush happened.
        if flush_framebuffer() {
            // Work was done — loop immediately to check for more.
            // When bounce submits frames at 60fps, the render thread must
            // flush each one promptly. Yielding here caused the thread to
            // wait for rescheduling, dropping frames.
            // Timer preemption (200Hz) still prevents CPU starvation.
            continue;
        }

        // No work was done — yield CPU to other threads
        crate::task::scheduler::yield_current();

        // Check one more time for data before waiting for the next tick.
        //
        // NOTE: We intentionally do NOT use kthread_park/unpark here.
        // There is a fundamental race between checking for data and parking:
        // if data arrives after the check but before park sets parked=true,
        // wake_render_thread's kthread_unpark is lost. Instead, the thread
        // runs again on the next timer tick and re-checks all data sources.
        if !RENDER_WAKE.swap(false, Ordering::Acquire) && !has_pending_flush() {
            wait_for_next_tick();
        }
    }

    RENDER_THREAD_RUNNING.store(false, Ordering::SeqCst);
}

/// One ARM64 timer tick: the timer runs at 1 kHz.
const TICK_NS: u64 = 1_000_000;

/// Wait until the next timer tick, when the loop re-checks its work.
///
/// On aarch64 the render thread sleeps on a one-tick timer and gives up its
/// CPU at once. It used to wait in WFI as the CPU's current thread, which
/// kept every thread queued on that CPU waiting until something rescheduled
/// it. While the suite runner owns the display the loop never yields, so
/// that was the 10-tick quantum: a ring of `sched_yield` processes saw its
/// members queued there wait about 12 ms to run (#1177). Asleep, the thread
/// leaves the CPU to the threads queued on it or to its idle thread, which
/// other CPUs wake with a reschedule IPI when they give it work.
fn wait_for_next_tick() {
    use crate::task::{scheduler, thread::ThreadState};

    let Some(tid) = scheduler::current_thread_id() else {
        arch_halt();
        return;
    };
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let wake_at = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos)
        .saturating_add(TICK_NS);
    scheduler::with_current(|sched| sched.block_current_for_timer(wake_at));
    loop {
        // The switch below answers any reschedule asked of this CPU, as the
        // idle loop's does; left set, the request would preempt the thread
        // this CPU switches to.
        let _ = scheduler::check_and_clear_need_resched();
        scheduler::schedule();
        if scheduler::thread_state(tid) != Some(ThreadState::BlockedOnTimer) {
            break;
        }
    }
}

/// Architecture-specific halt (wait for interrupt).
///
/// A private park primitive, kept separate from `crate::arch_halt` so it
/// keeps the exact `asm!` it emits today. It carries the same #772 park bump
/// the shared primitives do, so the render thread's wait when it has no
/// scheduler identity is counted the way the loops that use the shared
/// primitives are.
#[inline(always)]
fn arch_halt() {
    crate::per_cpu::note_wait_loop_park();
    unsafe {
        core::arch::asm!("wfi");
    }
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
fn update_mouse_cursor() {
    let (mx, my) = if crate::drivers::virtio::input_mmio::is_tablet_initialized() {
        crate::drivers::virtio::input_mmio::mouse_position()
    } else if crate::drivers::xhci::is_initialized() {
        crate::input::pointer::mouse_position()
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
    crate::graphics::arm64_fb::has_dirty_rect()
}

/// Flush the framebuffer if dirty. Returns true if a flush was performed.
fn flush_framebuffer() -> bool {
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
