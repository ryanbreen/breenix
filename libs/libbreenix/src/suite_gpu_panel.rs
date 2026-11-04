//! GPU-backed text panel; no CPU framebuffer mapping or pixel upload.
use crate::{gpu::{self, abi}, graphics};
use libgfx::surface::{Format, Surface};
use std::{ops::{Deref, DerefMut}, string::String};

pub(super) struct GpuPanel {
    surface: Surface,
    context: bgl::Context,
    target: u64,
    width: u32,
    height: u32,
}

impl GpuPanel {
    pub fn open() -> Result<Self, String> {
        let (r, caps) = gpu::caps();
        if r != 0 || caps.status != abi::status::ADMITTED {
            return Err(std::format!("CAPS status {} errno {}", caps.status, -r));
        }
        let (r, modes) = gpu::modes();
        if r != 0 { return Err(std::format!("MODES errno {}", -r)); }
        let mode = modes.modes.get(modes.current as usize).ok_or("MODES has no current mode")?;
        let surface = Surface::new(mode.width as usize, mode.height as usize, Format::BGRX)
            .map_err(|e| std::format!("panel surface: {e}"))?;
        let context = bgl::Context::new().map_err(|e| std::format!("panel context: {e}"))?;
        let target = gpu::scanout_target_create(abi::format::B8G8R8A8, mode.width, mode.height)
            .map_err(|e| std::format!("panel target: {e}"))?;
        Ok(Self { surface, context, target, width: mode.width, height: mode.height })
    }

    pub fn present(&mut self) -> Result<(), String> {
        self.surface.wait().map_err(|e| std::format!("panel draw: {e}"))?;
        self.context.begin(self.target, self.width, self.height);
        self.context.copy(self.surface.handle(), &[[0, 0, self.width as i32, self.height as i32, 0, 0]]);
        self.context.submit().and_then(|f| f.wait(2_000_000_000))
            .map_err(|e| std::format!("panel copy: {e}"))?;
        graphics::take_over_display().map_err(|e| std::format!("panel ownership: {e}"))?;
        self.context.present(self.target).and_then(|f| f.wait(2_000_000_000))
            .map_err(|e| std::format!("panel present: {e}"))
    }
}

impl Deref for GpuPanel {
    type Target = Surface;
    fn deref(&self) -> &Surface { &self.surface }
}
impl DerefMut for GpuPanel {
    fn deref_mut(&mut self) -> &mut Surface { &mut self.surface }
}
impl Drop for GpuPanel {
    fn drop(&mut self) { let _ = gpu::resource_destroy(self.target); }
}
