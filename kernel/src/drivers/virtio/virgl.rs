//! VirGL Command Encoder
//!
//! Encodes Gallium3D-style commands into the VirGL wire format for submission
//! via VirtIO GPU SUBMIT_3D. Each command is a sequence of 32-bit DWORDs with
//! a header encoding `(length << 16 | subcmd << 8 | object_type)`.

// =============================================================================
// VirGL Command Types (VIRGL_CCMD_*)
// =============================================================================

#[allow(dead_code)]
mod ccmd {
    pub const NOP: u8 = 0;
    pub const CREATE_OBJECT: u8 = 1;
    pub const BIND_OBJECT: u8 = 2;
    pub const DESTROY_OBJECT: u8 = 3;
    pub const SET_VIEWPORT_STATE: u8 = 4;
    pub const SET_FRAMEBUFFER_STATE: u8 = 5;
    pub const SET_VERTEX_BUFFERS: u8 = 6;
    pub const CLEAR: u8 = 7;
    pub const DRAW_VBO: u8 = 8;
    pub const RESOURCE_INLINE_WRITE: u8 = 9;
    pub const SET_SAMPLER_VIEWS: u8 = 10;
    pub const SET_INDEX_BUFFER: u8 = 11;
    pub const SET_CONSTANT_BUFFER: u8 = 12;
    pub const SET_STENCIL_REF: u8 = 13;
    pub const SET_BLEND_COLOR: u8 = 14;
    pub const SET_SCISSOR_STATE: u8 = 15;
    pub const BLIT: u8 = 16;
    pub const RESOURCE_COPY_REGION: u8 = 17;
    pub const BIND_SAMPLER_STATES: u8 = 18;
    pub const SET_POLYGON_STIPPLE: u8 = 22;
    pub const SET_MIN_SAMPLES: u8 = 33;
    pub const SET_SUB_CTX: u8 = 28;
    pub const CREATE_SUB_CTX: u8 = 29;
    pub const DESTROY_SUB_CTX: u8 = 30;
    pub const BIND_SHADER: u8 = 31;
    pub const SET_TWEAKS: u8 = 46;
}

// =============================================================================
// VirGL Object Types (VIRGL_OBJECT_*)
// =============================================================================

#[allow(dead_code)]
pub mod obj {
    pub const NULL: u8 = 0;
    pub const BLEND: u8 = 1;
    pub const RASTERIZER: u8 = 2;
    pub const DSA: u8 = 3;
    pub const SHADER: u8 = 4;
    pub const VERTEX_ELEMENTS: u8 = 5;
    pub const SAMPLER_VIEW: u8 = 6;
    pub const SAMPLER_STATE: u8 = 7;
    pub const SURFACE: u8 = 8;
    pub const QUERY: u8 = 9;
    pub const STREAMOUT_TARGET: u8 = 10;
}

// Public re-exports for bind_object() callers in gpu_pci.rs
pub const OBJ_BLEND: u8 = obj::BLEND;
pub const OBJ_DSA: u8 = obj::DSA;
pub const OBJ_RASTERIZER: u8 = obj::RASTERIZER;
pub const OBJ_VERTEX_ELEMENTS: u8 = obj::VERTEX_ELEMENTS;

// =============================================================================
// VirGL Format Constants (matches Gallium PIPE_FORMAT_*)
// =============================================================================

#[allow(dead_code)]
pub mod format {
    pub const B8G8R8A8_UNORM: u32 = 1;
    pub const B8G8R8X8_UNORM: u32 = 2;
    pub const R8G8B8A8_UNORM: u32 = 67;
    pub const R8_UNORM: u32 = 64;
    pub const R32_FLOAT: u32 = 28;
    pub const R32G32_FLOAT: u32 = 29;
    pub const R32G32B32_FLOAT: u32 = 30;
    pub const R32G32B32A32_FLOAT: u32 = 31;
}

// =============================================================================
// Pipe Constants
// =============================================================================

#[allow(dead_code)]
pub mod pipe {
    // Texture targets
    pub const BUFFER: u32 = 0;
    pub const TEXTURE_2D: u32 = 2;

    // Bind flags
    pub const BIND_DEPTH_STENCIL: u32 = 1 << 0;
    pub const BIND_RENDER_TARGET: u32 = 1 << 1;
    pub const BIND_SAMPLER_VIEW: u32 = 1 << 3;
    pub const BIND_VERTEX_BUFFER: u32 = 1 << 4;
    pub const BIND_INDEX_BUFFER: u32 = 1 << 5;
    pub const BIND_CONSTANT_BUFFER: u32 = 1 << 6;
    pub const BIND_SCANOUT: u32 = 1 << 18;
    pub const BIND_SHARED: u32 = 1 << 20;

