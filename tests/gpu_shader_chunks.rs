#[path = "../kernel/src/gpu/virgl/shader_chunks.rs"]
mod shader_chunks;

#[test]
fn shader_fragments_fit_a_page_and_reconstruct_exact_bytes() {
    for bytes in [1usize, 4, 4071, 4072, 4073, 65536, 252216, 262145] {
        let words: Vec<u32> = (0..bytes.div_ceil(4) as u32).collect();
        let mut rebuilt = Vec::new();
        for (index, (offset, part)) in shader_chunks::fragments(&words, bytes).enumerate() {
            assert!(part.len() + 6 <= 1024);
            assert!(part.len() <= shader_chunks::TEXT_WORDS);
            if index == 0 {
                assert_eq!(offset, bytes as u32);
            } else {
                assert_eq!(offset & 0x8000_0000, 0x8000_0000);
                assert_eq!((offset & 0x7fff_ffff) as usize, rebuilt.len() * 4);
            }
            rebuilt.extend_from_slice(part);
        }
        assert_eq!(rebuilt, words);
    }
}
