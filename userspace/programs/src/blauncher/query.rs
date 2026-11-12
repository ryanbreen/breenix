//! Edit the query using printable ASCII text from bwm.
pub fn append(query: &mut [u8; 64], len: &mut usize, selected: &mut usize, text: &str) {
    let before = *len;
    for byte in text.bytes().filter(|b| *b >= 0x20 && *b < 0x7f) {
        if *len < 63 { query[*len] = byte; *len += 1; }
    }
    if *len != before { *selected = 0; }
}

/// Append the first clipboard line, retaining complete printable UTF-8 characters.
pub fn paste(query: &mut [u8; 64], len: &mut usize, selected: &mut usize, bytes: &[u8]) {
    let end = bytes.iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(bytes.len());
    for ch in String::from_utf8_lossy(&bytes[..end]).chars().filter(|c| !c.is_control()) {
        let mut encoded = [0; 4];
        let part = ch.encode_utf8(&mut encoded).as_bytes();
        if *len + part.len() > query.len() { break; }
        query[*len..*len + part.len()].copy_from_slice(part);
        *len += part.len();
    }
    *selected = 0;
}