    // Clear buffer flags
    pub const CLEAR_DEPTH: u32 = 0x01;
    pub const CLEAR_STENCIL: u32 = 0x02;
    pub const CLEAR_COLOR0: u32 = 0x04;

    // Primitive types
    pub const PRIM_POINTS: u32 = 0;
    pub const PRIM_LINES: u32 = 1;
    pub const PRIM_TRIANGLES: u32 = 4;
    pub const PRIM_TRIANGLE_STRIP: u32 = 5;
    pub const PRIM_TRIANGLE_FAN: u32 = 6;

    // Shader types
    pub const SHADER_VERTEX: u32 = 0;
    pub const SHADER_FRAGMENT: u32 = 1;

    // Texture wrapping modes
    pub const TEX_WRAP_REPEAT: u32 = 0;
    pub const TEX_WRAP_CLAMP: u32 = 1;
    pub const TEX_WRAP_CLAMP_TO_EDGE: u32 = 2;
    pub const TEX_WRAP_CLAMP_TO_BORDER: u32 = 3;

    // Texture filtering modes
    pub const TEX_FILTER_NEAREST: u32 = 0;
    pub const TEX_FILTER_LINEAR: u32 = 1;

    // Mipmap filtering modes
    pub const TEX_MIPFILTER_NEAREST: u32 = 0;
    pub const TEX_MIPFILTER_LINEAR: u32 = 1;
    pub const TEX_MIPFILTER_NONE: u32 = 2;
}

// =============================================================================
// TGSI Texture Swizzle Constants
// =============================================================================

#[allow(dead_code)]
pub mod swizzle {
    pub const RED: u32 = 0;
    pub const GREEN: u32 = 1;
    pub const BLUE: u32 = 2;
    pub const ALPHA: u32 = 3;
    pub const ZERO: u32 = 4;
    pub const ONE: u32 = 5;

    /// Identity swizzle: RGBA → RGBA (packed as r[0:2] | g[3:5] | b[6:8] | a[9:11])
    pub const IDENTITY: u32 = RED | (GREEN << 3) | (BLUE << 6) | (ALPHA << 9);
}

// =============================================================================
// Command Buffer
// =============================================================================

/// Fixed-capacity VirGL command buffer. Accumulates u32 DWORDs for submission
/// via VIRTIO_GPU_CMD_SUBMIT_3D.
pub struct CommandBuffer {
    data: [u32; 3072], // 12KB — large enough for 12 circle draws with inline vertex data
    len: usize,
}

