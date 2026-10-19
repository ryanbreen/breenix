//! Framebuffer render thread (ARM64, displays without VirGL).
//!
//! The thread copies the shell framebuffer's double buffer to the screen and
//! flushes its dirty rectangle to the display for the current foreground
//! display owner, and draws the mouse cursor for that owner.
//! The screen console draws with its own thread (`crate::console`).

use crate::task::kthread::{kthread_run, kthread_should_stop, KthreadHandle};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Flag indicating if the render thread is running
static RENDER_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

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
        // After GPU-core handoff the GPU kernel console owns the screen.
        #[cfg(target_arch = "aarch64")]
        if crate::gpu::display_active() {
            crate::gpu::console::pump();
            wait_for_next_tick();
            continue;
        }
        // Only the current foreground display owner authorizes cursor writes
        // and framebuffer flushes. The token keeps the console from repainting
        // until a pass admitted before a switch or display return has finished.
        if let Some(_presenting) = crate::console::vt::Presentation::begin_cursor() {
            update_mouse_cursor();
            flush_framebuffer();
        }
        wait_for_next_tick();
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

/// Update the mouse cursor on the framebuffer if a pointing device is active.
///
/// Reads the current mouse position from whichever input driver is available:
/// VirtIO tablet (QEMU/Parallels) or XHCI USB HID mouse (VMware/Parallels).
/// Redraws the cursor sprite if the position has changed. This runs on
/// the render thread's stack, not in interrupt context.
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
            // Marks both the old and the new cursor rectangle dirty.
            super::cursor::update_cursor(&mut *fb_guard, mx as usize, my as usize);
        }
    }
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
