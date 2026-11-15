//! Selection against the captured and virtual display mode lists.
#![allow(dead_code)] // Include production hardware modules in the host test.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path = "../kernel/src/gpu/gk208/modeset.rs"] mod modeset;

// 20261113T180035Z-metal GT710_NATIVE, SHA-256 07da84b6557390f56c0b274dd93cc9a6e09f14684bc97b87e9978cdb0d86ff2c.
const R240_EDID: [u8; 128] = [
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x05, 0x05, 0x44, 0x48, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x16, 0x01, 0x03, 0x80, 0xa0, 0x5a, 0x78, 0x0a, 0xee, 0x95, 0xa3, 0x54, 0x4c, 0x99, 0x26,
    0x0f, 0x50, 0x54, 0xa7, 0xcf, 0x00, 0x71, 0x40, 0x71, 0x4a, 0x71, 0x4f, 0x81, 0xc0, 0x81, 0xcf,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x30, 0x2a, 0x00, 0x98, 0x51, 0x00, 0x2a, 0x40, 0x30, 0x70,
    0x13, 0x00, 0xda, 0x29, 0x11, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0xfe, 0x00, 0x41, 0x48, 0x45,
    0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfc, 0x00, 0x4c,
    0x43, 0x44, 0x31, 0x37, 0x2f, 0x31, 0x39, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfd,
    0x00, 0x31, 0x4c, 0x19, 0x51, 0x10, 0x00, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x47,
];

fn checksum(e: &mut [u8; 128]) {
    e[127] = 0;
    e[127] = 0u8.wrapping_sub(e.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
}

fn dense_panel() -> [u8; 128] {
    let mut e = [0u8; 128];
    e[..8].copy_from_slice(&[0,255,255,255,255,255,255,0]);
    e[18..21].copy_from_slice(&[1,4,0x80]);
    e[38..40].copy_from_slice(&[0x81,0]);
    e[54..72].copy_from_slice(&[0xe2,0x68,0,0xa0,0xa0,0x40,0x2e,0x60,0x30,0x20,0x36,0,0x1e,0xb3,0x10,0,0,0x1a]);
    checksum(&mut e);
    e
}

#[path = "../userspace/programs/src/display_select.rs"] mod display_select;
use display_select::{select, Refusal};

fn check_modes(modes: &[(u32, u32, u32)]) {
    assert_eq!(select(modes, None, None), None);
    let default = modes[1];
    let default_mode = (default.0 / default.2, default.1 / default.2);
    for (index, &(width, height, scale)) in modes.iter().enumerate() {
        let mode = (width / scale, height / scale);
        assert_eq!(select(modes, Some(mode), Some(scale)), Some(Ok(index)));
        let preferred = if modes.iter().any(|&(w,h,s)| s == default.2 && (w/s,h/s) == mode) { default.2 } else { 1 };
        let expected = modes.iter().position(|&(w,h,s)| s == preferred && (w/s,h/s) == mode).unwrap();
        assert_eq!(select(modes, Some(mode), None), Some(Ok(expected)));
    }
    for scale in [1, 2] {
        let expected = modes.iter().position(|&(w,h,s)| s == scale && (w/s,h/s) == default_mode);
        assert_eq!(select(modes, None, Some(scale)), Some(expected.ok_or(Refusal::Unsupported)));
    }
    assert_eq!(select(modes, Some((800,600)), Some(1)), Some(Err(Refusal::Unsupported)));
}

#[test]
fn selects_recorded_and_virtual_modes() {
    let r240 = modeset::display_modes(&edid::parse(&R240_EDID).unwrap()).unwrap();
    let dense = modeset::display_modes(&edid::parse(&dense_panel()).unwrap()).unwrap();
    assert_eq!(r240, [(1024,768,1),(1280,1024,1),(2560,2048,2)]);
    assert_eq!(dense, [(1280,800,1),(2560,1600,1),(5120,3200,2)]);
    check_modes(&r240);
    check_modes(&dense);
    check_modes(&[(1280,800,1),(2560,1600,2)]);
    assert_eq!(select(&r240, Some((1024,768)), Some(2)), Some(Err(Refusal::Unsupported)));
    assert_eq!(select(&dense, Some((1280,800)), None), Some(Ok(0)));
    assert_eq!(select(&dense, Some((2560,1600)), None), Some(Ok(1)));
    assert_eq!(select(&[], Some((800,600)), None), Some(Err(Refusal::Unsupported)));
}
