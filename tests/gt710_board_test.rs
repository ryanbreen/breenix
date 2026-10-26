//! Synthetic malformed-ROM checks for the bounded board parser.
#[path = "../kernel/src/gpu/gk208/board.rs"]
mod board;

fn checksum(bytes: &mut [u8]) {
    let end = bytes.len() - 1;
    bytes[end] = 0;
    bytes[end] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
}

fn rom() -> Vec<u8> {
    let mut r = vec![0; 4096];
    r[..3].copy_from_slice(&[0x55, 0xaa, 8]);
    r[0x18..0x1a].copy_from_slice(&0x40u16.to_le_bytes());
    r[0x40..0x44].copy_from_slice(b"PCIR");
    r[0x44..0x48].copy_from_slice(&[0xde, 0x10, 0x8b, 0x12]);
    r[0x50..0x52].copy_from_slice(&8u16.to_le_bytes());
    r[0x55] = 0x80;
    r[0x80..0x8c].copy_from_slice(&[0xff, 0xb8, b'B', b'I', b'T', 0, 0, 1, 12, 6, 1, 0]);
    r[0x8c..0x92].copy_from_slice(&[b'C', 2, 4, 0, 0xa0, 0]);
    r[0xa0..0xa4].copy_from_slice(&0xb0u32.to_le_bytes());
    r[0xb0..0xb4].copy_from_slice(&[0x40, 4, 12, 1]);
    r[0x36..0x38].copy_from_slice(&0x100u16.to_le_bytes());
    r[0x100..0x10a].copy_from_slice(&[0x40, 0x16, 1, 8, 0x80, 1, 0xcb, 0xbd, 0xdc, 0x4e]);
    r[0x114..0x116].copy_from_slice(&0x1c0u16.to_le_bytes());
    // TMDS on I2C entry 0, head 0, connector 0 and SOR 0.
    r[0x116..0x11a].copy_from_slice(&0x01000102u32.to_le_bytes());
    r[0x11a..0x11e].copy_from_slice(&0x30u32.to_le_bytes());
    r[0x180..0x188].copy_from_slice(&[0x41, 4, 1, 4, 0xe2, 3, 0, 0]);
    r[0x1c0..0x1c8].copy_from_slice(&[0x40, 4, 1, 4, 0x31, 0, 0, 0]);
    checksum(&mut r[0x80..0x8c]);
    checksum(&mut r);
    r
}

#[test]
fn rejects_bad_rom_tables_without_reading_outside_the_image() {
    let r = rom();
    let b = board::parse(&r).unwrap();
    assert_eq!(b.bit_version, 0x100);
    assert_eq!((b.routes[0].entry, b.routes[0].conf, b.routes[0].i2c_entry, b.routes[0].sor, b.routes[0].heads),
               (0x01000102, 0x30, 0x3e2, 0, 1));
    assert_eq!((b.dcb_version, b.i2c_version, b.connector_version, b.pll_version), (0x40, 0x41, 0x40, 0x40));
    assert_eq!((b.routes[0].connector, b.routes[0].kind, b.routes[0].drive), (0, 0x31, 2));
    for n in 0..r.len() { assert!(board::parse(&r[..n]).is_err(), "truncation {n}"); }
    for (offset, value) in [(0x36, 0xff), (0x101, 2), (0x102, 255), (0x103, 0),
                            (0x181, 0), (0x182, 255), (0x183, 0), (0x1c3, 0),
                            (0xa1, 0xff), (0xb1, 0), (0x8e, 255)] {
        let mut bad = r.clone(); bad[offset] = value; checksum(&mut bad);
        assert!(board::parse(&bad).is_err(), "offset {offset:x}");
    }
    let mut bad = r; bad[0x44] = 0; checksum(&mut bad);
    assert!(board::parse(&bad).is_err());
}
