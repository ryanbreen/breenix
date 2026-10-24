use super::kit::*;

pub fn keyboard_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    check(
        stream
            .read(2)?
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 1),
        "bound keyboard completion did not report A",
    )?;
    c.await_event("bound keyboard completion wakes bwm and delivers A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    Ok(())
}
pub fn pointer_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    {
        let w = c.info()?;
        replay(2, w.x + 40, w.y + 50)?;
    };
    check(
        stream.read(4)?.iter().any(|e| e.kind == 3),
        "bound pointer completion did not report absolute axes",
    )?;
    c.await_event("bound pointer completion wakes bwm", |e| {
        e.kind == kind::POINTER_MOVE
    })?;
    Ok(())
}
pub fn replay_faults() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    c.await_event("positive keyboard completion before planted fault", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    stream.read(2)?;
    c.clear()?;
    replay(0x101, 0, 0)?; // same production completion, suppress its report once
    check(
        stream.read(1).is_err(),
        "report-suppression fault still published input",
    )?;
    c.snapshot()?;
    check(
        c.events.iter().all(|e| e.kind != kind::KEY_PRESS),
        "report-suppression fault still reached a client",
    )
}

pub fn hid_keyboard_decode() -> CaseResult {
    let mut k = super::hid_keyboard::Keyboard::new();
    let mut events = Vec::new();
    k.report(&[2, 0, 4, 5, 0, 0, 0, 0], 0, &mut |c, v| {
        events.push((c, v))
    })
    .map_err(|e| format!("HID report: {e:?}"))?;
    k.report(&[0; 8], 1, &mut |c, v| events.push((c, v)))
        .map_err(|e| format!("HID release: {e:?}"))?;
    check(
        events == [(42, 1), (30, 1), (48, 1), (42, 0), (30, 0), (48, 0)],
        "boot keyboard report differs from literal key edges",
    )
}
pub fn hid_rollover() -> CaseResult {
    let mut k = super::hid_keyboard::Keyboard::new();
    let mut events = Vec::new();
    k.report(&[0, 0, 4, 5, 6, 7, 8, 9], 0, &mut |c, v| {
        events.push((c, v))
    })
    .map_err(|e| format!("six-key report: {e:?}"))?;
    check(
        events == [(30, 1), (48, 1), (46, 1), (32, 1), (18, 1), (33, 1)],
        "six-key rollover edges differ",
    )?;
    events.clear();
    let phantom = k
        .report(&[0, 0, 1, 1, 1, 1, 1, 1], 1, &mut |c, v| {
            events.push((c, v))
        })
        .map_err(|e| format!("phantom report: {e:?}"))?;
    check(
        phantom.phantom && events.is_empty(),
        "phantom report changed held keys",
    )?;
    k.report(&[0; 8], 2, &mut |c, v| events.push((c, v)))
        .map_err(|e| format!("rollover release: {e:?}"))?;
    check(
        events == [(30, 0), (48, 0), (46, 0), (32, 0), (18, 0), (33, 0)],
        "phantom state did not retain all six held keys",
    )
}
// HID 1.11 Appendix B.2 mouse fields with a signed wheel input.
const MOUSE: &[u8] = &[
    0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 1, 0xa1, 0, 0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1, 0x95,
    3, 0x75, 1, 0x81, 2, 0x95, 1, 0x75, 5, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38,
    0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 3, 0x81, 6, 0xc0, 0xc0,
];
pub fn hid_relative() -> CaseResult {
    let layout = super::hid_pointer::Layout::parse(MOUSE)?;
    let m = layout
        .decode(&[5, 0xfe, 4, 0])?
        .ok_or("relative report yielded no pointer motion")?;
    check(
        layout.axes() == "relative"
            && m.buttons == Some(5)
            && m.x
                .is_some_and(|a| a.relative && a.value == -2 && a.position(1, 1280) == 0)
            && m.y
                .is_some_and(|a| a.relative && a.value == 4 && a.position(1598, 1600) == 1599),
        "relative mouse report differs from literal motion/buttons/clamping",
    )
}
pub fn hid_wheel() -> CaseResult {
    let layout = super::hid_pointer::Layout::parse(MOUSE)?;
    for (b, expected) in [(0xff, -1), (1, 1), (0, 0)] {
        let m = layout
            .decode(&[0, 0, 0, b])?
            .ok_or("wheel report yielded no pointer motion")?;
        check(
            m.wheel == expected,
            "signed HID wheel report differs from literal steps",
        )?;
    }
    Ok(())
}
/// Decode-only test transport runs the same pure virtio decoder used by completion.
fn virtio_decode(events: &[(u16, u16, i32)]) -> CaseResult {
    use std::io::{Read, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/breenix/input-decode")
        .map_err(|e| format!("pure input decoder /proc/breenix/input-decode is absent: {e}"))?;
    for &(kind, code, value) in events {
        let mut request = [0; 16];
        request[..4].copy_from_slice(&1u32.to_le_bytes());
        request[8..10].copy_from_slice(&kind.to_le_bytes());
        request[10..12].copy_from_slice(&code.to_le_bytes());
        request[12..16].copy_from_slice(&value.to_le_bytes());
        f.write_all(&request).map_err(|e| e.to_string())?;
        let mut reply = [0; 8];
        f.read_exact(&mut reply).map_err(|e| e.to_string())?;
        check(
            u16::from_le_bytes(reply[..2].try_into().unwrap()) == kind
                && u16::from_le_bytes(reply[2..4].try_into().unwrap()) == code
                && i32::from_le_bytes(reply[4..8].try_into().unwrap()) == value,
            "virtio pure decode differs from literal core event",
        )?;
    }
    Ok(())
}
pub fn virtio_keyboard_decode() -> CaseResult {
    virtio_decode(&[(1, 30, 1), (1, 30, 0), (1, 42, 1), (1, 42, 0)])
}
pub fn virtio_tablet_decode() -> CaseResult {
    virtio_decode(&[
        (3, 0, 0),
        (3, 1, 32767),
        (1, 0x110, 1),
        (1, 0x111, 1),
        (1, 0x112, 1),
        (2, 8, -1),
        (2, 6, 1),
        (0, 0, 0),
    ])
}
pub fn replay_wake_fault() -> CaseResult {
    use libbreenix::test_control as tc;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    c.await_event("positive completion wake", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.clear()?;
    let point = abi::tc_point::INPUT_WAKE_SKIPPED;
    let before = tc::fired(point)?;
    tc::arm(point, 0, 0)?;
    replay(1, 0, 0)?;
    check(
        tc::fired(point)? == before + 1,
        "wake suppression did not fire exactly once",
    )?;
    time::sleep_ms(100)?;
    c.snapshot()?;
    check(
        c.events.iter().all(|e| e.kind != kind::KEY_PRESS),
        "idle compositor still received a suppressed wake",
    )?;
    tc::disarm_all()?;
    replay(1, 0, 0)?;
    c.await_event("clean completion wake after fault", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    Ok(())
}
pub fn devices_bound() -> CaseResult {
    let text = std::fs::read_to_string("/proc/breenix/input-devices")
        .map_err(|e| format!("bound input device inventory is absent: {e}"))?;
    let mut names = text
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect::<Vec<_>>();
    names.sort_unstable();
    check(
        names == ["virtio-keyboard-mmio", "virtio-tablet-mmio"],
        "ARM64 QEMU bindings differ from the coverage map",
    )
}
pub fn device_capabilities() -> CaseResult {
    let text = std::fs::read_to_string("/proc/breenix/input-devices")
        .map_err(|e| format!("input capability inventory is absent: {e}"))?;
    let keyboard = text
        .lines()
        .find(|l| l.split_whitespace().nth(1) == Some("virtio-keyboard-mmio"))
        .ok_or("bound keyboard absent")?;
    let pointer = text
        .lines()
        .find(|l| l.split_whitespace().nth(1) == Some("virtio-tablet-mmio"))
        .ok_or("bound tablet absent")?;
    check(
        keyboard.split_whitespace().any(|s| s == "key=1..255"),
        "keyboard key capabilities differ",
    )?;
    for cap in [
        "abs=0:0..32767",
        "abs=1:0..32767",
        "key=272..274",
        "rel=6,8",
    ] {
        check(
            pointer.split_whitespace().any(|s| s == cap),
            "tablet axes/buttons/wheel capabilities differ from declaration",
        )?;
    }
    Ok(())
}

/// Target captures contain descriptor/report bytes and independent literal
/// expected values. Missing captures fail explicitly; synthetic layouts do not
/// stand in for a recording of a different target.
fn captured_tablet(target: &str) -> CaseResult {
    use std::io::{Read, Write};
    let path = format!("/usr/share/breenix/input-fixtures/{target}.txt");
    let capture = std::fs::read_to_string(&path)
        .map_err(|e| format!("recorded {target} tablet fixture is absent: {e}"))?;
    let mut decoder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/breenix/input-decode")
        .map_err(|e| {
            format!("production HID decoder and endpoint merger test transport is absent: {e}")
        })?;
    let mut reports = 0;
    for line in capture
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 7 {
            return Err(
                "tablet capture requires endpoint descriptor report x y buttons wheel".into(),
            );
        }
        let endpoint = fields[0].parse::<u32>().map_err(|e| e.to_string())?;
        let hex = |s: &str| -> Result<Vec<u8>, String> {
            if s.len() % 2 != 0 {
                return Err("capture has odd hex length".into());
            }
            s.as_bytes()
                .chunks_exact(2)
                .map(|b| {
                    u8::from_str_radix(std::str::from_utf8(b).map_err(|e| e.to_string())?, 16)
                        .map_err(|e| e.to_string())
                })
                .collect()
        };
        let descriptor = hex(fields[1])?;
        let report = hex(fields[2])?;
        let expected = fields[3..]
            .iter()
            .map(|s| s.parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut request = Vec::new();
        for v in [
            2u32,
            endpoint,
            descriptor.len() as u32,
            report.len() as u32,
            u32::from(reports == 0),
        ] {
            request.extend_from_slice(&v.to_le_bytes());
        }
        request.extend_from_slice(&descriptor);
        request.extend_from_slice(&report);
        decoder.write_all(&request).map_err(|e| e.to_string())?;
        let mut reply = [0; 16];
        decoder.read_exact(&mut reply).map_err(|e| e.to_string())?;
        let actual = reply
            .chunks_exact(4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>();
        check(
            actual == expected,
            "captured tablet decode/merged endpoint state differs from its literal expected values",
        )?;
        reports += 1;
    }
    check(
        reports >= 2,
        "tablet capture does not exercise a report transition",
    )
}
pub fn hid_tablet_parallels() -> CaseResult {
    captured_tablet("parallels")
}
pub fn hid_tablet_vmware() -> CaseResult {
    captured_tablet("vmware")
}
