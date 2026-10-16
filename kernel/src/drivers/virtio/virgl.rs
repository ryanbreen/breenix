//! VirGL Command Encoder
//!
//! Encodes Gallium3D-style commands into the VirGL wire format for submission
//! via VirtIO GPU SUBMIT_3D. Each command is a sequence of 32-bit DWORDs with
//! a header encoding `(length << 16 | object_type << 8 | command)`.
//!
//! State provenance (#1659): independently derived from the published wire
//! definitions at revision 07a5bc398dfffdc36ad20a3476348753aa10a322:
//! https://chromium.googlesource.com/chromiumos/third_party/virglrenderer/+/07a5bc398dfffdc36ad20a3476348753aa10a322/src/virgl_protocol.h
//! Anchors below name fields, not host implementation routines. Breenix chooses
//! a single colour attachment, deterministic UNORM writes, no depth/stencil/alpha
//! test, no culling, filled triangles and no multisampling. Rasterization uses
//! half-integer pixel centres (OpenGL 4.6 core, section 14.6.1) and the bottom-edge
//! convention selected explicitly by this profile (not implied by the viewport).
//! https://registry.khronos.org/OpenGL/specs/gl/glspec46.core.pdf

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

/// Breenix's shader roles: one colour attachment, clip-space vertices and
/// perspective-correct RGBA sampling. No stream output or all-CBUFs broadcast.
///
/// Provenance (#1660): independently authored assembly from TGSI's language
/// specification, sections "Declaration", "Semantics", "Interpolation",
/// "MOV", "TEX" and "END": https://docs.mesa3d.org/gallium/tgsi.html
/// Token accounting uses only the published token STRUCTURES at the same wire
/// revision as this module (p_shader_tokens.h, not a compiler implementation):
/// https://chromium.googlesource.com/chromiumos/third_party/virglrenderer/+/07a5bc398dfffdc36ad20a3476348753aa10a322/src/gallium/include/pipe/p_shader_tokens.h
/// tgsi_header + tgsi_processor = 2 words; a declaration = declaration+range
/// (2), plus semantic/interpolation/sampler-view words when specified. MOV is
/// instruction+dst+src (3); TEX adds texture+second src (5); END is 1.
/// The four roles replace all five old strings: the broadcast fragment variant
/// is retired because SET_FRAMEBUFFER_STATE binds exactly one colour target.
#[derive(Clone, Copy)]
pub enum ShaderProgram {
    Position,
    UniformColour,
    PositionUv,
    SampleRgba,
}

impl ShaderProgram {
    fn stage(self) -> u32 {
        match self {
            Self::Position | Self::PositionUv => pipe::SHADER_VERTEX,
            Self::UniformColour | Self::SampleRgba => pipe::SHADER_FRAGMENT,
        }
    }

    fn tokens(self) -> u32 {
        match self {
            // input, POSITION output, MOV, END.
            Self::Position => 2 + 2 + 3 + 3 + 1,
            // CONST, COLOR output, MOV, END; no properties.
            Self::UniformColour => 2 + 2 + 3 + 3 + 1,
            // input range, POSITION and GENERIC outputs, two MOVs, END.
            Self::PositionUv => 2 + 2 + 3 + 3 + 3 + 3 + 1,
            // sampler, view, GENERIC+PERSPECTIVE input, COLOR, TEX, END.
            Self::SampleRgba => 2 + 2 + 3 + 4 + 3 + 5 + 1,
        }
    }

    fn source(self) -> &'static [u8] {
        match self {
            // IN[0] is the caller's homogeneous clip position, unchanged.
            Self::Position => b"VERT\nDCL OUT[0], POSITION\nDCL IN[0]\nMOV OUT[0].xyzw, IN[0].xyzw\nEND\n",
            // Each fragment writes the per-draw constant, including alpha.
            Self::UniformColour => b"FRAG\nDCL CONST[0]\nDCL OUT[0], COLOR[0]\nMOV OUT[0].xyzw, CONST[0].xyzw\nEND\n",
            // IN[1] carries normalized UV; W is retained for interpolation.
            Self::PositionUv => b"VERT\nDCL IN[0..1]\nDCL OUT[1], GENERIC[0]\nDCL OUT[0], POSITION\nMOV OUT[1].xyzw, IN[1].xyzw\nMOV OUT[0].xyzw, IN[0].xyzw\nEND\n",
            // One 2D floating-point RGBA sample; sampler state supplies wrap
            // and filtering. No colour transform, discard or alpha override.
            Self::SampleRgba => b"FRAG\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\nDCL IN[0], GENERIC[0], PERSPECTIVE\nDCL OUT[0], COLOR[0]\nTEX OUT[0].xyzw, IN[0].xyzw, SAMP[0], 2D\nEND\n",
        }
    }
}

