//! Byte-sized SVGA3D packets; source pins and disposition are recorded in gpu-core.md (#1913).

use alloc::vec::Vec;
use super::{memory::PageTree, wire::{self, cmd}};

/// A packet builder. Sizes are bytes, unlike the BGL stream's word counts.
pub struct Commands {
    words: Vec<u32>,
}

impl Default for Commands {
    fn default() -> Self { Self::new() }
}

impl Commands {
    pub fn new() -> Self { Self { words: Vec::new() } }
    pub fn words(&self) -> &[u32] { &self.words }
    pub fn command(&mut self, id: u32, payload: &[u32]) {
        self.words.extend_from_slice(&[id, (payload.len() * 4) as u32]);
        self.words.extend_from_slice(payload);
    }
    pub fn table(&mut self, kind: u32, tree: &PageTree, bytes: u32) {
        let ppn = tree.root_ppn();
        self.command(cmd::SET_OTABLE_BASE64, &[kind, ppn as u32, (ppn >> 32) as u32, bytes, 0, tree.depth()]);
    }
    pub fn mob(&mut self, id: u32, tree: &PageTree, bytes: u32) {
        let ppn = tree.root_ppn();
        self.command(cmd::DEFINE_MOB64, &[id, tree.depth(), ppn as u32, (ppn >> 32) as u32, bytes]);
    }
    pub fn destroy_mob(&mut self, id: u32) { self.command(cmd::DESTROY_MOB, &[id]); }
    pub fn surface(&mut self, id: u32, flags: u32, format: u32, width: u32, height: u32) {
        self.command(cmd::DEFINE_SURFACE, &[id, flags, format, 1, 0, 0, width, height, 1]);
    }
    pub fn bind_surface(&mut self, id: u32, mob: u32) { self.command(cmd::BIND_SURFACE, &[id, mob]); }
    pub fn destroy_surface(&mut self, id: u32) { self.command(cmd::DESTROY_SURFACE, &[id]); }
    pub fn update_surface(&mut self, id: u32) { self.command(cmd::UPDATE_SURFACE, &[id]); }
    pub fn readback_surface(&mut self, id: u32) { self.command(cmd::READBACK_SURFACE, &[id]); }
    pub fn context(&mut self, id: u32, mob: u32) {
        self.command(cmd::DX_DEFINE_CONTEXT, &[id]);
        self.command(cmd::DX_BIND_CONTEXT, &[id, mob, 0]);
    }
    pub fn cotable(&mut self, ctx: u32, mob: u32, kind: u32) {
        self.command(cmd::SET_COTABLE, &[ctx, mob, kind, 0]);
    }
    pub fn destroy_context(&mut self, id: u32) {
        self.command(cmd::DX_BIND_CONTEXT, &[id, wire::INVALID, 0]);
        self.command(cmd::DX_DESTROY_CONTEXT, &[id]);
    }
    pub fn screen(&mut self, width: u32, height: u32) {
        self.command(cmd::DEFINE_SCREEN, &[0, width, height, 0, 0, 1, 96]);
    }
    pub fn bind_screen(&mut self, surface: u32) { self.command(cmd::BIND_SCREEN, &[0, surface, 0, 0]); }
    pub fn update_screen(&mut self, width: u32, height: u32) {
        self.command(cmd::UPDATE_SCREEN, &[0, 0, 0, width, height]);
    }
    /// SURFACE_COPY image ids followed by one SVGA3dCopyBox.
    pub fn copy(&mut self, src: u32, dst: u32, source: [u32; 4], point: [u32; 2]) {
        let [sx, sy, width, height] = source;
        let [dx, dy] = point;
        self.command(cmd::SURFACE_COPY, &[src, 0, 0, dst, 0, 0, dx, dy, 0, width, height, 1, sx, sy, 0]);
    }
    pub fn fence(&mut self, mob: u32, value: u64) {
        self.command(cmd::MOB_FENCE64, &[value as u32, (value >> 32) as u32, mob, 0]);
    }
}
