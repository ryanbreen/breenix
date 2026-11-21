/// Decode the volatile write-cache support and enabled bits from IDENTIFY DEVICE.
pub(super) fn write_cache(data: &[u8]) -> (bool, bool) {
    // ATA ACS-3: words 82 and 85, bit 5 in each little-endian word.
    const SUPPORTED: usize = 82 * 2;
    const ENABLED: usize = 85 * 2;
    const WRITE_CACHE: u16 = 1 << 5;
    let supported = u16::from_le_bytes([data[SUPPORTED], data[SUPPORTED + 1]]);
    let enabled = u16::from_le_bytes([data[ENABLED], data[ENABLED + 1]]);
    (supported & WRITE_CACHE != 0, enabled & WRITE_CACHE != 0)
}