// =============================================================================
// Command Buffer
// =============================================================================

extern crate alloc;
use alloc::vec::Vec;

// 128 setup words + 60 * 52 rectangle words + teardown and documented tweaks.
// Keep storage off the kernel stack and reject an oversized stream as a whole.
const COMMAND_WORDS: usize = 4096;

/// Fixed-capacity VirGL command buffer. Accumulates u32 DWORDs for submission
/// via VIRTIO_GPU_CMD_SUBMIT_3D.
pub struct CommandBuffer {
    data: Vec<u32>,
    overflow: bool,
}

impl CommandBuffer {
    pub fn new() -> Self {
        Self {
            data: Vec::with_capacity(COMMAND_WORDS),
            overflow: false,
        }
    }

    /// Reset the buffer for reuse.
    pub fn clear(&mut self) {
        self.data.clear();
        self.overflow = false;
    }

    /// Get the command data as a u32 slice.
    pub fn as_slice(&self) -> &[u32] {
        &self.data
    }

    /// Never submit a prefix of an overflowing command stream.
    pub fn finish(&self) -> Result<&[u32], &'static str> {
        if self.overflow {
            Err("VirGL command buffer capacity exceeded")
        } else {
            Ok(&self.data)
        }
    }

    /// Number of u32 DWORDs in the buffer.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Size in bytes.
    pub fn byte_len(&self) -> usize {
        self.data.len() * 4
    }

    /// Push a single DWORD.
    fn push(&mut self, val: u32) {
        if self.data.len() < COMMAND_WORDS {
            self.data.push(val);
        } else {
            self.overflow = true;
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

    /// A batch owns subcontext 1 only until its draw commands have executed.
    /// CREATE/SET/DESTROY_SUB_CTX are published virgl_context_cmd commands.
    /// The published virgl_tweak_type IDs 0/1 select GLES BGRA emulation and
    /// destination swizzle. Enable both for our BGRA resources on GLES/ANGLE;
    /// desktop GL ignores GLES-only controls. ID 2 scales transform-feedback
    /// queries, which this profile never uses, so it is left at its default.
    pub fn begin_batch(&mut self) {
        self.create_sub_ctx(1);
        self.set_sub_ctx(1);
        for id in [0, 1] {
            self.push(Self::cmd0(ccmd::SET_TWEAKS, 0, 2));
            self.push(id);
            self.push(1);
        }
    }

    pub fn end_batch(&mut self) {
        self.set_sub_ctx(0);
        self.push(Self::cmd0(ccmd::DESTROY_SUB_CTX, 0, 1));
        self.push(1);
    }

    pub fn nop(&mut self) {
        self.push(Self::cmd0(ccmd::NOP, 0, 0));
    }

    /// Create a surface object wrapping a resource.
    pub fn create_surface(
        &mut self,
        handle: u32,
        res_handle: u32,
        fmt: u32,
        level: u32,
        layers: u32,
    ) {
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SURFACE, 5));
        self.push(handle);
        self.push(res_handle);
        self.push(fmt);
        self.push(level);
        self.push(layers); // first_layer | (last_layer << 16)
    }

    /// Create a blend state (no blending, write all color channels).
    pub fn create_blend_simple(&mut self, handle: u32) {
        // VIRGL_OBJ_BLEND_SIZE / S0 / S1 / S2: eight target slots.
        // S0=0 disables independent blending, logic ops, dithering and alpha
        // coverage/one. S1=0 is inactive because logic ops are disabled.
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::BLEND, 11));
        self.push(handle);
        self.push(0);
        self.push(0);
        for target in 0..8 {
            // S2: blending/functions/factors inactive (zero); only target 0
            // writes RGBA (four channel bits at bit 27). Others cannot write.
            self.push(if target == 0 { 0b1111 << 27 } else { 0 });
        }
    }

    /// Create a blend state with SRC_ALPHA / INV_SRC_ALPHA alpha blending.
    ///
    /// Used for rendering quads with per-pixel transparency (e.g., cursor texture
    /// where alpha=0 means transparent and alpha=0xFF means opaque).
    pub fn create_blend_alpha(&mut self, handle: u32) {
        // S2[0] encoding (per virgl_hw.h):
        //   bit 0: blend_enable = 1
        //   bits 1-3: rgb_func = PIPE_BLEND_ADD (0)
        //   bits 4-8: rgb_src_factor = PIPE_BLENDFACTOR_SRC_ALPHA (0x03)
        //   bits 9-13: rgb_dst_factor = PIPE_BLENDFACTOR_INV_SRC_ALPHA (0x13)
        //   bits 14-16: alpha_func = PIPE_BLEND_ADD (0)
        //   bits 17-21: alpha_src_factor = PIPE_BLENDFACTOR_ONE (0x01)
        //   bits 22-26: alpha_dst_factor = PIPE_BLENDFACTOR_ZERO (0x11)
        //   bits 27-30: colormask = 0xF (write RGBA)
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::BLEND, 11));
        self.push(handle);
        self.push(0); // S0: deterministic UNORM writes; no dithering
        self.push(0); // S1: logicop_func = 0
        self.push(0x7C42_2631); // S2[0]: alpha blend enabled
        for _ in 0..7 {
            self.push(0);
        }
    }

    /// Colour-only profile: no depth, stencil or alpha rejection.
    pub fn create_dsa_default(&mut self, handle: u32) {
        // VIRGL_OBJ_DSA_SIZE: handle, S0, front/back stencil, alpha reference.
        // All enable/write bits are zero. Inactive compare/op/mask fields and
        // alpha reference are canonical zero, including IEEE +0.0 for alpha.
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::DSA, 5));
        self.push_slice(&[handle, 0, 0, 0, 0.0f32.to_bits()]);
    }

    /// Filled, unculled triangles sampled at half-integer pixel centres.
    pub fn create_rasterizer_default(&mut self, handle: u32) {
        // VIRGL_OBJ_RS_S0: depth_clip=1 (bit 1), half_pixel_center=1
        // (bit 29), bottom_edge_rule=1 (bit 30). Every other S0 field is 0:
        // smooth interpolation, full Z clip, no discard/cull/scissor/clamp,
        // solid front/back fill (enum 0), no offsets/smoothing/stipple/MSAA.
        self.create_rasterizer(handle, (1 << 1) | (1 << 29) | (1 << 30));
    }

    /// Encode caller-selected S0 with the same defined scalar state profile.
    pub fn create_rasterizer(&mut self, handle: u32, s0_flags: u32) {
        // VIRGL_OBJ_RS_POINT_SIZE and LINE_WIDTH are IEEE f32 +1.0: even
        // though current draws are triangles, keep valid unit primitive sizes.
        // SPRITE_COORD_ENABLE=0; S3 stipple pattern/factor and clip-plane mask=0.
        // OFFSET_UNITS/SCALE/CLAMP=IEEE +0.0 (polygon offset disabled).
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::RASTERIZER, 9));
        self.push_slice(&[
            handle,
            s0_flags,
            1.0f32.to_bits(),
            0,
            0,
            1.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
        ]);
    }

    /// Bind an object by type and handle.
    pub fn bind_object(&mut self, handle: u32, obj_type: u8) {
        self.push(Self::cmd0(ccmd::BIND_OBJECT, obj_type, 1));
        self.push(handle);
    }

    /// Create a shader with exact metadata derived alongside its source.
    pub fn create_profile_shader(&mut self, handle: u32, program: ShaderProgram) {
        self.create_shader(handle, program.stage(), program.tokens(), program.source());
    }

    // VIRGL_OBJ_SHADER_{HDR_SIZE,OFFSET,NUM_TOKENS}: one NUL-terminated text
    // chunk, no stream output, exact compiled token count including its header.
    fn create_shader(&mut self, handle: u32, shader_type: u32, num_tokens: u32, tgsi_text: &[u8]) {
        let text_len = tgsi_text.len() + 1; // include null terminator
        let text_dwords = (text_len + 3) / 4;
        // Header DWORDs: handle, type, offset, num_tokens, num_so_outputs = 5
        let payload_len = 5 + text_dwords;

        self.push(Self::cmd0(
            ccmd::CREATE_OBJECT,
            obj::SHADER,
            payload_len as u16,
        ));
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
        self.push(Self::cmd0(
            ccmd::SET_FRAMEBUFFER_STATE,
            0,
            (nr_cbufs + 2) as u16,
        ));
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
        self.push(f32_bits(width / 2.0)); // scale_x
        self.push(f32_bits(-height / 2.0)); // scale_y (negative for GL convention)
        self.push(f32_bits(0.5)); // scale_z
        self.push(f32_bits(width / 2.0)); // translate_x
        self.push(f32_bits(height / 2.0)); // translate_y
        self.push(f32_bits(0.5)); // translate_z
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
        // VIRGL_OBJ_CLEAR_DEPTH_0/1 and STENCIL are present in the wire
        // packet but inactive: CLEAR_COLOR0 excludes depth and stencil. Use
        // canonical IEEE f64 +0.0, low word first, and stencil integer zero.
        let unused_depth = 0.0f64.to_bits();
        self.push(unused_depth as u32);
        self.push((unused_depth >> 32) as u32);
        self.push(0);
    }

    /// Create vertex elements describing vertex layout.
    /// Each element: (src_offset, instance_divisor, vertex_buffer_index, src_format)
    pub fn create_vertex_elements(&mut self, handle: u32, elements: &[(u32, u32, u32, u32)]) {
        let len = 4 * elements.len() + 1;
        self.push(Self::cmd0(
            ccmd::CREATE_OBJECT,
            obj::VERTEX_ELEMENTS,
            len as u16,
        ));
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
    pub fn resource_inline_write(&mut self, res_handle: u32, x: u32, w: u32, data: &[u32]) {
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
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        stride: u32,
        data: &[u32],
    ) {
        let len = 11 + data.len();
        self.push(Self::cmd0(ccmd::RESOURCE_INLINE_WRITE, 0, len as u16));
        self.push(res_handle);
        self.push(0); // level
        self.push(0); // usage
        self.push(stride); // stride in bytes
        self.push(0); // layer_stride
        self.push(x); // x in pixels
        self.push(y); // y in pixels
        self.push(0); // z
        self.push(w); // width in pixels
        self.push(h); // height in pixels
        self.push(1); // depth
        self.push_slice(data);
    }

    /// Draw primitives.
    pub fn draw_vbo(&mut self, start: u32, count: u32, mode: u32, max_index: u32) {
        self.push(Self::cmd0(ccmd::DRAW_VBO, 0, 12));
        self.push(start); // offset 1: START
        self.push(count); // offset 2: COUNT
        self.push(mode); // offset 3: MODE
        self.push(0); // offset 4: INDEXED = false
        self.push(1); // offset 5: INSTANCE_COUNT
        self.push(0); // offset 6: INDEX_BIAS
        self.push(0); // offset 7: START_INSTANCE
        self.push(0); // offset 8: PRIMITIVE_RESTART = disabled
        self.push(0); // offset 9: RESTART_INDEX
        self.push(0); // offset 10: MIN_INDEX
        self.push(max_index); // offset 11: MAX_INDEX
        self.push(0); // offset 12: COUNT_FROM_SO
    }

    // =========================================================================
    // Texture / Sampler Commands
    // =========================================================================

    /// Create a sampler view (binds a texture resource for shader sampling).
    ///
    /// VirGL protocol: CREATE_OBJECT(SAMPLER_VIEW) with 6 payload DWORDs:
    /// `[handle, res_handle, fmt_target, val0(layers), val1(levels), swizzle_packed]`
    ///
    /// `fmt_target`: bits [0:23] = pipe format, bits [24:31] = pipe texture target.
    /// `val0`: `first_layer | (last_layer << 16)` (for TEXTURE_2D, both 0).
    /// `val1`: `first_level | (last_level << 8)` (for single-mip, both 0).
    /// `swizzle_packed` encodes channel mapping: `r | (g<<3) | (b<<6) | (a<<9)`
    /// using constants from `swizzle::*`. Use `swizzle::IDENTITY` for default.
    pub fn create_sampler_view(
        &mut self,
        handle: u32,
        res_handle: u32,
        format: u32,
        target: u32,
        first_layer: u32,
        last_layer: u32,
        first_level: u32,
        last_level: u32,
        swizzle_packed: u32,
    ) {
        let fmt_target = (format & 0x00FF_FFFF) | ((target & 0xFF) << 24);
        let val0 = (first_layer & 0xFFFF) | ((last_layer & 0xFFFF) << 16);
        let val1 = (first_level & 0xFF) | ((last_level & 0xFF) << 8);
        self.push(Self::cmd0(ccmd::CREATE_OBJECT, obj::SAMPLER_VIEW, 6));
        self.push(handle);
        self.push(res_handle);
        self.push(fmt_target);
        self.push(val0);
        self.push(val1);
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
    pub fn bind_sampler_states(
        &mut self,
        shader_type: u32,
        start_slot: u32,
        state_handles: &[u32],
    ) {
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
        src_res: u32,
        src_fmt: u32,
        src_x: u32,
        src_y: u32,
        src_w: u32,
        src_h: u32,
        dst_res: u32,
        dst_fmt: u32,
        dst_x: u32,
        dst_y: u32,
        dst_w: u32,
        dst_h: u32,
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
        self.push(dst_res); // DST_RES_HANDLE
        self.push(0); // DST_LEVEL
        self.push(dst_fmt); // DST_FORMAT
        self.push(dst_x); // DST_X
        self.push(dst_y); // DST_Y
        self.push(0); // DST_Z
        self.push(dst_w); // DST_W
        self.push(dst_h); // DST_H
        self.push(1); // DST_D
                      // Source
        self.push(src_res); // SRC_RES_HANDLE
        self.push(0); // SRC_LEVEL
        self.push(src_fmt); // SRC_FORMAT
        self.push(src_x); // SRC_X
        self.push(src_y); // SRC_Y
        self.push(0); // SRC_Z
        self.push(src_w); // SRC_W
        self.push(src_h); // SRC_H
        self.push(1); // SRC_D
    }
}

/// Colour pipeline with handles unique across all object types in subcontext 1.
pub fn colour_batch(width: u32, height: u32, resource: u32) -> CommandBuffer {
    use self::format as fmt;
    let mut batch = CommandBuffer::new();
    batch.begin_batch();
    batch.create_surface(1, resource, fmt::B8G8R8X8_UNORM, 0, 0);
    batch.set_framebuffer_state(0, &[1]);
    batch.create_blend_simple(2);
    batch.create_dsa_default(3);
    batch.create_rasterizer_default(4);
    batch.create_profile_shader(5, ShaderProgram::Position);
    batch.create_profile_shader(6, ShaderProgram::UniformColour);
    batch.create_vertex_elements(7, &[(0, 0, 0, fmt::R32G32B32A32_FLOAT)]);
    batch.bind_object(2, OBJ_BLEND);
    batch.bind_object(3, OBJ_DSA);
    batch.bind_object(4, OBJ_RASTERIZER);
    batch.bind_object(7, OBJ_VERTEX_ELEMENTS);
    batch.bind_shader(5, pipe::SHADER_VERTEX);
    batch.bind_shader(6, pipe::SHADER_FRAGMENT);
    batch.set_viewport(width as f32, height as f32);
    batch
}

/// Reinterpret f32 as u32 bits (IEEE 754).
#[inline]
fn f32_bits(f: f32) -> u32 {
    f.to_bits()
}

#[cfg(test)]
mod tests {
    use super::CommandBuffer;

    #[test]
    fn shader_metadata_covers_each_declared_program_exactly() {
        use super::ShaderProgram::*;
        for (program, stage, count) in [
            (Position, 0, 11),
            (UniformColour, 1, 11),
            (PositionUv, 0, 17),
            (SampleRgba, 1, 20),
        ] {
            let mut stream = CommandBuffer::new();
            stream.create_profile_shader(23, program);
            let words = stream.as_slice();
            assert_eq!(
                &words[1..6],
                &[23, stage, program.source().len() as u32 + 1, count, 0]
            );
            let packed: Vec<u8> = words[6..]
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect();
            assert_eq!(&packed[..program.source().len()], program.source());
            assert!(packed[program.source().len()..]
                .iter()
                .all(|byte| *byte == 0));
            assert_eq!((words[0] >> 16) as usize, words.len() - 1);
        }
    }

    #[test]
    fn colour_profile_disables_dither_and_unused_attachments() {
        let mut stream = CommandBuffer::new();
        stream.create_blend_simple(7);
        assert_eq!(
            stream.as_slice(),
            &[0x000b0101, 7, 0, 0, 0x78000000, 0, 0, 0, 0, 0, 0, 0]
        );
        stream.clear();
        stream.create_dsa_default(9);
        assert_eq!(stream.as_slice(), &[0x00050301, 9, 0, 0, 0, 0]);
    }

    #[test]
    fn rasterization_has_defined_unit_sizes_and_pixel_rules() {
        let mut stream = CommandBuffer::new();
        stream.create_rasterizer_default(3);
        assert_eq!(
            stream.as_slice(),
            &[0x00090201, 3, 0x60000002, 0x3f800000, 0, 0, 0x3f800000, 0, 0, 0]
        );
    }

    #[test]
    fn colour_clear_does_not_request_depth_or_stencil() {
        let mut stream = CommandBuffer::new();
        stream.clear_color(1.0, 0.5, 0.0, 1.0);
        assert_eq!(
            stream.as_slice(),
            &[0x00080007, 4, 0x3f800000, 0x3f000000, 0, 0x3f800000, 0, 0, 0]
        );
    }
}
