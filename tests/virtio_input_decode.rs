#[path = "../kernel/src/drivers/virtio/input_decode.rs"]
mod input_decode;
use input_decode::{decode, Event, Frame};
fn wire(kind: u16, code: u16, value: i32) -> [u8; 8] {
    let mut b = [0; 8];
    b[..2].copy_from_slice(&kind.to_le_bytes());
    b[2..4].copy_from_slice(&code.to_le_bytes());
    b[4..].copy_from_slice(&value.to_le_bytes());
    b
}
#[test]
fn literal_keyboard_buttons_axes_and_signed_wheels() {
    for (kind, code, value) in [
        (1, 30, 1),
        (1, 30, 0),
        (1, 42, 2),
        (1, 272, 1),
        (1, 273, 0),
        (1, 274, 1),
        (3, 0, 0),
        (3, 1, 32767),
        (2, 0, -20),
        (2, 1, 40),
        (2, 6, 1),
        (2, 8, -1),
        (0, 0, 0),
    ] {
        assert_eq!(
            decode(wire(kind, code, value)),
            Some(Event { kind, code, value })
        );
    }
    for (kind, code, value) in [
        (1, 30, 3),
        (0, 0, 1),
        (0, 1, 0),
        (1, 275, 1),
        (3, 2, 0),
        (7, 0, 0),
    ] {
        assert_eq!(decode(wire(kind, code, value)), None);
    }
}
#[test]
fn button_up_before_axes_and_partial_xy_publish_only_at_syn() {
    let mut frame = Frame::new();
    let expected = [
        Event {
            kind: 1,
            code: 272,
            value: 0,
        },
        Event {
            kind: 3,
            code: 0,
            value: 100,
        },
        Event {
            kind: 3,
            code: 1,
            value: 200,
        },
        Event {
            kind: 0,
            code: 0,
            value: 0,
        },
    ];
    // A consumer between X and Y has no completed frame to observe.
    for e in &expected[..3] {
        assert!(frame.push(*e).is_none());
    }
    let (events, overflow) = frame.push(expected[3]).unwrap();
    assert!(!overflow);
    assert_eq!(events, expected);
    assert!(frame
        .push(Event {
            kind: 1,
            code: 30,
            value: 1
        })
        .is_none());
    let (events, overflow) = frame.push(Event::default()).unwrap();
    assert!(!overflow);
    assert_eq!(events.len(), 2);
}
#[test]
fn overlong_frame_is_flagged_instead_of_publishing_a_prefix() {
    let mut frame = Frame::new();
    for _ in 0..input_decode::FRAME_EVENTS + 1 {
        assert!(frame
            .push(Event {
                kind: 2,
                code: 0,
                value: 1
            })
            .is_none());
    }
    let (_, overflow) = frame.push(Event::default()).unwrap();
    assert!(overflow);
    let (events, overflow) = frame.push(Event::default()).unwrap();
    assert!(!overflow);
    assert_eq!(events, [Event::default()]);
}
