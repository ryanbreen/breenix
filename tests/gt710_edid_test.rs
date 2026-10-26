//! EDID 1.4 detailed-timing and block-integrity check, from the VESA format.
#[path = "../kernel/src/gpu/gk208/edid.rs"]
mod edid;

fn checksum(block: &mut [u8]) {
    block[127] = 0;
    block[127] = 0u8.wrapping_sub(block.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
}

#[test]
fn requires_complete_live_edid_blocks_and_exact_supported_timings() {
    let mut e = vec![0u8; 256];
    e[..8].copy_from_slice(&[0,255,255,255,255,255,255,0]);
    e[18] = 1; e[19] = 4; e[20] = 0x80; e[126] = 1;
    // 2560x1600 RB: 268.5 MHz, 2720x1646, sync 2608..2640 / 1603..1609.
    e[54..72].copy_from_slice(&[0xe2,0x68,0,0xa0,0xa0,0x40,0x2e,0x60,0x30,0x20,0x36,0,0,0,0,0,0,0x1a]);
    // 1280x800 at 60 Hz in the base standard-timing list (DMT 0x1c).
    e[38..40].copy_from_slice(&[0x81,0]);
    e[128] = 2; e[129] = 3; e[130] = 4;
    checksum(&mut e[..128]); checksum(&mut e[128..]);
    let modes = edid::parse(&e).unwrap();
    let low = modes.iter().find(|m| m.width == 1280 && m.height == 800).unwrap();
    assert_eq!((low.clock_khz, low.htotal, low.vtotal), (83500, 1680, 831));
    let high = modes.iter().find(|m| m.width == 2560 && m.height == 1600).unwrap();
    assert_eq!((high.clock_khz, high.hsync_start, high.hsync_end, high.htotal,
                high.vsync_start, high.vsync_end, high.vtotal), (268500,2608,2640,2720,1603,1609,1646));
    for n in 0..e.len() { assert!(edid::parse(&e[..n]).is_err()); }
    let mut bad = e.clone(); bad[160] ^= 1; assert!(edid::parse(&bad).is_err());
    let mut bad = e.clone(); bad[126] = 8; checksum(&mut bad[..128]); assert!(edid::parse(&bad).is_err());
    let mut bad = e.clone(); bad[57] = 0; checksum(&mut bad[..128]); assert!(edid::parse(&bad).is_err());
    let mut missing = e; missing[38..40].copy_from_slice(&[1,1]); checksum(&mut missing[..128]);
    assert!(!edid::parse(&missing).unwrap().iter().any(|m| m.width == 1280 && m.height == 800));
}
