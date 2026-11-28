/// ATA ACS-3 IDENTIFY geometry for the port's DMA EXT path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identify {
    pub sectors: u64,
    pub logical_bytes: usize,
    pub physical_bytes: u32,
    pub alignment_offset: Option<u16>,
    pub flush_ext: bool,
    serial: [u8; 20],
    model: [u8; 40],
}

impl Identify {
    pub(super) fn matches_after_reset(self, actual: Self) -> bool {
        self.serial.iter().any(|&byte| !matches!(byte, 0 | 0xff | b' ')) && self == actual
    }
}

pub(super) fn parse(data: &[u8]) -> Result<Identify, &'static str> {
    let data = data.get(..512).ok_or("AHCI: short IDENTIFY DEVICE data")?;
    let word = |n: usize| u16::from_le_bytes([data[n * 2], data[n * 2 + 1]]);
    let valid = |word| word & 0xc000u16 == 0x4000;
    let commands = word(83);
    if !valid(commands) || commands & (1 << 10) == 0 {
        return Err("AHCI: disk does not support LBA48");
    }
    let sectors = u64::from_le_bytes(data[200..208].try_into().unwrap());
    if sectors == 0 || sectors > (1u64 << 48) {
        return Err("AHCI: invalid LBA48 capacity");
    }
    let size = word(106);
    let logical_bytes = if valid(size) && size & (1 << 12) != 0 {
        let words = u32::from(word(117)) | (u32::from(word(118)) << 16);
        if words != 2048 {
            return Err("AHCI: unsupported long logical sector size");
        }
        4096
    } else {
        512
    };
    let physical_ratio = if valid(size) && size & (1 << 13) != 0 {
        1u32 << (size & 0xf)
    } else {
        1
    };
    let alignment = word(209);
    let alignment_offset = valid(alignment).then_some(alignment & 0x3fff);
    if alignment_offset.is_some_and(|offset| u32::from(offset) >= physical_ratio) {
        return Err("AHCI: invalid logical sector alignment");
    }
    Ok(Identify {
        sectors,
        logical_bytes,
        physical_bytes: logical_bytes as u32 * physical_ratio,
        alignment_offset,
        flush_ext: commands & (1 << 13) != 0,
        serial: data[20..40].try_into().unwrap(),
        model: data[54..94].try_into().unwrap(),
    })
}
