//! SVGA command-buffer wire facts, independently encoded for Breenix (#1663).
//!
//! Published VMware device definitions, version distributed in Linux v6.12:
//! https://github.com/torvalds/linux/blob/v6.12/drivers/gpu/drm/vmwgfx/device_include/svga_reg.h
//! Anchors: SVGACBHeader, SVGACBContext, SVGA_CB_CONTEXT_MASK,
//! SVGA_REG_COMMAND_LOW/HIGH, SVGACBFlags, SVGACBStatus and SVGADCCmdStartStop.
//! These are the device interface, not a driver implementation or its policy.
//!
//! A header is 64 bytes (4+4+8+4+4+8+4+4+6*4), aligned to 64 bytes so the
//! low six address bits can select a submission context. Register addresses
//! and the header's PA are byte addresses; MOB base PPNs are a different ABI.
//! Breenix uses physical command storage (NO_IRQ only, no MOB or DX flags),
//! context 0 for SVGA3D packets and device context 63 only for device commands.

#[repr(C, align(64))]
pub struct Header {
    pub status: u32,
    pub error_offset: u32,
    pub id: u64,
    pub flags: u32,
    pub length: u32,
    pub pa: u64,
    pub offset: u32,
    pub dx_context: u32,
    pub must_be_zero: [u32; 6],
}

pub fn address_words(address: u64, context: u32) -> Result<(u32, u32), &'static str> {
    if address & 63 != 0 || (context != 0 && context != 63) {
        return Err("SVGA command-header address/context is invalid");
    }
    Ok(((address >> 32) as u32, address as u32 | context))
}

impl Header {
    pub fn new(commands: u64, length: usize) -> Result<Self, &'static str> {
        // Stay within the published default command-buffer maximum (512 KiB),
        // with whole DWORD packets. No optional larger-buffer capability.
        if length == 0 || length % 4 != 0 || length > 512 * 1024 {
            return Err("SVGA command-buffer length is invalid");
        }
        Ok(Self {
            status: 0,
            error_offset: 0,
            id: 0,
            flags: 1,
            length: length as u32,
            pa: commands,
            offset: 0,
            dx_context: 0,
            must_be_zero: [0; 6],
        })
    }
}
