//! Streaming parser for the processor records published by Breenix procfs.
//! It retains only a prefix position, regardless of line length or read boundaries.
pub(super) struct ProcessorCount {
    prefix: usize,
    count: usize,
}
impl ProcessorCount {
    pub(super) const fn new() -> Self { Self { prefix: 0, count: 0 } }
    pub(super) fn feed(&mut self, bytes: &[u8]) {
        const NAME: &[u8] = b"processor";
        for &byte in bytes {
            if byte == b'\n' {
                self.prefix = 0;
            } else if self.prefix < NAME.len() && byte == NAME[self.prefix] {
                self.prefix += 1;
            } else if self.prefix == NAME.len() && matches!(byte, b' ' | b'\t') {
                // Whitespace between the field name and colon.
            } else {
                if self.prefix == NAME.len() && byte == b':' {
                    self.count += 1;
                }
                self.prefix = usize::MAX;
            }
        }
    }
    pub(super) fn get(&self) -> usize { self.count }
}
