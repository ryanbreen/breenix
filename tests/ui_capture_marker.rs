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
