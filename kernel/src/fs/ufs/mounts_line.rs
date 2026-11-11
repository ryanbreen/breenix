use alloc::vec::Vec;

#[inline]
pub fn format_mount_line(source: &[u8], dir: &[u8]) -> Vec<u8> {
    fn append_field(out: &mut Vec<u8>, field: &[u8]) {
        for &byte in field {
            match byte {
                b' ' => out.extend_from_slice(b"\\040"),
                b'\t' => out.extend_from_slice(b"\\011"),
                b'\n' => out.extend_from_slice(b"\\012"),
                b'\\' => out.extend_from_slice(b"\\134"),
                _ => out.push(byte),
            }
        }
    }
    let mut out = Vec::new();
    append_field(&mut out, source);
    out.push(b' ');
    append_field(&mut out, dir);
    out.extend_from_slice(b" ufs rw 0 0\n");
    out
}
