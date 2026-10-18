//! Owned objects used by DX contexts and surfaces. Submission holds retain these trees
//! until the device has ended its access; an errno alone cannot release them.

use alloc::sync::Arc;
use alloc::vec::Vec;
use super::{commands::Commands, memory::PageTree, Error};
use crate::gpu::dma::{Pages, PAGE};

pub struct Mob {
    pub id: u32,
    pub tree: Arc<PageTree>,
    pub bytes: u32,
}

impl Mob {
    pub fn new(id: u32, data: Arc<Pages>, bytes: u32) -> Result<Self, Error> {
        if bytes == 0 || bytes as usize > data.bytes() { return Err(Error::InvalidCommand); }
        let tree = Arc::new(PageTree::new(data).ok_or(Error::NoMemory)?);
        Ok(Self { id, tree, bytes })
    }
    pub fn allocate(id: u32, bytes: u32) -> Result<Self, Error> {
        let data = Arc::new(Pages::new((bytes as usize).div_ceil(PAGE)).ok_or(Error::NoMemory)?);
        Self::new(id, data, bytes)
    }
    pub fn define(&self, commands: &mut Commands) { commands.mob(self.id, &self.tree, self.bytes); }
}

/// DX state is 8192 bytes, the published SVGADXContextMobFormat. A cotable's capacity
/// follows the resource-handle quota plus three backend scratch views, or the fixed
/// catalogue/state object count; entry widths come from svga3d_dx.h's packed structs.
pub struct Context {
    pub id: u32,
    pub state: Mob,
    pub cotables: Vec<(u32, Mob)>,
}

impl Context {
    /// `mobs` supplies eight reserved, distinct ids: state, RTV, SRV, layout, blend,
    /// depth/stencil, rasterizer and shader table. Reserving ids is the backend's job.
    pub fn new(id: u32, mobs: [u32; 8]) -> Result<Self, Error> {
        let state = Mob::allocate(mobs[0], 8192)?;
        let tables = [(0, 32, 256 + 3), (2, 32, 256 + 3), (3, 1024, 1),
            (4, 128, 1), (5, 16, 1), (6, 32, 1), (10, 32, 17 * 2)];
        let mut cotables = Vec::new();
        cotables.try_reserve_exact(tables.len()).map_err(|_| Error::NoMemory)?;
        for (i, &(kind, width, count)) in tables.iter().enumerate() {
            cotables.push((kind, Mob::allocate(mobs[i + 1], width * count)?));
        }
        Ok(Self { id, state, cotables })
    }
    pub fn define(&self, commands: &mut Commands) {
        self.state.define(commands);
        for (_, mob) in &self.cotables { mob.define(commands); }
        commands.context(self.id, self.state.id);
        for (kind, mob) in &self.cotables { commands.cotable(self.id, mob.id, *kind); }
    }
    pub fn hold(&self) -> Vec<Arc<PageTree>> {
        core::iter::once(self.state.tree.clone())
            .chain(self.cotables.iter().map(|(_, mob)| mob.tree.clone())).collect()
    }
    /// Unbind and destroy the context before retiring its MOBs. The caller retains this
    /// object through the command buffer's completion (including an error completion).
    pub fn destroy(&self, commands: &mut Commands) {
        commands.destroy_context(self.id);
        for (_, mob) in &self.cotables { commands.destroy_mob(mob.id); }
        commands.destroy_mob(self.state.id);
    }
}

pub struct Surface {
    pub id: u32,
    pub mob: Mob,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
}

impl Surface {
    pub fn define(&self, commands: &mut Commands) {
        self.mob.define(commands);
        commands.surface(self.id, self.flags, self.format, self.width, self.height);
        commands.bind_surface(self.id, self.mob.id);
    }
    pub fn destroy(&self, commands: &mut Commands) {
        commands.bind_surface(self.id, super::wire::INVALID);
        commands.destroy_surface(self.id);
        commands.destroy_mob(self.mob.id);
    }
}
