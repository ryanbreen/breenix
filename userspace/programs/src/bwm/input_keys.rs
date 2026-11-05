//! Core-key translation harvested from #2476 dfc2e318, without repeat policy.
use super::session::{self, Input};
use libbreenix::input::{self, kind, Capability, Event};
use libbreenix::window::abi;

/// US key text uses the physical key code; modifiers remain independent bits.
fn key_text(code: u16, mods: u32) -> Option<u8> {
    if mods
        & (libbreenix::input::modifier::CTRL
            | libbreenix::input::modifier::ALT
            | libbreenix::input::modifier::SUPER)
        != 0
    {
        return None;
    }
    let shift = mods & libbreenix::input::modifier::SHIFT != 0;
    let letter = match code {
        16..=25 => Some(b"qwertyuiop"[(code - 16) as usize]),
        30..=38 => Some(b"asdfghjkl"[(code - 30) as usize]),
        44..=50 => Some(b"zxcvbnm"[(code - 44) as usize]),
        _ => None,
    };
    if let Some(c) = letter {
        return Some(if shift { c.to_ascii_uppercase() } else { c });
    }
    match code {
        2..=11 => Some(if shift {
            b"!@#$%^&*()"[(code - 2) as usize]
        } else {
            b"1234567890"[(code - 2) as usize]
        }),
        12 => Some(if shift { b'_' } else { b'-' }),
        13 => Some(if shift { b'+' } else { b'=' }),
        26 => Some(if shift { b'{' } else { b'[' }),
        27 => Some(if shift { b'}' } else { b']' }),
        39 => Some(if shift { b':' } else { b';' }),
        40 => Some(if shift { b'"' } else { b'\'' }),
        41 => Some(if shift { b'~' } else { b'`' }),
        43 => Some(if shift { b'|' } else { b'\\' }),
        51 => Some(if shift { b'<' } else { b',' }),
        52 => Some(if shift { b'>' } else { b'.' }),
        53 => Some(if shift { b'?' } else { b'/' }),
        57 => Some(b' '),
        _ => None,
    }
}

pub(super) fn chord_key(code: u16) -> u8 {
    match code {
        input::key::ENTER => b'\r',
        input::key::TAB => b'\t',
        input::key::ESC => 27,
        14 => 8,
        _ => key_text(code, 0).unwrap_or(0),
    }
}

pub(super) fn normalize(value: i32, cap: Capability, extent: usize) -> i32 {
    ((value as i64 - cap.minimum as i64) * extent.saturating_sub(1) as i64
        / (cap.maximum as i64 - cap.minimum as i64))
        .clamp(0, extent.saturating_sub(1) as i64) as i32
}

pub(super) fn events(source: Event, payload: &[u8]) -> Vec<abi::WindowEvent> {
    let mut events = Vec::new();
    match source.kind {
        kind::KEY if source.value == 0 => events.push(session::event(Input::KeyRelease {
            keycode: source.code as u32,
            modifiers: source.flags,
        })),
        kind::KEY => {
            let text = key_text(source.code, source.flags);
            events.push(session::event(Input::Key {
                keycode: source.code as u32,
                modifiers: source.flags,
                repeat: source.value == 2,
                text,
            }));
            if let Some(text) = text {
                let mut bytes = [0; 240];
                bytes[0] = text;
                events.push(session::event(Input::Text { bytes, len: 1 }));
            }
        }
        kind::TEXT => {
            let text = std::str::from_utf8(payload).expect("core UTF-8");
            let mut at = 0;
            while at < text.len() {
                let mut end = (at + 240).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let mut bytes = [0; 240];
                bytes[..end - at].copy_from_slice(&payload[at..end]);
                events.push(session::event(Input::Text {
                    bytes,
                    len: (end - at) as u32,
                }));
                at = end;
            }
        }
        _ => {}
    }
    for (index, event) in events.iter_mut().enumerate() {
        event.serial = if index == 0 {
            source.serial
        } else {
            input::abi::derived_serial(source.serial, index as u32).expect("input derivation")
        };
        event.time_ns = source.time_ns;
        event.modifiers = source.flags;
    }
    events
}
