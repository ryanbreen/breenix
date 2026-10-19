//! VMware interface facts; pinned header locations are in the GPU core provenance record.

pub const VENDOR: u16 = 0x15ad;
pub const DEVICE: u16 = 0x0406;
pub const ID_3: u32 = 0x9000_0003;
pub const INVALID: u32 = u32::MAX;

pub mod reg {
    pub const ID: u32 = 0;
    pub const ENABLE: u32 = 1;
    pub const WIDTH: u32 = 2;
    pub const HEIGHT: u32 = 3;
    pub const CAPABILITIES: u32 = 17;
    pub const CONFIG_DONE: u32 = 20;
    pub const IRQMASK: u32 = 33;
    pub const COMMAND_LOW: u32 = 48;
    pub const COMMAND_HIGH: u32 = 49;
    pub const DEV_CAP: u32 = 52;
    pub const SCREEN_MAX_WIDTH: u32 = 55;
    pub const SCREEN_MAX_HEIGHT: u32 = 56;
    pub const MOB_MAX_SIZE: u32 = 57;
    pub const IRQ_STATUS: u32 = 82;
}

pub const COMMAND_BUFFERS: u32 = 0x0100_0000;
pub const GB_OBJECTS: u32 = 0x0800_0000;
pub const CB_HEADER: usize = 64;
pub const CB_DEVICE: u32 = 0x3f;
pub const CB_DX_CONTEXT: u32 = 2;
pub const CB_NO_IRQ: u32 = 1;
pub const QUEUE_CAPACITY: usize = 32;
/// The command-buffer size `svga_reg.h` names as supported (`SVGA_CB_MAX_SIZE_DEFAULT`).
pub const CB_MAX_BYTES: usize = 512 * 1024;
/// The single-command size `svga_reg.h` names as supported (`SVGA_CB_MAX_COMMAND_SIZE`).
pub const COMMAND_MAX_BYTES: usize = 32 * 1024;
pub const PT64_1: u32 = 5;
pub const PT64_2: u32 = 6;

pub mod status {
    pub const PENDING: u32 = 0;
    pub const COMPLETED: u32 = 1;
    pub const QUEUE_FULL: u32 = 2;
    pub const COMMAND_ERROR: u32 = 3;
    pub const HEADER_ERROR: u32 = 4;
    pub const PREEMPTED: u32 = 5;
    pub const SUBMISSION_ERROR: u32 = 6;
    pub const PARTIAL_COMPLETE: u32 = 7;
}

/// Device-context commands (`SVGADeviceContextCmdId`).
pub mod dc {
    pub const START_STOP_CONTEXT: u32 = 1;
    pub const PREEMPT: u32 = 2;
}

pub mod cap {
    pub const MAX_TEXTURE_WIDTH: u32 = 19;
    /// DX format entries of the vertex streams: R32G32B32A32_UINT and R32G32_FLOAT.
    pub const UINT4: u32 = 146;
    pub const FLOAT2: u32 = 224;
    /// `SVGA3D_DXFMT_DX_VERTEX_BUFFER`.
    pub const VERTEX_BUFFER: u32 = 1 << 8;
    pub const MAX_TEXTURE_HEIGHT: u32 = 20;
    pub const MAX_CONTEXTS: u32 = 77;
    pub const MAX_SURFACES: u32 = 78;
    pub const DX_CONTEXT: u32 = 95;
    pub const RGBA: u32 = 167;
    pub const R8: u32 = 192;
    pub const BGRA: u32 = 240;
    pub const SUPPORTED: u32 = 1;
    pub const SAMPLE: u32 = 2;
    pub const RENDER: u32 = 4;
}

pub mod format {
    pub const BUFFER: u32 = 37;
    pub const R32G32B32A32_UINT: u32 = 47;
    pub const RGBA: u32 = 68;
    pub const R8: u32 = 93;
    pub const R32G32_FLOAT: u32 = 125;
    pub const BGRA: u32 = 141;
}

pub mod bind {
    pub const SCREEN: u32 = 1 << 16;
    pub const VERTEX: u32 = 1 << 20;
    pub const SAMPLE: u32 = 1 << 23;
    pub const RENDER: u32 = 1 << 24;
}

