//! PNG §5.5 CRC-32, polynomial x^32+x^26+x^23+…+1, reflected form.
const fn table() -> [u32; 256] {
    let mut t = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xedb88320
            } else {
                c >> 1
            };
            bit += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
const TABLE: [u32; 256] = table();
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_parts(bytes, &[])
}
pub fn crc32_parts(a: &[u8], b: &[u8]) -> u32 {
    let mut c = u32::MAX;
    for &v in a.iter().chain(b) {
        crate::work::step();
        c = (c >> 8) ^ TABLE[((c ^ v as u32) & 255) as usize];
    }
    !c
}
