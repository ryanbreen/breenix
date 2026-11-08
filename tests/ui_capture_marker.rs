#[path = "../libs/libuitest/src/capture_marker.rs"]
mod capture_marker;
use capture_marker::{decode, encode, whole, Generation, MARKER_BYTES};

#[test]
fn marker_round_trip_and_whole_frame() {
    let generation = Generation { commit: 0x1020_3040_5060_7080, producer: 0x1122_3344 };
    let marker = encode(generation);
    assert_eq!(decode(&marker), Some(generation));
    let mut pixels = vec![0; MARKER_BYTES * 3];
    for row in pixels.chunks_exact_mut(MARKER_BYTES) { row.copy_from_slice(&marker); }
    assert!(whole(&pixels, MARKER_BYTES, 3, &marker, generation.commit));
    assert!(!whole(&pixels, MARKER_BYTES, 3, &marker, generation.commit + 1));
    pixels[MARKER_BYTES] ^= 1;
    assert!(!whole(&pixels, MARKER_BYTES, 3, &marker, generation.commit));
    pixels[MARKER_BYTES] ^= 1;
    let other = encode(Generation { producer: generation.producer + 1, ..generation });
    assert!(!whole(&pixels, MARKER_BYTES, 3, &other, generation.commit));
    assert!(!whole(&pixels[..pixels.len()-1], MARKER_BYTES, 3, &marker, generation.commit));
}

#[test]
fn marker_frame_is_opaque_bounded_and_complete() {
    use capture_marker::{frame, BACKGROUND};
    let g = Generation { commit: 9, producer: 2 };
    let pixels = frame(16, 4, g).unwrap();
    assert!(whole(&pixels, 64, 4, &encode(g), 9));
    assert_eq!(&pixels[48..52], &BACKGROUND);
    assert!(pixels.chunks_exact(4).all(|p| p[3] == 255));
    assert!(frame(MARKER_BYTES/4 - 1, 4, g).is_err());
    assert!(frame(16, 0, g).is_err());
    assert!(frame(usize::MAX, 4, g).is_err());
    assert!(!whole(&pixels, usize::MAX, 4, &encode(g), 9));
    for n in 0..MARKER_BYTES { assert_eq!(decode(&encode(g)[..n]), None); }
    let mut invalid = encode(g);
    invalid[3] = 0;
    assert_eq!(decode(&invalid), None);
}
