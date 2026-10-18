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
    pub fn from_words(words: Vec<u32>) -> Self { Self { words } }
    pub fn words(&self) -> &[u32] { &self.words }
    /// The byte offset the next command starts at.
    pub fn bytes(&self) -> u32 { (self.words.len() * 4) as u32 }
    pub fn into_words(self) -> Vec<u32> { self.words }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }
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
    /// DX_MOB_FENCE_64 at a byte offset of the fence MOB.
    pub fn fence_at(&mut self, mob: u32, offset: u32, value: u64) {
        self.command(cmd::MOB_FENCE64, &[value as u32, (value >> 32) as u32, mob, offset]);
    }
    pub fn nop_error(&mut self) { self.command(cmd::NOP_ERROR, &[]); }
    pub fn destroy_screen(&mut self) { self.command(cmd::DESTROY_SCREEN, &[0]); }
    pub fn unbind_screen(&mut self) { self.command(cmd::BIND_SCREEN, &[0, wire::INVALID, 0, 0]); }

    // DX commands (svga3d_dx.h); `ty` is an SVGA3dShaderType.
    pub fn dx_define_shader(&mut self, id: u32, ty: u32, bytes: u32) {
        self.command(cmd::DX_DEFINE_SHADER, &[id, ty, bytes]);
    }
    pub fn dx_bind_shader(&mut self, ctx: u32, id: u32, mob: u32, offset: u32) {
        self.command(cmd::DX_BIND_SHADER, &[ctx, id, mob, offset]);
    }
    pub fn dx_set_shader(&mut self, id: u32, ty: u32) { self.command(cmd::DX_SET_SHADER, &[id, ty]); }
    /// One RGBA render target and no depth-stencil view.
    pub fn dx_set_render_target(&mut self, view: u32) {
        self.command(cmd::DX_SET_RENDERTARGETS, &[wire::INVALID, view]);
    }
    pub fn dx_define_rtv(&mut self, view: u32, sid: u32, format: u32) {
        // Texture-2D view of mip 0, array slice 0, one slice.
        self.command(cmd::DX_DEFINE_RENDERTARGET_VIEW, &[view, sid, format, wire::value::RESOURCE_TEXTURE2D, 0, 0, 1]);
    }
    pub fn dx_destroy_rtv(&mut self, view: u32) { self.command(cmd::DX_DESTROY_RENDERTARGET_VIEW, &[view]); }
    pub fn dx_define_srv(&mut self, view: u32, sid: u32, format: u32) {
        // Most detailed mip 0, first array slice 0, one mip, one slice.
        self.command(cmd::DX_DEFINE_SHADERRESOURCE_VIEW, &[view, sid, format, wire::value::RESOURCE_TEXTURE2D, 0, 0, 1, 1]);
    }
    pub fn dx_destroy_srv(&mut self, view: u32) { self.command(cmd::DX_DESTROY_SHADERRESOURCE_VIEW, &[view]); }
    pub fn dx_set_shader_resource(&mut self, slot: u32, ty: u32, view: u32) {
        self.command(cmd::DX_SET_SHADER_RESOURCES, &[slot, ty, view]);
    }
    /// Vertex buffers 0 and 1: the vertices, `stride` bytes each, and the per-instance
    /// records, `record` bytes each, both from byte 0.
    pub fn dx_set_vertex_buffers(&mut self, vertices: u32, stride: u32, records: u32, record: u32) {
        self.command(cmd::DX_SET_VERTEX_BUFFERS, &[0, vertices, stride, 0, records, record, 0]);
    }
    pub fn dx_set_input_layout(&mut self, id: u32) { self.command(cmd::DX_SET_INPUT_LAYOUT, &[id]); }
    pub fn dx_set_topology(&mut self, topology: u32) { self.command(cmd::DX_SET_TOPOLOGY, &[topology]); }
    /// Blend factor 0 and sample mask 0xffffffff.
    pub fn dx_set_blend_state(&mut self, id: u32) {
        self.command(cmd::DX_SET_BLEND_STATE, &[id, 0, 0, 0, 0, u32::MAX]);
    }
    pub fn dx_set_depth_stencil_state(&mut self, id: u32) { self.command(cmd::DX_SET_DEPTHSTENCIL_STATE, &[id, 0]); }
    pub fn dx_set_rasterizer_state(&mut self, id: u32) { self.command(cmd::DX_SET_RASTERIZER_STATE, &[id]); }
    /// One viewport: binary32 bit patterns of x, y, width, height, then depth 0 to 1.
    pub fn dx_set_viewport(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.command(cmd::DX_SET_VIEWPORTS, &[0, x, y, width, height, 0, 0x3f80_0000]);
    }
    /// RGBA as binary32 bit patterns.
    pub fn dx_clear_rtv(&mut self, view: u32, rgba: [u32; 4]) {
        self.command(cmd::DX_CLEAR_RENDERTARGET_VIEW, &[view, rgba[0], rgba[1], rgba[2], rgba[3]]);
    }
    /// A `w` x `h` region of subresource 0 at (sx, sy) of `src` to (dx, dy) of `dst`.
    #[allow(clippy::too_many_arguments)]
    pub fn dx_copy_region(&mut self, dst: u32, dx: u32, dy: u32, src: u32, sx: u32, sy: u32, w: u32, h: u32) {
        self.command(cmd::DX_PRED_COPY_REGION, &[dst, 0, src, 0, dx, dy, 0, w, h, 1, sx, sy, 0]);
    }
    /// `count` vertices from vertex `start`, as one instance: instance `record` of the
    /// per-instance stream.
    pub fn dx_draw_instanced(&mut self, count: u32, start: u32, record: u32) {
        self.command(cmd::DX_DRAW_INSTANCED, &[count, 1, start, record]);
    }
    /// Input register 0: two binary32 values per vertex at offset 0 of buffer 0. Input
    /// registers 1 to `vectors`: four 32-bit unsigned values each, consecutive in a record
    /// of buffer 1, one record per instance.
    pub fn dx_define_element_layout(&mut self, id: u32, vectors: u32) {
        use wire::{format, value};
        let mut words = alloc::vec![id, 0, 0, format::R32G32_FLOAT, value::INPUT_PER_VERTEX, 0, 0];
        for r in 1..=vectors {
            words.extend_from_slice(&[1, 16 * (r - 1), format::R32G32B32A32_UINT, value::INPUT_PER_INSTANCE, 1, r]);
        }
        self.command(cmd::DX_DEFINE_ELEMENTLAYOUT, &words);
    }
    /// Blending off on the eight render targets, write mask 0xf, logic op off.
    pub fn dx_define_blend_state(&mut self, id: u32) {
        use wire::value::*;
        let mut words = alloc::vec![id, 0];
        for _ in 0..8 {
            // blendEnable, srcBlend, destBlend, blendOp; the alpha triple and write mask;
            // logicOpEnable, logicOp, padding (one byte each, little-endian).
            words.push(BLEND_ONE << 8 | BLEND_ZERO << 16 | BLENDEQ_ADD << 24);
            words.push(BLEND_ONE | BLEND_ZERO << 8 | BLENDEQ_ADD << 16 | WRITE_ALL << 24);
            words.push(0);
        }
        self.command(cmd::DX_DEFINE_BLEND_STATE, &words);
    }
    /// Depth and stencil tests off: depth disabled (DEPTH_WRITE_MASK_ALL, COMPARISON_ALWAYS),
    /// stencil disabled with STENCIL_KEEP operations.
    pub fn dx_define_depth_stencil_state(&mut self, id: u32) {
        use wire::value::*;
        let ops = STENCIL_KEEP | STENCIL_KEEP << 8 | STENCIL_KEEP << 16 | COMPARISON_ALWAYS << 24;
        self.command(cmd::DX_DEFINE_DEPTHSTENCIL_STATE, &[id, DEPTH_WRITE_ALL << 8 | COMPARISON_ALWAYS << 16, 0, ops, ops]);
    }
    /// Solid fill, no culling, no depth bias, depth clip on, no scissor, no multisample,
    /// line width 1.0.
    pub fn dx_define_rasterizer_state(&mut self, id: u32) {
        use wire::value::*;
        self.command(cmd::DX_DEFINE_RASTERIZER_STATE, &[id, FILL_SOLID | CULL_NONE << 8, 0, 0, 0, 1, 0x3f80_0000, 0]);
    }
    /// The screen target's rectangle at the origin.
    pub fn update_screen_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.command(cmd::UPDATE_SCREEN, &[0, x, y, width, height]);
    }
}
