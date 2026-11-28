//! Identify Namespace, NVM Command Set 1.0 §4.1.5.
#[derive(Clone, Copy, Debug)]
pub(super) struct Namespace {
    pub sectors: u64,
    pub logical_bytes: usize,
}

pub(super) fn parse_namespace(data: &[u8; 4096]) -> Result<Namespace, &'static str> {
    let sectors = u64::from_le_bytes(data[..8].try_into().unwrap());
    let last_format = usize::from(data[25]);
    let flbas = usize::from(data[26]);
    let mut format = flbas & 0x0f;
    if last_format >= 16 {
        format |= (flbas & 0x60) >> 1;
    }
    if last_format >= 64 || format > last_format {
        return Err("invalid NVMe active LBA format");
    }
    let offset = 128 + 4 * format;
    let metadata = u16::from_le_bytes([data[offset], data[offset + 1]]);
    if metadata != 0 || data[29] & 7 != 0 {
        return Err("NVMe namespace metadata or protection information is unsupported");
    }
    let logical_bytes = match data[offset + 2] {
        9 => 512,
        12 => 4096,
        _ => return Err("unsupported NVMe logical block size"),
    };
    if sectors == 0 || sectors.checked_mul(logical_bytes as u64).is_none() {
        return Err("invalid NVMe namespace capacity");
    }
    Ok(Namespace { sectors, logical_bytes })
}
