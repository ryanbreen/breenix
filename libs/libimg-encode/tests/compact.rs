#[test]
fn fixed_matches_and_stored_noise_roundtrip_at_length_boundaries() {
    for len in [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,22,23,26,27,30,31,34,35,42,43,50,51,58,59,66,67,82,83,98,99,114,115,130,131,162,163,194,195,226,227,257,258,259,516,65535,65536] {
        for distance in 1..=4 {
            let input: Vec<_> = (0..len).map(|i| (i % distance + 144) as u8).collect();
            let encoded = libimg_encode::deflate::zlib_compact(&input);
            assert_eq!(libimg::inflate::zlib_decompress(&encoded, len).unwrap(), input);
        }
        let mut seed = 42u32;
        let noise: Vec<_> = (0..len).map(|_| {seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed as u8}).collect();
        let encoded = libimg_encode::deflate::zlib_compact(&noise);
        assert_eq!(libimg::inflate::zlib_decompress(&encoded, len).unwrap(), noise);
        assert!(encoded.len() <= libimg_encode::deflate::zlib_compress(&noise).len());
    }
}
