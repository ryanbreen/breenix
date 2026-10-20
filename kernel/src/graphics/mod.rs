//! ARM64 graphics: the framebuffer (`arm64_fb`), the boot and diagnostics
//! screens, the mouse cursor, and the primitives display owners draw with
//! through the fb syscalls. The screen console is `crate::console`.

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
        _ => CompositorBackend::None,
    }
}

pub mod arm64_fb;
pub mod boot_screen;
pub mod cursor;
pub mod double_buffer;
pub mod firmware_writes;
pub mod font;
pub mod primitives;
pub mod render_task;

pub use double_buffer::DoubleBufferedFrameBuffer;
