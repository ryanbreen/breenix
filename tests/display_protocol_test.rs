#[path = "../userspace/programs/src/bwm/display_protocol.rs"]
mod display_protocol;
use display_protocol::{parse, reply, Command, Geometry, Rect};

#[test]
fn parses_commands_and_refuses_malformed_requests() {
    assert_eq!(parse("mode 1920x1080\n"), Ok(Command::Mode(1920, 1080)));
    assert_eq!(parse("scale 2\n"), Ok(Command::Scale(2)));
    for input in ["mode 0x1080\n", "mode 12x3x4\n", "scale 3\n", "scale 1 extra\n", "mode 2x3", "hello\n"] {
        assert!(parse(input).is_err(), "{input:?}");
    }
}

#[test]
fn formats_success_and_refusal_as_one_line() {
    let rect = Rect { x: 1, y: 2, width: 3, height: 4 };
    let geometry = Geometry { width: 1920, height: 1080, scale: 2, taskbar: rect, appbar: rect, work: rect };
    assert_eq!(reply(Ok(geometry)), "ok 1920x1080 scale 2 taskbar 1,2,3,4 appbar 1,2,3,4 work 1,2,3,4\n");
    assert_eq!(reply(Err("unsupported")), "refused unsupported\n");
}
