//! Read the negotiated handoff versions from an ELF64 object symbol.

use crate::HARDWARE_CONFIG_VERSION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffError {
    Missing,
    MalformedElf,
    SymbolKind,
    SymbolSize,
    SymbolSection,
    SymbolExtent,
    VersionRange(u32, u32),
}

impl core::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing => write!(f, "kernel has no BREENIX_HANDOFF_VERSIONS (v7-only or stripped); this loader writes v{HARDWARE_CONFIG_VERSION}"),
            Self::VersionRange(lo, hi) => write!(f, "kernel accepts HardwareConfig v{lo}..v{hi}; this loader writes v{HARDWARE_CONFIG_VERSION}"),
            error => write!(f, "cannot read kernel HardwareConfig versions: {error:?}"),
        }
    }
}

pub fn handoff_decision(elf: &[u8]) -> Result<(), HandoffError> {
    let version = find_object_symbol(elf, b"BREENIX_HANDOFF_VERSIONS")?;
    let low = u32_at(version, 0);
    let high = u32_at(version, 4);
    if !(low..=high).contains(&HARDWARE_CONFIG_VERSION) {
        return Err(HandoffError::VersionRange(low, high));
    }
    Ok(())
}

fn file_range(elf: &[u8], offset: u64, size: u64) -> Option<&[u8]> {
    let end = offset.checked_add(size)?;
    elf.get(usize::try_from(offset).ok()?..usize::try_from(end).ok()?)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn find_object_symbol<'a>(elf: &'a [u8], name: &[u8]) -> Result<&'a [u8], HandoffError> {
    use HandoffError::*;
    if elf.len() < 64 || &elf[..6] != b"\x7fELF\x02\x01" {
        return Err(MalformedElf);
    }
    let sh_size = u64::from(u16_at(elf, 58));
    let sh_count = u64::from(u16_at(elf, 60));
    if sh_count == 0 {
        return Err(Missing);
    }
    if sh_size < 64 {
        return Err(MalformedElf);
    }
    let sections = file_range(elf, u64_at(elf, 40), sh_size * sh_count).ok_or(MalformedElf)?;
    let section = |index: u64| file_range(sections, index * sh_size, sh_size);
    for symtab in sections
        .chunks_exact(sh_size as usize)
        .filter(|s| u32_at(s, 4) == 2)
    {
        let stride = u64_at(symtab, 56);
        if stride < 24 || stride > isize::MAX as u64 || u64_at(symtab, 32) % stride != 0 {
            return Err(MalformedElf);
        }
        let symbols =
            file_range(elf, u64_at(symtab, 24), u64_at(symtab, 32)).ok_or(MalformedElf)?;
        let strings = section(u64::from(u32_at(symtab, 40)))
            .filter(|s| u32_at(s, 4) == 3)
            .ok_or(MalformedElf)?;
        let strings =
            file_range(elf, u64_at(strings, 24), u64_at(strings, 32)).ok_or(MalformedElf)?;
        for symbol in symbols.chunks_exact(stride as usize) {
            let offset = u32_at(symbol, 0) as usize;
            let Some(symbol_name) = strings.get(offset..) else {
                return Err(MalformedElf);
            };
            if !symbol_name.starts_with(name) || symbol_name.get(name.len()) != Some(&0) {
                continue;
            }
            if symbol[4] & 0xf != 1 {
                return Err(SymbolKind);
            }
            if u64_at(symbol, 16) != 8 {
                return Err(SymbolSize);
            }
            let index = u64::from(u16_at(symbol, 6));
            if index == 0 || index >= 0xff00 {
                return Err(SymbolSection);
            }
            let data = section(index)
                .filter(|s| u32_at(s, 4) == 1)
                .ok_or(SymbolSection)?;
            let section_size = u64_at(data, 32);
            let address = u64_at(data, 16);
            address.checked_add(section_size).ok_or(SymbolExtent)?;
            let relative = u64_at(symbol, 8).checked_sub(address).ok_or(SymbolExtent)?;
            let file = file_range(elf, u64_at(data, 24), section_size).ok_or(SymbolExtent)?;
            return file_range(file, relative, 8).ok_or(SymbolExtent);
        }
    }
    Err(Missing)
}
