//! Edit the query using printable ASCII text from bwm.
pub fn append(query: &mut [u8; 64], len: &mut usize, selected: &mut usize, text: &str) {
    let before = *len;
    for byte in text.bytes().filter(|b| *b >= 0x20 && *b < 0x7f) {
        if *len < 63 { query[*len] = byte; *len += 1; }
    }
    if *len != before { *selected = 0; }
}
