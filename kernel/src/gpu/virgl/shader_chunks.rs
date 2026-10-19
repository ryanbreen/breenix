//! Shader-text fragmentation from P:273–282. A complete command fits one DMA page.

/// Command header plus five shader fields occupy six words of a 4096-byte page.
pub const TEXT_WORDS: usize = 1024 - 6;

/// The first offset is the unpadded total byte length. Continuations have bit 31
/// set and the byte offset of their first text word. Padding follows the final NUL.
pub fn fragments(words: &[u32], bytes: usize) -> impl Iterator<Item = (u32, &[u32])> {
    words
        .chunks(TEXT_WORDS)
        .enumerate()
        .map(move |(index, part)| {
            let offset = if index == 0 {
                bytes as u32
            } else {
                0x8000_0000 | (index * TEXT_WORDS * 4) as u32
            };
            (offset, part)
        })
}
