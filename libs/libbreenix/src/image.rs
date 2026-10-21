//! The running executable's own image: its read-only loaded segments, found from the ELF
//! header the linker loads with the first segment (`__ehdr_start`). Bytes inside one are
//! code or read-only statics of the program rather than heap, stack, mapped or writable
//! memory.

const PT_LOAD: u32 = 1;
const PF_W: u32 = 2;

/// Whether `bytes` starts and ends inside one of the executable's read-only loaded segments.
#[cfg(target_os = "breenix")]
pub fn read_only(bytes: &[u8]) -> bool {
    extern "C" {
        static __ehdr_start: u8;
    }
    // SAFETY: the linker places the ELF header and program headers at the start of the
    // first loaded segment (userspace/programs/linker.ld), which stays mapped for the
    // program's life; `in_image` validates the header before reading program headers.
    unsafe { in_image(core::ptr::addr_of!(__ehdr_start), bytes.as_ptr() as u64, bytes.len() as u64) }
}

/// Off Breenix there is no program image to consult, so this answers false.
#[cfg(not(target_os = "breenix"))]
pub fn read_only(_bytes: &[u8]) -> bool {
    false
}

/// Whether `[start, start + len)` lies inside one non-writable `PT_LOAD` segment of the
/// 64-bit little-endian ELF image whose header is at `ehdr`.
///
/// # Safety
/// `ehdr` must address a readable ELF header followed, at `e_phoff`, by its readable
/// program headers.
pub(crate) unsafe fn in_image(ehdr: *const u8, start: u64, len: u64) -> bool {
    let read = |offset: usize, n: usize| {
        let mut v = 0u64;
        for i in (0..n).rev() {
            // SAFETY: the caller's contract makes the header and program headers readable.
            v = v << 8 | unsafe { *ehdr.add(offset + i) } as u64;
        }
        v
    };
    // ELF magic, 64-bit class, little-endian data.
    if read(0, 4) != 0x464c_457f || read(4, 1) != 2 || read(5, 1) != 1 {
        return false;
    }
    let Some(end) = start.checked_add(len) else { return false };
    let (phoff, phentsize, phnum) = (read(32, 8) as usize, read(54, 2) as usize, read(56, 2) as usize);
    (0..phnum).any(|i| {
        let ph = phoff + i * phentsize;
        let (kind, flags) = (read(ph, 4) as u32, read(ph + 4, 4) as u32);
        let (vaddr, memsz) = (read(ph + 16, 8), read(ph + 40, 8));
        kind == PT_LOAD && flags & PF_W == 0 && start >= vaddr && end <= vaddr.saturating_add(memsz)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ELF header with three program headers: read-execute, read-only and read-write.
    fn image() -> [u8; 64 + 3 * 56] {
        let mut h = [0u8; 64 + 3 * 56];
        h[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        h[32..40].copy_from_slice(&64u64.to_le_bytes());
        h[54..56].copy_from_slice(&56u16.to_le_bytes());
        h[56..58].copy_from_slice(&3u16.to_le_bytes());
        for (i, (flags, vaddr)) in [(5u32, 0x4000_0000u64), (4, 0x4001_0000), (6, 0x4002_0000)].into_iter().enumerate() {
            let ph = 64 + i * 56;
            h[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
            h[ph + 4..ph + 8].copy_from_slice(&flags.to_le_bytes());
            h[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
            h[ph + 40..ph + 48].copy_from_slice(&0x1000u64.to_le_bytes());
        }
        h
    }

    #[test]
    fn only_bytes_inside_one_read_only_segment_count() {
        let h = image();
        let inside = |start, len| unsafe { in_image(h.as_ptr(), start, len) };
        assert!(inside(0x4000_0010, 16), "read-execute segment");
        assert!(inside(0x4001_0000, 0x1000), "the whole read-only segment");
        assert!(!inside(0x4001_0ff0, 32), "runs past the read-only segment");
        assert!(!inside(0x4002_0000, 8), "writable segment");
        assert!(!inside(0x5000_0000, 8), "outside the image, as heap memory is");
        assert!(!inside(u64::MAX - 4, 8), "wrapping range");
        let mut bad = h;
        bad[0] = 0;
        assert!(!unsafe { in_image(bad.as_ptr(), 0x4000_0010, 16) }, "not an ELF header");
    }
}
