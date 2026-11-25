//! The program header that makes a zero-padded copy of an ELF64 executable
//! load its padding, so an exec of the copy reads the padding as it reads the
//! program (exec reads only PT_LOAD file bytes). Layouts are the ELF gABI's.
//! Free of std and libbreenix so a host test can check it.

const PHDR: usize = 56;
const PAGE: u64 = 4096;
const PT_LOAD: u32 = 1;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PF_R: u32 = 4;

/// For `image`, an ELF64 little-endian executable, followed by zeros up to
/// `file_len` bytes, the file offset of its PT_GNU_STACK program header and
/// the PT_LOAD header to write there: read-only, from the first page boundary
/// after the image to the end of the file, mapped one page above the end of the
/// image's highest segment. The Breenix loaders ignore PT_GNU_STACK.
pub fn padding_load(image: &[u8], file_len: u64) -> Result<(u64, [u8; PHDR]), &'static str> {
    if image.len() < 64 || image[..7] != *b"\x7fELF\x02\x01\x01" {
        return Err("not an ELF64 little-endian file");
    }
    let u16_at = |at: usize| u16::from_le_bytes([image[at], image[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
    let (phoff, phentsize, phnum) = (u64_at(32) as usize, u16_at(54), u16_at(56));
    if phentsize != PHDR || phoff.checked_add(phnum * PHDR).is_none_or(|end| end > image.len()) {
        return Err("program header table out of range");
    }
    let mut segments_end = 0;
    let mut stack = None;
    for index in 0..phnum {
        let at = phoff + index * PHDR;
        match u32_at(at) {
            PT_LOAD => segments_end = segments_end.max(u64_at(at + 16) + u64_at(at + 40)),
            PT_GNU_STACK => stack = Some(at),
            _ => {}
        }
    }
    let stack = stack.ok_or("no PT_GNU_STACK header to replace")?;
    let start = (image.len() as u64).div_ceil(PAGE) * PAGE;
    if file_len <= start {
        return Err("no padding after the image");
    }
    let address = segments_end.div_ceil(PAGE) * PAGE + PAGE;
    let size = file_len - start;
    let mut header = [0; PHDR];
    header[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
    header[4..8].copy_from_slice(&PF_R.to_le_bytes());
    header[8..16].copy_from_slice(&start.to_le_bytes());
    header[16..24].copy_from_slice(&address.to_le_bytes());
    header[24..32].copy_from_slice(&address.to_le_bytes());
    header[32..40].copy_from_slice(&size.to_le_bytes());
    header[40..48].copy_from_slice(&size.to_le_bytes());
    header[48..56].copy_from_slice(&PAGE.to_le_bytes());
    Ok((stack as u64, header))
}
