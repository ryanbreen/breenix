use libbreenix::window::abi::{event_kind, WindowEvent};
use breengel::{Delivered, Event};

#[test]
fn window_event_layout_is_unchanged() {
    use core::mem::{offset_of, size_of};
    assert_eq!(size_of::<WindowEvent>(), 320);
    for (actual, expected) in [
        (offset_of!(WindowEvent, serial), 0), (offset_of!(WindowEvent, time_ns), 8),
        (offset_of!(WindowEvent, epoch), 16), (offset_of!(WindowEvent, kind), 24),
        (offset_of!(WindowEvent, flags), 26), (offset_of!(WindowEvent, modifiers), 28),
        (offset_of!(WindowEvent, keycode), 32), (offset_of!(WindowEvent, button), 36),
        (offset_of!(WindowEvent, x), 40), (offset_of!(WindowEvent, y), 44),
        (offset_of!(WindowEvent, scroll_x), 48), (offset_of!(WindowEvent, scroll_y), 52),
        (offset_of!(WindowEvent, width), 56), (offset_of!(WindowEvent, height), 60),
        (offset_of!(WindowEvent, pressed), 64), (offset_of!(WindowEvent, text_len), 68),
        (offset_of!(WindowEvent, lost), 72), (offset_of!(WindowEvent, clicks), 76),
        (offset_of!(WindowEvent, text), 80),
    ] { assert_eq!(actual, expected); }
}

#[test]
fn pointer_events_and_clicks_decode() {
    assert_eq!((event_kind::POINTER_ENTER, event_kind::POINTER_LEAVE, event_kind::POINTER_CANCEL), (16, 17, 18));
    let mut raw = WindowEvent::new(event_kind::POINTER_ENTER);
    raw.serial = 42; raw.time_ns = 99; raw.x = -7; raw.y = 31; raw.clicks = 2;
    assert_eq!(Delivered::from_raw(&raw), Delivered { serial: 42, time_ns: 99, clicks: 2, event: Event::PointerEnter { x: -7, y: 31 } });
    raw.kind = event_kind::POINTER_LEAVE;
    assert_eq!(Event::from_raw(&raw), Event::PointerLeave { x: -7, y: 31 });
    raw.kind = event_kind::POINTER_CANCEL;
    assert_eq!(Event::from_raw(&raw), Event::PointerCancel);
    raw.kind = event_kind::POINTER_MOVE;
    assert_eq!(Event::from_raw(&raw), Event::MouseMove { x: -7, y: 31 });
    raw.kind = event_kind::FOCUS_GAINED;
    assert_eq!(Event::from_raw(&raw), Event::FocusGained);
    raw.kind = event_kind::LAST + 1;
    assert_eq!(Event::from_raw(&raw), Event::Unknown { kind: raw.kind });
}

#[test]
fn existing_kinds_keep_their_decoding() {
    use breengel::Modifiers;
    let mut raw = WindowEvent::new(1);
    raw.x = 3; raw.y = -4; raw.keycode = 9; raw.button = 0x111; raw.pressed = 1;
    raw.scroll_x = -2; raw.scroll_y = 5; raw.width = 80; raw.height = 60;
    raw.epoch = 77; raw.lost = 6; raw.text_len = 1; raw.text[0] = b'A';
    let mods = Modifiers::default();
    let expected = [
        Event::KeyPress { ascii: b'A', keycode: 9, modifiers: mods, text: "A".into(), repeat: false },
        Event::KeyRelease { keycode: 9, modifiers: mods }, Event::Text { text: "A".into() },
        Event::MouseMove { x: 3, y: -4 }, Event::MouseButton { button: 0x111, pressed: true, x: 3, y: -4 },
        Event::Scroll { delta_x: -2, delta_y: 5 }, Event::FocusGained, Event::FocusLost,
        Event::CloseRequested, Event::Resized { width: 80, height: 60 },
        Event::ResizeFailed { width: 80, height: 60 }, Event::FontChanged,
        Event::CompositorChanged { epoch: 77 }, Event::Overflow { lost: 6 },
        Event::ResizeRequested { width: 80, height: 60 },
    ];
    for (index, event) in expected.into_iter().enumerate() {
        raw.kind = index as u16 + 1;
        assert_eq!(Event::from_raw(&raw), event, "kind {}", raw.kind);
    }
}
