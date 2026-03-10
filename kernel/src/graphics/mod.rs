//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

use core::sync::atomic::{AtomicU8, Ordering};

// ─── Compositor Backend Detection ────────────────────────────────────────────

/// GPU compositing backend, detected once at boot time.
///
/// Set during driver initialization based on which GPU hardware is present.
/// Once set, this never changes for the lifetime of the system. All
/// compositing syscalls dispatch through this — no per-call fallbacks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CompositorBackend {
    /// No GPU compositing available (GOP framebuffer only).
    None = 0,
    /// VirtIO GPU with VirGL 3D acceleration (Parallels, QEMU).
    VirGL = 1,
    /// VMware SVGA3 with STDU display pipeline (VMware Fusion).
    Svga3Stdu = 2,
}

static COMPOSITOR_BACKEND: AtomicU8 = AtomicU8::new(CompositorBackend::None as u8);

/// Set the active compositor backend (called once during boot).
pub fn set_compositor_backend(backend: CompositorBackend) {
    COMPOSITOR_BACKEND.store(backend as u8, Ordering::Release);
}

/// Get the active compositor backend.
pub fn compositor_backend() -> CompositorBackend {
    match COMPOSITOR_BACKEND.load(Ordering::Acquire) {
        1 => CompositorBackend::VirGL,
        2 => CompositorBackend::Svga3Stdu,
        _ => CompositorBackend::None,
    }
}

#[cfg(target_arch = "aarch64")]
pub mod arm64_fb;
#[cfg(target_arch = "x86_64")]
pub mod demo;
pub mod double_buffer;
pub mod font;
#[cfg(target_arch = "aarch64")]
pub mod particles;
pub mod primitives;
// Render queue/task enabled for ARM64 always, x86_64 with interactive feature
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub mod log_capture;
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub mod render_queue;
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub mod render_task;
pub mod split_screen;
pub mod terminal;

#[cfg(target_arch = "aarch64")]
pub mod cursor;

pub use double_buffer::DoubleBufferedFrameBuffer;
