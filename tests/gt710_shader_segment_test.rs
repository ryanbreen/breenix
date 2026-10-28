#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

#[test]
fn segment_contains_both_programs_at_kepler_offsets() {
    let segment = shaders::triangle_segment();
    assert_eq!(shaders::VERTEX_OFFSET, 0x30);
    assert_eq!(shaders::FRAGMENT_OFFSET, 0x1b0);
    assert_eq!(segment.len(), 0x280);
    assert_eq!((shaders::VERTEX_OFFSET + 80) % 128, 0);
    assert_eq!((shaders::FRAGMENT_OFFSET + 80) % 128, 0);
    let mut expected = [0u8; 0x280];
    for (offset, header, code) in [
        (0x30, &include_bytes!("../kernel/assets/kepler/diagnostic-triangle-vertex.sph")[..], &include_bytes!("../kernel/assets/kepler/diagnostic-triangle-vertex.bin")[..]),
        (0x1b0, &include_bytes!("../kernel/assets/kepler/diagnostic-triangle-fragment.sph")[..], &include_bytes!("../kernel/assets/kepler/diagnostic-triangle-fragment.bin")[..]),
    ] {
        expected[offset..offset + header.len()].copy_from_slice(header);
        expected[offset + header.len()..offset + header.len() + code.len()].copy_from_slice(code);
    }
    let digest = sha256::sha256(&expected);
    assert_eq!(digest, [0xc5,0x13,0x38,0x00,0x02,0xcf,0x61,0xa3,0x52,0x11,0xc2,0x2e,0x99,0x3b,0xfd,0x8a,0x7b,0xea,0x6c,0x2d,0x83,0x1c,0xee,0x65,0x81,0xbe,0xc4,0x27,0xf0,0x7b,0xd4,0xc0]);
    assert_eq!(sha256::sha256(&segment), digest);
    assert_eq!(segment, expected);
}

#[test]
fn controls_fit_after_the_unchanged_triangle_segment() {
    let controls = [
        (shaders::CONSTANT_COLOUR_FRAGMENT_OFFSET, &shaders::CONSTANT_COLOUR_FRAGMENT_HEADER[..], &shaders::CONSTANT_COLOUR_FRAGMENT_CODE[..]),
        (shaders::DROP_COLOUR_VERTEX_OFFSET, &shaders::DROP_COLOUR_VERTEX_HEADER[..], &shaders::DROP_COLOUR_VERTEX_CODE[..]),
    ];
    let mut end = shaders::SEGMENT_SIZE;
    for (offset, header, code) in controls {
        assert!(offset >= end);
        assert_eq!((offset + header.len()) % 128, 0);
        assert_eq!((offset + header.len()) % 64, 0);
        end = offset + header.len() + code.len();
    }
    assert!(end <= shaders::CODE_REGION_SIZE);
    assert_eq!(shaders::CODE_REGION_SIZE % 64, 0);
}
