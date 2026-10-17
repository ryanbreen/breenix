//! Wire numbers and layouts: virtio-gpu (VirtIO 1.3 CSD01: the transport and section 5.7)
//! and VirGL (the protocol header `virgl_protocol.h`, the capability header `virgl_hw.h`,
//! and the Gallium enumerants `p_defines.h` carries on the wire). These are interface
//! facts only; each value's source and line is in `docs/gui/provenance/gpu-core.md`.

/// Device feature bits (section 5.7.3) and the transport's VERSION_1 (section 6).
pub const F_VIRGL: u64 = 1 << 0;
pub const F_VERSION_1: u64 = 1 << 32;

/// DEVICE_NEEDS_RESET in the device status (section 2.1).
pub const STATUS_NEEDS_RESET: u8 = 64;

/// Device configuration offsets (section 5.7.4).
pub const CFG_NUM_SCANOUTS: usize = 8;
pub const CFG_NUM_CAPSETS: usize = 12;

/// Control queue commands (section 5.7.6.7).
pub mod cmd {
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;
    pub const CTX_CREATE: u32 = 0x0200;
    pub const CTX_DESTROY: u32 = 0x0201;
    pub const CTX_ATTACH_RESOURCE: u32 = 0x0202;
    pub const RESOURCE_CREATE_3D: u32 = 0x0204;
    pub const TRANSFER_FROM_HOST_3D: u32 = 0x0206;
    pub const SUBMIT_3D: u32 = 0x0207;
}

/// Responses (section 5.7.6.7).
pub mod resp {
    pub const OK_NODATA: u32 = 0x1100;
    pub const OK_DISPLAY_INFO: u32 = 0x1101;
    pub const OK_CAPSET_INFO: u32 = 0x1102;
    pub const OK_CAPSET: u32 = 0x1103;
    /// The first error response; every error is at or above it.
    pub const ERR_UNSPEC: u32 = 0x1200;
}

/// `virtio_gpu_ctrl_hdr.flags` (section 5.7.6.7).
pub const FLAG_FENCE: u32 = 1 << 0;
/// `sizeof(virtio_gpu_ctrl_hdr)`.
pub const HDR: usize = 24;
/// Header field offsets.
pub const HDR_TYPE: usize = 0;
pub const HDR_FLAGS: usize = 4;
pub const HDR_FENCE: usize = 8;
pub const HDR_CTX: usize = 16;

/// Capability set ids (section 5.7.6.8, GET_CAPSET_INFO).
pub const CAPSET_VIRGL: u32 = 1;
pub const CAPSET_VIRGL2: u32 = 2;

/// `virtio_gpu_resp_display_info` (section 5.7.6.8): 16 entries of `{ rect, enabled,
/// flags }` after the header.
pub const MAX_SCANOUTS: usize = 16;
pub const DISPLAY_ONE: usize = 24;

/// VirGL capability set layout (`virgl_hw.h`): byte offsets into `virgl_caps_v2`.
pub mod caps {
    /// `virgl_caps_v1` ends here; a set this short has no v2 fields.
    pub const V1_SIZE: usize = 308;
    pub const SAMPLER_MASK: usize = 4;
    pub const RENDER_MASK: usize = 68;
    pub const GLSL_LEVEL: usize = 264;
    pub const CAPABILITY_BITS: usize = 392;
    pub const CAPABILITY_BITS_V2: usize = 688;
    pub const MAX_TEXTURE_2D_SIZE: usize = 484;
    pub const RENDERER: usize = 696;
    pub const RENDERER_LEN: usize = 64;
}

/// VirGL formats (`enum virgl_formats`).
pub mod format {
    pub const B8G8R8A8_UNORM: u32 = 1;
    pub const R8_UNORM: u32 = 64;
    pub const R8G8B8A8_UNORM: u32 = 67;
    pub const R32G32_FLOAT: u32 = 29;
}

/// VirGL bind flags (`VIRGL_BIND_*`).
pub mod bind {
    pub const RENDER_TARGET: u32 = 1 << 1;
    pub const SAMPLER_VIEW: u32 = 1 << 3;
    pub const VERTEX_BUFFER: u32 = 1 << 4;
}

/// Gallium texture targets as the wire carries them (`enum pipe_texture_target`).
pub mod target {
    pub const BUFFER: u32 = 0;
    pub const TEXTURE_2D: u32 = 2;
}

/// VirGL context commands (`enum virgl_context_cmd`).
pub mod ccmd {
    pub const CREATE_OBJECT: u32 = 1;
    pub const BIND_OBJECT: u32 = 2;
    pub const DESTROY_OBJECT: u32 = 3;
    pub const SET_VIEWPORT_STATE: u32 = 4;
    pub const SET_FRAMEBUFFER_STATE: u32 = 5;
    pub const SET_VERTEX_BUFFERS: u32 = 6;
    pub const CLEAR: u32 = 7;
    pub const DRAW_VBO: u32 = 8;
    pub const RESOURCE_INLINE_WRITE: u32 = 9;
    pub const SET_CONSTANT_BUFFER: u32 = 12;
    pub const BIND_SHADER: u32 = 31;
}

/// VirGL object types (`enum virgl_object_type`).
pub mod obj {
    pub const BLEND: u32 = 1;
    pub const RASTERIZER: u32 = 2;
    pub const DSA: u32 = 3;
    pub const SHADER: u32 = 4;
    pub const VERTEX_ELEMENTS: u32 = 5;
    pub const SURFACE: u32 = 8;
}

/// `VIRGL_CMD0(cmd, obj, len)`: an 8-bit command, an 8-bit object type and a 16-bit
/// payload length in dwords.
pub const fn cmd0(cmd: u32, obj: u32, len: u32) -> u32 {
    cmd | (obj << 8) | (len << 16)
}

/// The longest payload one command header can carry.
pub const CMD0_MAX_LEN: u32 = 0xFFFF;

/// Gallium enumerants VirGL commands carry (`p_defines.h`).
pub mod pipe {
    pub const SHADER_VERTEX: u32 = 0;
    pub const SHADER_FRAGMENT: u32 = 1;
    pub const PRIM_TRIANGLES: u32 = 4;
    pub const CLEAR_COLOR0: u32 = 1 << 2;
    pub const MASK_RGBA: u32 = 0xF;
}

/// Rasterizer state word 0 (`VIRGL_OBJ_RS_S0_*`).
pub const RS_S0_HALF_PIXEL_CENTER: u32 = 1 << 29;
/// Blend state per-target word (`VIRGL_OBJ_BLEND_S2_RT_COLORMASK`).
pub const fn blend_s2_colormask(mask: u32) -> u32 {
    (mask & 0xF) << 27
}
