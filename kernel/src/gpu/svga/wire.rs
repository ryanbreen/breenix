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
    pub const IRQ_STATUS: u32 = 82;
}

pub const COMMAND_BUFFERS: u32 = 0x0100_0000;
pub const GB_OBJECTS: u32 = 0x0800_0000;
pub const CB_HEADER: usize = 64;
pub const CB_DEVICE: u32 = 0x3f;
pub const CB_DX_CONTEXT: u32 = 2;
pub const CB_NO_IRQ: u32 = 1;
pub const QUEUE_CAPACITY: usize = 32;
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

pub mod cap {
    pub const MAX_TEXTURE_WIDTH: u32 = 19;
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
    pub const RGBA: u32 = 68;
    pub const R8: u32 = 93;
    pub const BGRA: u32 = 141;
}

pub mod bind {
    pub const SCREEN: u32 = 1 << 16;
    pub const VERTEX: u32 = 1 << 20;
    pub const CONSTANT: u32 = 1 << 22;
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
    pub const DEFINE_CONTEXT: u32 = 1143;
    pub const DESTROY_CONTEXT: u32 = 1144;
    pub const BIND_CONTEXT: u32 = 1145;
    pub const DRAW: u32 = 1152;
    pub const SET_COTABLE: u32 = 1207;
    pub const MOB_FENCE64: u32 = 1216;
}

/// Packed entry widths, derived from the published structs (not table allocation sizes).
pub mod entry {
    pub const MOB: usize = 16;
    pub const SURFACE: usize = 64;
    pub const CONTEXT: usize = 8;
    pub const SHADER: usize = 16;
    pub const SCREEN: usize = 64;
    pub const DX_CONTEXT: usize = 8;
}
