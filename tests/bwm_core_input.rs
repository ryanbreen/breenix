//! Exercise the production core-to-window translation, without a guest boot.
// The session module also exports the window-management API used by bwm.
#[allow(dead_code)]
#[path = "../userspace/programs/src/bwm/session.rs"]
mod session;
#[path = "../userspace/programs/src/bwm/input_keys.rs"]
mod input_keys;

use libbreenix::input::{key, kind, modifier, Event};
use libbreenix::window::abi;

fn source(code: u16, value: i32, flags: u32) -> Event {
    Event { serial: 42, time_ns: 123456, device: 7, kind: kind::KEY, code, value, flags }
}

#[test]
fn core_keys_keep_linux_codes_modifiers_and_releases() {
    for code in [key::LEFT_CTRL, key::RIGHT_CTRL, key::LEFT_SUPER, key::RIGHT_SUPER,
                 key::ESC, key::ENTER, key::TAB, 14, key::HOME, key::END, key::UP,
                 key::DOWN, key::LEFT, key::RIGHT, key::PAGE_UP, key::PAGE_DOWN, key::DELETE] {
        for flags in [0, modifier::CTRL, modifier::SUPER, modifier::SHIFT | modifier::ALT] {
            for value in [0, 1] {
                let events = input_keys::events(source(code, value, flags), &[]);
                let text = value == 1 && [key::ESC, key::ENTER, key::TAB, 14].contains(&code);
                assert_eq!(events.len(), if text { 2 } else { 1 }, "code={code} flags={flags} value={value}");
                let e = events[0];
                assert_eq!(e.kind, if value == 0 { abi::event_kind::KEY_RELEASE } else { abi::event_kind::KEY_PRESS });
                assert_eq!((e.keycode, e.modifiers), (code as u32, flags));
                assert_eq!((e.serial, e.time_ns), (42, 123456));
                assert_eq!(e.text_len, u32::from(text));
            }
        }
    }
    assert_ne!(modifier::CTRL, modifier::SUPER);
}

#[test]
fn printable_and_terminal_control_presses_emit_text() {
    assert_eq!(input_keys::chord_key(key::ENTER), b'\r');
    assert_eq!(input_keys::chord_key(key::TAB), b'\t');
    assert_eq!(input_keys::chord_key(key::ESC), 27);
    assert_eq!(input_keys::chord_key(key::C), b'c');
    for (code, flags, text) in [(key::A, 0, b'a'), (key::A, modifier::SHIFT, b'A'),
                              (2, modifier::SHIFT, b'!'), (57, 0, b' ')] {
        let events = input_keys::events(source(code, 1, flags), &[]);
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].kind, events[0].keycode), (abi::event_kind::KEY_PRESS, code as u32));
        assert_eq!((events[0].text_len, events[0].text[0]), (1, text));
        assert_eq!(events[1].kind, abi::event_kind::TEXT);
        assert_eq!((events[1].text_len, events[1].text[0]), (1, text));
        assert_eq!(libbreenix::input::abi::derived_source(events[1].serial), Some((42, 1)));
        assert_eq!(events[1].time_ns, 123456);
    }
    for flags in [modifier::SUPER, modifier::ALT] {
        let events = input_keys::events(source(key::C, 1, flags), &[]);
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].keycode, events[0].modifiers, events[0].text_len), (key::C as u32, flags, 1));
        assert_eq!(events[0].text[0], b'c');
    }
    for (code, flags, byte) in [(key::C, modifier::CTRL, 3), (32, modifier::CTRL, 4),
        (key::ESC, 0, 27), (14, 0, 8), (key::TAB, 0, b'\t'), (key::ENTER, 0, b'\r')] {
        for value in [1, 2] {
            let events = input_keys::events(source(code, value, flags), &[]);
            assert_eq!(events.len(), 2);
            assert_eq!((events[0].text_len, events[0].text[0]), (1, byte));
            assert_eq!((events[1].text_len, events[1].text[0]), (1, byte));
        }
    }
    assert_eq!(input_keys::events(source(key::A, 0, 0), &[]).len(), 1);
}

#[test]
fn explicit_text_is_split_at_utf8_boundaries_without_truncation() {
    let text = format!("{}é界", "a".repeat(239));
    let e = Event { kind: kind::TEXT, code: 0, value: text.len() as i32, ..source(0, 1, 0) };
    let events = input_keys::events(e, text.as_bytes());
    assert_eq!(events.len(), 2);
    let mut received = Vec::new();
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.kind, abi::event_kind::TEXT);
        assert_eq!(event.time_ns, e.time_ns);
        assert_eq!(event.serial, if index == 0 { e.serial } else { libbreenix::input::abi::derived_serial(e.serial, index as u32).unwrap() });
        let bytes = &event.text[..event.text_len as usize];
        assert!(std::str::from_utf8(bytes).is_ok());
        received.extend_from_slice(bytes);
    }
    assert_eq!(received, text.as_bytes());
}

#[test]
fn absolute_axes_use_the_advertised_range() {
    for (minimum, maximum) in [(0, 32767), (-500, 500), (100, 1100)] {
        let cap = libbreenix::input::Capability { kind: kind::ABS, code: 0, minimum, maximum };
        assert_eq!(input_keys::normalize(minimum, cap, 2560), 0);
        assert_eq!(input_keys::normalize(maximum, cap, 2560), 2559);
        assert_eq!(input_keys::normalize((minimum + maximum) / 2, cap, 2560), 1279);
    }
}
