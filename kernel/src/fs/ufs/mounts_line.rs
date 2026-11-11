use alloc::string::String;

pub fn format_mount_line(source: &[u8], dir: &[u8]) -> String {
    fn append_field(out: &mut String, field: &[u8]) {
        for ch in String::from_utf8_lossy(field).chars() {
            match ch {
                ' ' => out.push_str("\\040"),
                '\t' => out.push_str("\\011"),
                '\n' => out.push_str("\\012"),
                '\\' => out.push_str("\\134"),
                _ => out.push(ch),
            }
        }
    }
    let mut out = String::new();
    append_field(&mut out, source);
    out.push(' ');
    append_field(&mut out, dir);
    out.push_str(" ufs rw 0 0\n");
    out
}
