//! Terminal keyboard encoding, following xterm's PC-style function keys.

pub fn encode(key: u32, ctrl: bool, text: &str, application: bool) -> Vec<u8> {
    let cursor = match key { 103 => Some(b'A'), 108 => Some(b'B'), 106 => Some(b'C'), 105 => Some(b'D'), 102 => Some(b'H'), 107 => Some(b'F'), _ => None };
    if let Some(last) = cursor {
        return vec![0x1b, if application { b'O' } else { b'[' }, last];
    }
    let sequence: &[u8] = match key {
        111 => b"\x1b[3~", 104 => b"\x1b[5~", 109 => b"\x1b[6~",
        59 => b"\x1bOP", 60 => b"\x1bOQ", 61 => b"\x1bOR", 62 => b"\x1bOS",
        14 => b"\x7f", 28 => b"\r", _ => b"",
    };
    if !sequence.is_empty() { return sequence.to_vec(); }
    if ctrl {
        let letter = match key {
            30 => Some(b'a'), 48 => Some(b'b'), 46 => Some(b'c'), 32 => Some(b'd'), 18 => Some(b'e'),
            33 => Some(b'f'), 34 => Some(b'g'), 35 => Some(b'h'), 23 => Some(b'i'), 36 => Some(b'j'),
            37 => Some(b'k'), 38 => Some(b'l'), 50 => Some(b'm'), 49 => Some(b'n'), 24 => Some(b'o'),
            25 => Some(b'p'), 16 => Some(b'q'), 19 => Some(b'r'), 31 => Some(b's'), 20 => Some(b't'),
            22 => Some(b'u'), 47 => Some(b'v'), 17 => Some(b'w'), 45 => Some(b'x'), 21 => Some(b'y'), 44 => Some(b'z'),
            _ => text.bytes().next().filter(|_| text.len() == 1).map(|b| b.to_ascii_lowercase()),
        };
        if let Some(letter @ b'a'..=b'z') = letter { return vec![letter - b'a' + 1]; }
    }
    text.as_bytes().to_vec()
}

/// A companion TEXT event repeats the preceding physical key's text. Explicit
/// text sources, including their continuation chunks, carry their own bytes.
pub fn encode_text(text: &str, source: Option<u64>, last_key: u64) -> Vec<u8> {
    if source == Some(last_key) { Vec::new() } else { text.as_bytes().to_vec() }
}
