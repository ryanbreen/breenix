use arm64_boot_contract::handoff_elf::{handoff_decision, HandoffError};

fn elf(kind: u8, section_kind: u32, object_offset: u64, range: [u32; 2]) -> Vec<u8> {
    const STRINGS: &[u8] = b"\0BREENIX_HANDOFF_VERSIONS\0";
    fn put(image: &mut [u8], offset: usize, bytes: &[u8]) {
        image[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    let mut image = vec![0; 416];
    put(&mut image, 0, b"\x7fELF\x02\x01");
    put(&mut image, 40, &64u64.to_le_bytes());
    put(&mut image, 58, &64u16.to_le_bytes());
    put(&mut image, 60, &4u16.to_le_bytes());
    // .symtab, .strtab and the object section.
    for (index, ty, file, size) in [
        (1, 2, 320, 24),
        (2, 3, 352, STRINGS.len() as u64),
        (3, section_kind, 400, 8),
    ] {
        let sh = (index + 1) * 64;
        put(&mut image, sh + 4, &ty.to_le_bytes());
        put(&mut image, sh + 24, &u64::to_le_bytes(file));
        put(&mut image, sh + 32, &size.to_le_bytes());
    }
    put(&mut image, 128 + 40, &2u32.to_le_bytes());
    put(&mut image, 128 + 56, &24u64.to_le_bytes());
    put(&mut image, 256 + 16, &0x8000u64.to_le_bytes());
    put(&mut image, 320, &1u32.to_le_bytes());
    image[324] = kind;
    put(&mut image, 326, &3u16.to_le_bytes());
    put(&mut image, 328, &(0x8000 + object_offset).to_le_bytes());
    put(&mut image, 336, &8u64.to_le_bytes());
    put(&mut image, 352, STRINGS);
    put(&mut image, 400, &range[0].to_le_bytes());
    put(&mut image, 404, &range[1].to_le_bytes());
    image
}

#[test]
fn checks_object_type_file_bounds_and_negotiated_version() {
    assert_eq!(handoff_decision(&elf(1, 1, 0, [7, 8])), Ok(()));
    assert_eq!(
        handoff_decision(&elf(1, 1, 0, [7, 7])),
        Err(HandoffError::VersionRange(7, 7))
    );
    assert_eq!(
        handoff_decision(&elf(2, 1, 0, [7, 8])),
        Err(HandoffError::SymbolKind)
    );
    assert_eq!(
        handoff_decision(&elf(1, 8, 0, [7, 8])),
        Err(HandoffError::SymbolSection)
    );
    assert_eq!(
        handoff_decision(&elf(1, 1, 1, [7, 8])),
        Err(HandoffError::SymbolExtent)
    );
    let mut stripped = elf(1, 1, 0, [7, 8]);
    stripped[60] = 0;
    assert_eq!(handoff_decision(&stripped), Err(HandoffError::Missing));
}