impl CommandBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0u32; 3072],
            len: 0,
        }
    }

    /// Reset the buffer for reuse.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Get the command data as a u32 slice.
    pub fn as_slice(&self) -> &[u32] {
        &self.data[..self.len]
    }

    /// Number of u32 DWORDs in the buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Size in bytes.
    pub fn byte_len(&self) -> usize {
        self.len * 4
    }

    /// Push a single DWORD.
    fn push(&mut self, val: u32) {
        if self.len < self.data.len() {
            self.data[self.len] = val;
            self.len += 1;
        }
    }

    /// Push a slice of DWORDs.
    fn push_slice(&mut self, vals: &[u32]) {
        for &v in vals {
            self.push(v);
        }
    }

    // =========================================================================
    // Command Encoders
    // =========================================================================

    /// Encode a VirGL command header.
    #[inline]
    fn cmd0(cmd: u8, obj: u8, len: u16) -> u32 {
        (cmd as u32) | ((obj as u32) << 8) | ((len as u32) << 16)
    }

    /// Create a sub-context.
    pub fn create_sub_ctx(&mut self, sub_ctx_id: u32) {
        self.push(Self::cmd0(ccmd::CREATE_SUB_CTX, 0, 1));
        self.push(sub_ctx_id);
    }

    /// Set the active sub-context.
    pub fn set_sub_ctx(&mut self, sub_ctx_id: u32) {
        self.push(Self::cmd0(ccmd::SET_SUB_CTX, 0, 1));
        self.push(sub_ctx_id);
    }

    /// Send a SET_TWEAKS command (Mesa compatibility hint to virglrenderer).
    /// tweak_id=1: gles_emulate_bgra — enables BGRA texture emulation on GLES hosts
    /// tweak_id=2: gles_apply_bgra_dest_swizzle — BGRA destination swizzle
    pub fn set_tweaks(&mut self, tweak_id: u32, value: u32) {
        self.push(Self::cmd0(ccmd::SET_TWEAKS, 0, 2));
        self.push(tweak_id);
        self.push(value);
    }

    /// Set minimum samples (Mesa sends this with value=1 for all draws).
    pub fn set_min_samples(&mut self, value: u32) {
        self.push(Self::cmd0(ccmd::SET_MIN_SAMPLES, 0, 1));
        self.push(value);
    }

    /// Create a surface object wrapping a resource.
    pub fn create_surface(&mut self, handle: u32, res_handle: u32, fmt: u32, level: u32, layers: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SURFACE, 5));
        self.push(handle);
        self.push(res_handle);
        self.push(fmt);
        self.push(level);
        self.push(layers); // first_layer | (last_layer << 16)
    }

    /// Create a blend state (no blending, write all color channels).
    pub fn create_blend_simple(&mut self, handle: u32) {
        // len=11: handle + S0 + S1 + S2[0..7]
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::BLEND, 11));
        self.push(handle);
        self.push(0x00000004); // S0: dither enabled (bit 2) — matches Mesa
        self.push(0); // S1: logicop_func = 0
        // S2[0]: colormask=0xF (write RGBA), blend disabled
        // VIRGL_OBJ_BLEND_S2_RT_COLORMASK(x) = ((x) & 0xf) << 27 in virgl_hw.h
        // Mesa sends 0x78000000 — NOT 0xF0000000 (that was a 1-bit shift error)
        self.push(0xF << 27);
        // S2[1..7]: unused render targets
        for _ in 0..7 {
            self.push(0);
        }
    }

    /// Create a depth-stencil-alpha state matching Mesa exactly.
    /// Mesa sends DSA with S0=0x00000000, length=5.
    pub fn create_dsa_default(&mut self, handle: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::DSA, 5));
        self.push(handle);
        self.push(0); // S0: all zeros — matches Mesa
        self.push(0); // S1: front stencil
        self.push(0); // S2: back stencil
        self.push(0); // alpha_ref
    }

    /// Create a rasterizer state matching Mesa exactly.
    /// Mesa sends rasterizer with S0=0x60000002 (depth_clip + bottom_edge_rule), length=9.
    pub fn create_rasterizer_default(&mut self, handle: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::RASTERIZER, 9));
        self.push(handle);
        self.push(0x60000002); // S0: depth_clip_near(bit1) + bottom_edge_rule(bit30) — matches Mesa
        self.push(0); // point_size = 0.0
        self.push(0); // sprite_coord_enable
        self.push(0); // S3
        self.push(0); // line_width
        self.push(0); // S5: offset_units
        self.push(0); // S6: offset_scale
        self.push(0); // S7: offset_clamp
    }

    /// Create a rasterizer state with custom S0 flags.
    pub fn create_rasterizer(&mut self, handle: u32, s0_flags: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::RASTERIZER, 9));
        self.push(handle);
        self.push(s0_flags);
        self.push(0);
        self.push(0);
        self.push(0);
        self.push(0);
        self.push(0);
        self.push(0);
        self.push(0);
    }

    /// Bind an object by type and handle.
    pub fn bind_object(&mut self, handle: u32, obj_type: u8) {
        self.push(Self::cmd0(ccmd::BIND_OBJECT, obj_type, 1));
        self.push(handle);
    }

    /// Create a shader from TGSI text.
    ///
    /// `num_tokens` must be nonzero (Mesa uses the actual TGSI token count).
    /// Parallels' VirGL silently rejects the entire batch if num_tokens=0.
    /// Use 300 as a safe default if the actual token count is unknown.
    pub fn create_shader(&mut self, handle: u32, shader_type: u32, num_tokens: u32, tgsi_text: &[u8]) {
        let text_len = tgsi_text.len() + 1; // include null terminator
        let text_dwords = (text_len + 3) / 4;
        // Header DWORDs: handle, type, offset, num_tokens, num_so_outputs = 5
        let payload_len = 5 + text_dwords;

        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SHADER, payload_len as u16));
        self.push(handle);
        self.push(shader_type);
        // OFFSET field: shader byte length with bit 31 CLEAR = first/only chunk.
        self.push(text_len as u32);
        self.push(num_tokens);
        self.push(0); // num_so_outputs = 0

        // Pack TGSI text bytes into DWORDs (little-endian)
        let mut i = 0;
        while i < text_dwords {
            let base = i * 4;
            let mut dword = 0u32;
            for b in 0..4 {
                if base + b < tgsi_text.len() {
                    dword |= (tgsi_text[base + b] as u32) << (b * 8);
                }
            }
            self.push(dword);
            i += 1;
        }
    }

    /// Bind a shader by handle and type.
    pub fn bind_shader(&mut self, handle: u32, shader_type: u32) {
        self.push(Self::cmd0(ccmd::BIND_SHADER, 0, 2));
        self.push(handle);
        self.push(shader_type);
    }

    /// Set framebuffer state (nr_cbufs color buffer surface handles, optional depth surface).
    pub fn set_framebuffer_state(&mut self, zsurf_handle: u32, cbuf_handles: &[u32]) {
        let nr_cbufs = cbuf_handles.len() as u32;
        self.push(Self::cmd0(ccmd::SET_FRAMEBUFFER_STATE, 0, (nr_cbufs + 2) as u16));
        self.push(nr_cbufs);
        self.push(zsurf_handle);
        for &h in cbuf_handles {
            self.push(h);
        }
    }

    /// Set viewport state for one viewport.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.push(Self::cmd0(ccmd::SET_VIEWPORT_STATE, 0, 7));
        self.push(0); // start_slot = 0
        self.push(f32_bits(width / 2.0));     // scale_x
        self.push(f32_bits(-height / 2.0));    // scale_y (negative for GL convention)
        self.push(f32_bits(0.5));              // scale_z
        self.push(f32_bits(width / 2.0));      // translate_x
        self.push(f32_bits(height / 2.0));     // translate_y
        self.push(f32_bits(0.5));              // translate_z
    }

    /// Set scissor state for one viewport slot.
    ///
    /// VirGL protocol packs each pair into one u32:
    ///   `[start_slot, (min_x | min_y<<16), (max_x | max_y<<16)]`, length=3.
    /// Restricts rendering to the rectangle [min_x, min_y) .. [max_x, max_y).
    /// Requires the scissor bit in the rasterizer state to be enabled.
    pub fn set_scissor_state(&mut self, min_x: u32, min_y: u32, max_x: u32, max_y: u32) {
        self.push(Self::cmd0(ccmd::SET_SCISSOR_STATE, 0, 3));
        self.push(0); // start_slot = 0
        self.push((min_x & 0xFFFF) | ((min_y & 0xFFFF) << 16));
        self.push((max_x & 0xFFFF) | ((max_y & 0xFFFF) << 16));
    }

    /// Set constant buffer for a shader stage.
    /// `shader_type`: 0=VERTEX, 1=FRAGMENT
    /// `index`: constant buffer index (usually 0)
    /// `data`: f32 values reinterpreted as u32 bits
    pub fn set_constant_buffer(&mut self, shader_type: u32, index: u32, data: &[u32]) {
        let len = 2 + data.len();
        self.push(Self::cmd0(ccmd::SET_CONSTANT_BUFFER, 0, len as u16));
        self.push(shader_type);
        self.push(index);
        self.push_slice(data);
    }

    /// Clear the framebuffer.
    /// Color values are f32 (0.0-1.0), reinterpreted as u32 bits.
    pub fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.push(Self::cmd0(ccmd::CLEAR, 0, 8));
        self.push(pipe::CLEAR_COLOR0); // buffers = clear color only
        self.push(f32_bits(r));
        self.push(f32_bits(g));
        self.push(f32_bits(b));
        self.push(f32_bits(a));
        // depth as f64 = 1.0 (0x3FF0000000000000) matching Mesa exactly
        self.push(0x00000000); // low 32 bits
        self.push(0x3FF00000); // high 32 bits = 1.0
        self.push(0); // stencil
    }

    /// Create vertex elements describing vertex layout.
    /// Each element: (src_offset, instance_divisor, vertex_buffer_index, src_format)
    pub fn create_vertex_elements(&mut self, handle: u32, elements: &[(u32, u32, u32, u32)]) {
        let len = 4 * elements.len() + 1;
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::VERTEX_ELEMENTS, len as u16));
        self.push(handle);
        for &(offset, divisor, vb_index, fmt) in elements {
            self.push(offset);
            self.push(divisor);
            self.push(vb_index);
            self.push(fmt);
        }
    }

    /// Set vertex buffers: (stride, offset, resource_handle) per buffer.
    /// VirGL protocol: payload is just [stride, offset, handle] * N. Host infers
    /// num_buffers from length / 3. No start_slot field — buffers bind from slot 0.
    pub fn set_vertex_buffers(&mut self, buffers: &[(u32, u32, u32)]) {
        let len = 3 * buffers.len();
        self.push(Self::cmd0(ccmd::SET_VERTEX_BUFFERS, 0, len as u16));
        for &(stride, offset, res_handle) in buffers {
            self.push(stride);
            self.push(offset);
            self.push(res_handle);
        }
    }

    /// Inline write data into a resource (upload vertex/index data).
    pub fn resource_inline_write(
        &mut self,
        res_handle: u32,
        x: u32, w: u32,
        data: &[u32],
    ) {
        let len = 11 + data.len();
        self.push(Self::cmd0(ccmd::RESOURCE_INLINE_WRITE, 0, len as u16));
        self.push(res_handle);
        self.push(0); // level
        self.push(0); // usage
        self.push(0); // stride (0 for buffers)
        self.push(0); // layer_stride
        self.push(x); // x offset in bytes
        self.push(0); // y
        self.push(0); // z
        self.push(w); // width in bytes
        self.push(1); // h
        self.push(1); // d
        self.push_slice(data);
    }

    /// Inline write a 2D region of pixel data into a texture resource.
    pub fn resource_inline_write_2d(
        &mut self,
        res_handle: u32,
        x: u32, y: u32, w: u32, h: u32,
        stride: u32,
        data: &[u32],
    ) {
        let len = 11 + data.len();
        self.push(Self::cmd0(ccmd::RESOURCE_INLINE_WRITE, 0, len as u16));
        self.push(res_handle);
        self.push(0);       // level
        self.push(0);       // usage
        self.push(stride);  // stride in bytes
        self.push(0);       // layer_stride
        self.push(x);       // x in pixels
        self.push(y);       // y in pixels
        self.push(0);       // z
        self.push(w);       // width in pixels
        self.push(h);       // height in pixels
        self.push(1);       // depth
        self.push_slice(data);
    }

    /// Draw primitives.
    pub fn draw_vbo(
        &mut self,
        start: u32,
        count: u32,
        mode: u32,
        max_index: u32,
    ) {
        self.push(Self::cmd0(ccmd::DRAW_VBO, 0, 12));
        self.push(start);           // offset 1: START
        self.push(count);           // offset 2: COUNT
        self.push(mode);            // offset 3: MODE
        self.push(0);               // offset 4: INDEXED = false
        self.push(1);               // offset 5: INSTANCE_COUNT
        self.push(0);               // offset 6: INDEX_BIAS
        self.push(0);               // offset 7: START_INSTANCE
        self.push(0);               // offset 8: PRIMITIVE_RESTART = disabled
        self.push(0);               // offset 9: RESTART_INDEX
        self.push(0);               // offset 10: MIN_INDEX
        self.push(max_index);       // offset 11: MAX_INDEX
        self.push(0);               // offset 12: COUNT_FROM_SO
    }

    // =========================================================================
    // Texture / Sampler Commands
    // =========================================================================

    /// Create a sampler view (binds a texture resource for shader sampling).
    ///
    /// VirGL protocol: CREATE_OBJECT(SAMPLER_VIEW) with 6 payload DWORDs:
    /// `[handle, res_handle, format, first_level, last_level, swizzle_packed]`
    ///
    /// `swizzle_packed` encodes channel mapping: `r | (g<<3) | (b<<6) | (a<<9)`
    /// using constants from `swizzle::*`. Use `swizzle::IDENTITY` for default.
    pub fn create_sampler_view(
        &mut self,
        handle: u32,
        res_handle: u32,
        format: u32,
        first_level: u32,
        last_level: u32,
        swizzle_packed: u32,
    ) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SAMPLER_VIEW, 6));
        self.push(handle);
        self.push(res_handle);
        self.push(format);
        self.push(first_level);
        self.push(last_level);
        self.push(swizzle_packed);
    }

    /// Create a sampler state (texture filtering and wrapping).
    ///
    /// VirGL protocol: CREATE_OBJECT(SAMPLER_STATE) with 9 payload DWORDs:
    /// `[handle, S0, lod_bias, min_lod, max_lod, border_r, border_g, border_b, border_a]`
    ///
    /// S0 bit packing: `wrap_s[0:2] | wrap_t[3:5] | wrap_r[6:8] |
    ///                   min_img[9:10] | min_mip[11:12] | mag_img[13:14]`
    pub fn create_sampler_state(
        &mut self,
        handle: u32,
        wrap_s: u32,
        wrap_t: u32,
        wrap_r: u32,
        min_img_filter: u32,
        min_mip_filter: u32,
        mag_img_filter: u32,
    ) {
        let s0 = (wrap_s & 7)
            | ((wrap_t & 7) << 3)
            | ((wrap_r & 7) << 6)
            | ((min_img_filter & 3) << 9)
            | ((min_mip_filter & 3) << 11)
            | ((mag_img_filter & 3) << 13);

        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SAMPLER_STATE, 9));
        self.push(handle);
        self.push(s0);
        self.push(0); // lod_bias = 0.0
        self.push(0); // min_lod = 0.0
        self.push(0); // max_lod = 0.0
        self.push(0); // border_color[0] = 0.0
        self.push(0); // border_color[1] = 0.0
        self.push(0); // border_color[2] = 0.0
        self.push(0); // border_color[3] = 0.0
    }

    /// Bind sampler views to a shader stage.
    ///
    /// VirGL protocol: SET_SAMPLER_VIEWS with DWORDs:
    /// `[shader_type, start_slot, view_handle0, view_handle1, ...]`
    pub fn set_sampler_views(&mut self, shader_type: u32, start_slot: u32, view_handles: &[u32]) {
        let len = 2 + view_handles.len();
        self.push(Self::cmd0(ccmd::SET_SAMPLER_VIEWS, 0, len as u16));
        self.push(shader_type);
        self.push(start_slot);
        for &h in view_handles {
            self.push(h);
        }
    }

    /// Bind sampler states to a shader stage.
    ///
    /// VirGL protocol: BIND_SAMPLER_STATES with DWORDs:
    /// `[shader_type, start_slot, state_handle0, state_handle1, ...]`
    pub fn bind_sampler_states(&mut self, shader_type: u32, start_slot: u32, state_handles: &[u32]) {
        let len = 2 + state_handles.len();
        self.push(Self::cmd0(ccmd::BIND_SAMPLER_STATES, 0, len as u16));
        self.push(shader_type);
        self.push(start_slot);
        for &h in state_handles {
            self.push(h);
        }
    }

    /// Blit (copy) a rectangle between two resources entirely on the host GPU.
    ///
    /// VirGL protocol: VIRGL_CCMD_BLIT with 21 DWORDs:
    /// S0 (mask/filter/flags), scissor min/max, dst resource/level/format/box, src resource/level/format/box
    ///
    /// This is the key operation for the two-resource display pipeline:
    /// VirGL renders to resource A (3D), then BLIT copies to resource B (2D scanout)
    /// entirely on the host — no guest DMA needed.
    pub fn blit(
        &mut self,
        src_res: u32, src_fmt: u32, src_x: u32, src_y: u32, src_w: u32, src_h: u32,
        dst_res: u32, dst_fmt: u32, dst_x: u32, dst_y: u32, dst_w: u32, dst_h: u32,
    ) {
        self.push(Self::cmd0(ccmd::BLIT, 0, 21));
        // S0: mask=0xF (RGBA), filter=0 (NEAREST), no scissor
        let mask: u32 = 0xF; // PIPE_MASK_RGBA
        let filter: u32 = 0; // PIPE_TEX_FILTER_NEAREST
        self.push(mask | (filter << 8));
        // Scissor min/max (unused, set to 0)
        self.push(0); // scissor minx/miny
        self.push(0); // scissor maxx/maxy
        // Destination
        self.push(dst_res);  // DST_RES_HANDLE
        self.push(0);        // DST_LEVEL
        self.push(dst_fmt);  // DST_FORMAT
        self.push(dst_x);    // DST_X
        self.push(dst_y);    // DST_Y
        self.push(0);        // DST_Z
        self.push(dst_w);    // DST_W
        self.push(dst_h);    // DST_H
        self.push(1);        // DST_D
        // Source
        self.push(src_res);  // SRC_RES_HANDLE
        self.push(0);        // SRC_LEVEL
        self.push(src_fmt);  // SRC_FORMAT
        self.push(src_x);    // SRC_X
        self.push(src_y);    // SRC_Y
        self.push(0);        // SRC_Z
        self.push(src_w);    // SRC_W
        self.push(src_h);    // SRC_H
        self.push(1);        // SRC_D
    }
}

/// Reinterpret f32 as u32 bits (IEEE 754).
#[inline]
fn f32_bits(f: f32) -> u32 {
    f.to_bits()
}
