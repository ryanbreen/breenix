//! Virtio 1.2 sections 2.5 and 5.2.4–6. Addressing uses 512-byte sectors.
use super::blk_queue::{Limits, SECTOR_BYTES};

pub const SIZE_MAX: u64 = 1 << 1;
pub const SEG_MAX: u64 = 1 << 2;
pub const BLK_SIZE: u64 = 1 << 6;
pub const TOPOLOGY: u64 = 1 << 10;
pub const VERSION_1: u64 = 1 << 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockConfig {
    pub capacity: u64,
    pub limits: Limits,
    pub block_hint: u32,
    pub physical_exp: u8,
    pub alignment_offset: u8,
    pub min_io_blocks: u16,
    pub opt_io_blocks: u32,
}

#[cfg(target_arch = "x86_64")]
pub fn required_config_bytes(features: u64) -> usize {
    if features & TOPOLOGY != 0 { 32 }
    else if features & BLK_SIZE != 0 { 24 }
    else if features & SEG_MAX != 0 { 16 }
    else if features & SIZE_MAX != 0 { 12 }
    else { 8 }
}

/// Sample the negotiated fields together. Legacy transports have no
/// generation counter, so require two matching samples.
pub fn read_config(
    features: u64,
    generation: impl Fn() -> Option<u32>,
    read8: impl Fn(usize) -> u8,
    read16: impl Fn(usize) -> u16,
    read32: impl Fn(usize) -> u32,
) -> Result<BlockConfig, &'static str> {
    let sample = || {
        let low = u64::from(read32(0));
        let high = u64::from(read32(4));
        let (physical_exp, alignment_offset, min_io_blocks, opt_io_blocks) =
            if features & TOPOLOGY != 0 { (read8(24), read8(25), read16(26), read32(28)) }
            else { (0, 0, 0, 0) };
        BlockConfig {
            capacity: (high << 32) | low,
            limits: Limits {
                size_max: if features & SIZE_MAX != 0 { read32(8) } else { 0 },
                seg_max: if features & SEG_MAX != 0 { read32(12) } else { 0 },
                exact_used_len: features & VERSION_1 != 0,
            },
            block_hint: if features & BLK_SIZE != 0 { read32(20) } else { SECTOR_BYTES as u32 },
            physical_exp,
            alignment_offset,
            min_io_blocks,
            opt_io_blocks,
        }
    };
    for _ in 0..100 {
        let before = generation();
        let config = sample();
        let stable = match before {
            Some(_) => generation() == before,
            None => sample() == config,
        };
        if stable {
            return config.validate(features);
        }
    }
    Err("Unstable VirtIO block configuration")
}

impl BlockConfig {
    fn validate(self, features: u64) -> Result<Self, &'static str> {
        if self.capacity == 0 || self.capacity.checked_mul(SECTOR_BYTES as u64).is_none() {
            return Err("Invalid VirtIO block capacity");
        }
        if self.block_hint < SECTOR_BYTES as u32 || !self.block_hint.is_power_of_two() {
            return Err("Invalid VirtIO preferred block size");
        }
        let ratio = 1u32.checked_shl(u32::from(self.physical_exp))
            .ok_or("Invalid VirtIO physical block exponent")?;
        if self.block_hint.checked_mul(ratio).is_none() || u32::from(self.alignment_offset) >= ratio {
            return Err("Invalid VirtIO block topology");
        }
        if features & SIZE_MAX != 0 && self.limits.size_max < SECTOR_BYTES as u32 {
            return Err("VirtIO maximum segment smaller than a sector");
        }
        if features & SEG_MAX != 0 && self.limits.seg_max == 0 {
            return Err("VirtIO block device allows no data segments");
        }
        Ok(self)
    }
}
