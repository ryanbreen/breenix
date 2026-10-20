//! DX contexts and the MOBs behind them. A context's MOBs stay allocated until the device
//! has answered the buffer that destroys them; an errno alone cannot release them.

use alloc::sync::Arc;
use alloc::vec::Vec;
use super::{commands::Commands, memory::PageTree, wire, Error};
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

/// A DX context: its 8,192-byte state (`SVGADXContextMobFormat`) and seven context object
/// tables, each sized as entries times the published entry width (`svga3d_dx.h`).
pub struct Context {
    pub id: u32,
    pub state: Mob,
    pub cotables: Vec<(u32, Mob)>,
}

impl Context {
    /// `mobs` supplies eight reserved, distinct ids: state, render-target views,
    /// shader-resource views, element layouts, blend, depth/stencil, rasterizer and shader
    /// tables. `views` sizes both view tables and `shaders` the shader table; the fixed
    /// states have one entry each.
    pub fn new(id: u32, mobs: [u32; 8], views: u32, shaders: u32) -> Result<Self, Error> {
        use wire::{cotable as t, entry as e};
        let state = Mob::allocate(mobs[0], 8192)?;
        let tables = [
            (t::RTVIEW, e::RTVIEW, views),
            (t::SRVIEW, e::SRVIEW, views),
            (t::ELEMENTLAYOUT, e::ELEMENTLAYOUT, 1),
            (t::BLENDSTATE, e::BLENDSTATE, 1),
            (t::DEPTHSTENCIL, e::DEPTHSTENCIL, 1),
            (t::RASTERIZERSTATE, e::RASTERIZERSTATE, 1),
            (t::DXSHADER, e::DXSHADER, shaders),
        ];
        let mut cotables = Vec::new();
        cotables.try_reserve_exact(tables.len()).map_err(|_| Error::NoMemory)?;
        for (i, &(kind, width, count)) in tables.iter().enumerate() {
            cotables.push((kind, Mob::allocate(mobs[i + 1], width as u32 * count)?));
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
    /// Unbind and destroy the context, then its MOBs.
    pub fn destroy(&self, commands: &mut Commands) {
        commands.destroy_context(self.id);
        for (_, mob) in &self.cotables { commands.destroy_mob(mob.id); }
        commands.destroy_mob(self.state.id);
    }
}