pub mod cmd {
    pub const SURFACE_COPY: u32 = 1042;
    pub const SET_OTABLE_BASE64: u32 = 1115;
    pub const DEFINE_MOB64: u32 = 1135;
    pub const DESTROY_MOB: u32 = 1094;
    pub const DEFINE_SURFACE: u32 = 1097;
    pub const DESTROY_SURFACE: u32 = 1098;
    pub const BIND_SURFACE: u32 = 1099;
    pub const UPDATE_SURFACE: u32 = 1102;
    pub const READBACK_SURFACE: u32 = 1104;
    pub const DEFINE_SCREEN: u32 = 1124;
    pub const DESTROY_SCREEN: u32 = 1125;
    pub const BIND_SCREEN: u32 = 1126;
    pub const UPDATE_SCREEN: u32 = 1127;
    pub const GB_SCREEN_DMA: u32 = 1131;
    pub const NOP_ERROR: u32 = 1137;
    pub const DX_DEFINE_CONTEXT: u32 = 1143;
    pub const DX_DESTROY_CONTEXT: u32 = 1144;
    pub const DX_BIND_CONTEXT: u32 = 1145;
    pub const DX_SET_SHADER_RESOURCES: u32 = 1149;
    pub const DX_SET_SHADER: u32 = 1150;
    pub const DX_DRAW: u32 = 1152;
    pub const DX_SET_INPUT_LAYOUT: u32 = 1157;
    pub const DX_SET_VERTEX_BUFFERS: u32 = 1158;
    pub const DX_SET_TOPOLOGY: u32 = 1160;
    pub const DX_SET_RENDERTARGETS: u32 = 1161;
    pub const DX_SET_BLEND_STATE: u32 = 1162;
    pub const DX_SET_DEPTHSTENCIL_STATE: u32 = 1163;
    pub const DX_SET_RASTERIZER_STATE: u32 = 1164;
    pub const DX_SET_VIEWPORTS: u32 = 1174;
    pub const DX_CLEAR_RENDERTARGET_VIEW: u32 = 1176;
    pub const DX_PRED_COPY_REGION: u32 = 1178;
    pub const DX_DEFINE_SHADERRESOURCE_VIEW: u32 = 1185;
    pub const DX_DESTROY_SHADERRESOURCE_VIEW: u32 = 1186;
    pub const DX_DEFINE_RENDERTARGET_VIEW: u32 = 1187;
    pub const DX_DESTROY_RENDERTARGET_VIEW: u32 = 1188;
    pub const DX_DEFINE_ELEMENTLAYOUT: u32 = 1191;
    pub const DX_DEFINE_BLEND_STATE: u32 = 1193;
    pub const DX_DEFINE_DEPTHSTENCIL_STATE: u32 = 1195;
    pub const DX_DEFINE_RASTERIZER_STATE: u32 = 1197;
    pub const DX_DEFINE_SHADER: u32 = 1201;
    pub const DX_BIND_SHADER: u32 = 1203;
    pub const SET_COTABLE: u32 = 1207;
    pub const MOB_FENCE64: u32 = 1216;
}

/// Context object tables (`SVGACOTableType`).
pub mod cotable {
    pub const RTVIEW: u32 = 0;
    pub const SRVIEW: u32 = 2;
    pub const ELEMENTLAYOUT: u32 = 3;
    pub const BLENDSTATE: u32 = 4;
    pub const DEPTHSTENCIL: u32 = 5;
    pub const RASTERIZERSTATE: u32 = 6;
    pub const DXSHADER: u32 = 10;
}

/// `SVGA3dResourceType`, `SVGA3dPrimitiveType` and the fixed-state enumerations
/// (`svga3d_types.h`, `svga3d_dx.h`).
pub mod value {
    pub const RESOURCE_TEXTURE2D: u32 = 3;
    pub const TRIANGLELIST: u32 = 1;
    pub const INPUT_PER_VERTEX: u32 = 0;
    pub const INPUT_PER_INSTANCE: u32 = 1;
    pub const FILL_SOLID: u32 = 3;
    pub const CULL_NONE: u32 = 1;
    pub const BLEND_ZERO: u32 = 1;
    pub const BLEND_ONE: u32 = 2;
    pub const BLENDEQ_ADD: u32 = 1;
    pub const WRITE_ALL: u32 = 0xf;
    pub const DEPTH_WRITE_ALL: u32 = 1;
    pub const COMPARISON_ALWAYS: u32 = 8;
    pub const STENCIL_KEEP: u32 = 1;
    pub const SCREEN_PRIMARY: u32 = 1;
}

/// Packed entry widths, derived from the published structs (not table allocation sizes).
pub mod entry {
    pub const MOB: usize = 16;
    pub const SURFACE: usize = 64;
    pub const CONTEXT: usize = 8;
    pub const SHADER: usize = 16;
    pub const SCREEN: usize = 64;
    pub const DX_CONTEXT: usize = 8;
    pub const RTVIEW: usize = 32;
    pub const SRVIEW: usize = 32;
    pub const ELEMENTLAYOUT: usize = 1024;
    pub const BLENDSTATE: usize = 128;
    pub const DEPTHSTENCIL: usize = 16;
    pub const RASTERIZERSTATE: usize = 32;
    pub const DXSHADER: usize = 32;
}
