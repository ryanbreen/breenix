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
    pub const SET_SUB_CTX: u8 = 28;
    pub const CREATE_SUB_CTX: u8 = 29;
    pub const DESTROY_SUB_CTX: u8 = 30;
    pub const BIND_SHADER: u8 = 31;
}

// =============================================================================
// VirGL Object Types (VIRGL_OBJECT_*)
// =============================================================================

#[allow(dead_code)]
mod obj {
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
        self.push(0); // S0: no special features
        self.push(0); // S1: logicop_func = 0
        // S2[0]: colormask=0xF (write RGBA), blend disabled
        self.push(0xF << 27);
        // S2[1..7]: unused render targets
        for _ in 0..7 {
            self.push(0);
        }
    }

    /// Create a depth-stencil-alpha state (all disabled).
    pub fn create_dsa_disabled(&mut self, handle: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::DSA, 5));
        self.push(handle);
        self.push(0); // S0: depth/alpha disabled
        self.push(0); // S1: front stencil disabled
        self.push(0); // S2: back stencil disabled
        self.push(0); // alpha_ref = 0.0
    }

    /// Create a basic rasterizer state (fill mode, depth clip, half-pixel center).
    pub fn create_rasterizer_default(&mut self, handle: u32) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::RASTERIZER, 9));
        self.push(handle);
        // S0: depth_clip(1<<1) | fill_front=FILL(0<<10) | fill_back=FILL(0<<12) | half_pixel_center(1<<29)
        // PIPE_POLYGON_MODE: FILL=0, LINE=1, POINT=2. Fill fields are 0 so omitted.
        self.push((1 << 1) | (1 << 29));
        self.push(0x3F800000u32); // point_size = 1.0f
        self.push(0); // sprite_coord_enable
        self.push(0); // S3
        self.push(0x3F800000u32); // line_width = 1.0f
        self.push(0); // offset_units
        self.push(0); // offset_scale
        self.push(0); // offset_clamp
    }

    /// Bind an object by type and handle.
    pub fn bind_object(&mut self, handle: u32, obj_type: u8) {
        self.push(Self::cmd0(ccmd::BIND_OBJECT, obj_type, 1));
        self.push(handle);
    }

    /// Create a shader from TGSI text.
    pub fn create_shader(&mut self, handle: u32, shader_type: u32, tgsi_text: &[u8]) {
        let text_len = tgsi_text.len() + 1; // include null terminator
        let text_dwords = (text_len + 3) / 4;
        // Header DWORDs: handle, type, offset, num_tokens, num_so_outputs = 5
        let payload_len = 5 + text_dwords;

        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SHADER, payload_len as u16));
        self.push(handle);
        self.push(shader_type);
        self.push(1 << 31); // offset = 0, bit 31 = 1 (last chunk — triggers compilation)
        self.push(text_len as u32); // byte length of TGSI text including null
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
                // else: zero (null terminator / padding)
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

    /// Clear the framebuffer.
    /// Color values are f32 (0.0-1.0), reinterpreted as u32 bits.
    pub fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.push(Self::cmd0(ccmd::CLEAR, 0, 8));
        self.push(pipe::CLEAR_COLOR0); // buffers = clear color only
        self.push(f32_bits(r));
        self.push(f32_bits(g));
        self.push(f32_bits(b));
        self.push(f32_bits(a));
        // depth as f64 split into two u32s (0.0)
        self.push(0);
        self.push(0);
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

    /// Draw primitives.
    pub fn draw_vbo(
        &mut self,
        start: u32,
        count: u32,
        mode: u32,
        max_index: u32,
    ) {
        self.push(Self::cmd0(ccmd::DRAW_VBO, 0, 12));
        self.push(start);
        self.push(count);
        self.push(mode);
        self.push(0); // indexed = false
        self.push(1); // instance_count
        self.push(0); // start_instance
        self.push(0); // index_bias
        self.push(0); // min_index
        self.push(max_index);
        self.push(0); // primitive_restart = disabled
        self.push(0); // restart_index
        self.push(0); // count_from_so
    }
}

/// Reinterpret f32 as u32 bits (IEEE 754).
#[inline]
fn f32_bits(f: f32) -> u32 {
    f.to_bits()
}
