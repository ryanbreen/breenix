//! SVGA table sizing derived independently for the STDU objects (#1664).
//!
//! VMware's packed device definitions, distributed in Linux v6.12:
//! https://github.com/torvalds/linux/blob/v6.12/drivers/gpu/drm/vmwgfx/device_include/svga3d_cmd.h
//! SVGAOTableMobEntry: format u32 + size u32 + base PPN64 = 16 bytes.
//! SVGAOTableSurfaceEntry: five u32, Size(three u32), four u32,
//! two u8 + u16, f32, two padding u32 = 64 bytes.
//! SVGAOTableScreenTargetEntry: ImageId(three u32), six u32/i32,
//! seven padding u32 = 64 bytes. No host compiler layout is assumed.
//! ContextEntry (two u32) and ShaderEntry (four u32) need no storage: this
//! STDU path creates no GB context or shader IDs. A submission context is not
//! a GB context object and does not index SVGA_OTABLE_CONTEXT.
//!
//! Breenix policy: each required table covers [0, highest_live_id], including
//! unused ID zero. Backing is one zeroed page addressed with PT64_0; no backing
//! MOB is defined because SET_OTABLE_BASE64 directly takes a page number.
//! Production uses MOB/surface ID 1 and screen ID 0. Testing adds an offscreen
//! GPU-copy MOB/surface ID 2; its tables cover three entries rather than two.
//! Table bytes, allocated page bytes and object quotas are distinct quantities.
//! Page-depth definitions: SVGAMobFormat (PT64_0=4, PT64_2=6) in:
//! https://github.com/torvalds/linux/blob/v6.12/drivers/gpu/drm/vmwgfx/device_include/svga3d_types.h

pub const MOB_ENTRY_BYTES: usize = 4 + 4 + 8;
pub const SURFACE_ENTRY_BYTES: usize = 5 * 4 + 3 * 4 + 4 * 4 + 1 + 1 + 2 + 4 + 2 * 4;
pub const SCREEN_ENTRY_BYTES: usize = 3 * 4 + 6 * 4 + 7 * 4;
pub const PT64_0: u32 = 4;
pub const PT64_2: u32 = 6;

pub fn table_bytes(entry_bytes: usize, highest_id: u32) -> Result<u32, &'static str> {
    let bytes = (highest_id as usize)
        .checked_add(1)
        .and_then(|entries| entries.checked_mul(entry_bytes))
        .ok_or("SVGA object-table size overflow")?;
    if bytes == 0 || bytes > 4096 {
        return Err("SVGA object table exceeds one-page profile");
    }
    Ok(bytes as u32)
}

/// One 4KiB root with 512 PPN64 entries, each naming a 4KiB leaf of 512
/// PPN64 entries, describes at most 512*512 data pages. All unused entries
/// remain zero. Byte offsets within data pages are not part of a PPN.
pub fn leaf_count(data_pages: usize) -> Result<usize, &'static str> {
    if data_pages == 0 || data_pages > 512 * 512 {
        return Err("SVGA backing exceeds PT64_2 profile");
    }
    Ok(data_pages.div_ceil(512))
}
