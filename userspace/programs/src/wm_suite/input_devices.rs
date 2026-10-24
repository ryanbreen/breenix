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
/// Keep read serials pending to fill the ledger without reader overflow.
pub fn stalled_ledger() -> CaseResult {
    let _claim = InputClaim::new()?;
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    stream.grab(&[])?;
    let mut last = 0;
    let edges = (0..255).map(|n| (1, 30, (n & 1) as i32)).collect::<Vec<_>>();
    for n in 1..=32 {
        device.batch(&edges, n)?;
        last = stream.read_from(256, &[device.id])?.last().unwrap().serial;
    }
    check(device.batch(&[(1, 30, 0)], 33).err()
        .is_some_and(|e| e.starts_with("uinput batch errno 11:")),
        "pending ledger did not reach its unchanged capacity")?;
    replay(1, 0, 0)?;
    let events = stream.read(4)?;
    check(events.iter().any(|e| e.kind == 1 && e.code == 30 && e.value == 1 && e.device == 2),
        "full pending ledger stopped the physical keyboard")?;
    check(events.iter().any(|e| e.kind == 0 && e.code == 3),
        "hardware ledger overflow omitted SYN_DROPPED")?;
    // Inspect the full snapshot without allocating a String for each field.
    let text = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
    let serial = last.to_string();
    check(text.lines().any(|row| {
        let mut fields = row.split_whitespace();
        fields.next() == Some(serial.as_str()) && fields.next() == Some("lost")
            && fields.next().is_some() && fields.next() == Some("overflow")
            && fields.next().is_none()
    }), "hardware ledger overflow omitted the pending source loss")
}

pub fn pointer_completion() -> CaseResult {
    let (d, mut c) = one()?;
    let w = c.info()?;
    d.pointer(w.x + 20, w.y + 20, 0, 0)?;
    c.await_event("positive tablet polling consumer before same-frame click", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (20, 20)
    })?;
    c.clear()?;
    let mut stream = Stream::open()?;
    replay(2, w.x + 40, w.y + 50)?;
    let events = stream.read(5)?;
    check(
        events.iter().any(|e| e.kind == 3),
        "bound pointer completion did not report absolute axes",
    )?;
    let edges = events.iter().filter(|e| e.kind == 1 && e.code == 272).collect::<Vec<_>>();
    value("pointer-source-edges", edges.len() as i64, "", Some((2, 2)));
    check(edges.len() == 2 && edges[0].value == 1 && edges[1].value == 0
        && edges[0].serial < edges[1].serial && edges[0].device == 3 && edges[1].device == 3,
        "bound tablet completion lost or reordered same-frame button edges")?;
    c.await_event("bound pointer completion wakes bwm", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (40, 50)
    })?;
    let press = c.await_event("completed-frame pointer press", |e| {
        e.kind == kind::POINTER_BUTTON && e.button == 272 && e.pressed == 1
    })?;
    let release = c.await_event("completed-frame pointer release", |e| {
        e.kind == kind::POINTER_BUTTON && e.button == 272 && e.pressed == 0
    })?;
    check(press.serial < release.serial && (press.x, press.y) == (40, 50)
        && (release.x, release.y) == (40, 50),
        "completed-frame fast click lost coordinates or reordered a client edge")
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

/// Selector 3 runs the kernel's stateful boot-keyboard decoder. Replies carry
/// a phantom flag and literal normalized key edges, without publishing input.
fn keyboard_decode(reports: &[[u8; 8]]) -> Result<Vec<(bool, Vec<(u16, i32)>)>, String> {
    use std::io::{Read, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/breenix/input-decode")
        .map_err(|e| format!("production keyboard decoder transport is absent: {e}"))?;
    let mut decoded = Vec::new();
    for (n, report) in reports.iter().enumerate() {
        let mut request = Vec::new();
        request.extend_from_slice(&3u32.to_le_bytes());
        request.extend_from_slice(&u32::from(n == 0).to_le_bytes());
        request.extend_from_slice(report);
        f.write_all(&request).map_err(|e| e.to_string())?;
        let mut header = [0; 8];
        f.read_exact(&mut header).map_err(|e| e.to_string())?;
        let phantom = u32::from_le_bytes(header[..4].try_into().unwrap());
        let count = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if phantom > 1 || count > 32 {
            return Err("invalid keyboard decoder reply".into());
        }
        let mut edges = Vec::new();
        for _ in 0..count {
            let mut raw = [0; 8];
            f.read_exact(&mut raw).map_err(|e| e.to_string())?;
            if u16::from_le_bytes(raw[..2].try_into().unwrap()) != 1 {
                return Err("keyboard decoder emitted a non-key edge".into());
            }
            edges.push((
                u16::from_le_bytes(raw[2..4].try_into().unwrap()),
                i32::from_le_bytes(raw[4..].try_into().unwrap()),
            ));
        }
        decoded.push((phantom == 1, edges));
    }
    Ok(decoded)
}
pub fn hid_keyboard_decode() -> CaseResult {
    let got = keyboard_decode(&[[2, 0, 4, 5, 0, 0, 0, 0], [0; 8]])?;
    check(
        got == [
            (false, vec![(42, 1), (30, 1), (48, 1)]),
            (false, vec![(42, 0), (30, 0), (48, 0)]),
        ],
        "production boot-keyboard decode differs from literal key edges",
    )
}
pub fn hid_rollover() -> CaseResult {
    let got = keyboard_decode(&[[0, 0, 4, 5, 6, 7, 8, 9], [0, 0, 1, 1, 1, 1, 1, 1], [0; 8]])?;
    check(
        got == [
            (
                false,
                vec![(30, 1), (48, 1), (46, 1), (32, 1), (18, 1), (33, 1)],
            ),
            (true, vec![]),
            (
                false,
                vec![(30, 0), (48, 0), (46, 0), (32, 0), (18, 0), (33, 0)],
            ),
        ],
        "production phantom state did not retain the six held keys",
    )
}
// HID 1.11 Appendix B.2 mouse fields with a signed wheel input.
const MOUSE: &[u8] = &[
    0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 1, 0xa1, 0, 0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1, 0x95,
    3, 0x75, 1, 0x81, 2, 0x95, 1, 0x75, 5, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38,
    0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 3, 0x81, 6, 0xc0, 0xc0,
];
fn pointer_decode(reports: &[[u8; 4]]) -> Result<Vec<[i32; 4]>, String> {
    use std::io::{Read, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/breenix/input-decode")
        .map_err(|e| format!("production pointer decoder transport is absent: {e}"))?;
    let mut decoded = Vec::new();
    for (n, report) in reports.iter().enumerate() {
        let mut request = Vec::new();
        for v in [
            2u32,
            0,
            MOUSE.len() as u32,
            report.len() as u32,
            u32::from(n == 0),
        ] {
            request.extend_from_slice(&v.to_le_bytes());
        }
        request.extend_from_slice(MOUSE);
        request.extend_from_slice(report);
        f.write_all(&request).map_err(|e| e.to_string())?;
        let mut reply = [0; 16];
        f.read_exact(&mut reply).map_err(|e| e.to_string())?;
        decoded.push(std::array::from_fn(|i| {
            i32::from_le_bytes(reply[i * 4..i * 4 + 4].try_into().unwrap())
        }));
    }
    Ok(decoded)
}
pub fn hid_relative() -> CaseResult {
    let got = pointer_decode(&[[5, 0xfe, 4, 0], [0, 0, 0, 0]])?;
    check(
        got == [[-2, 4, 5, 0], [0, 0, 0, 0]],
        "production relative mouse decode differs from literal motion and buttons",
    )?;
    // Exercise the decoded deltas through the real pointer accumulator as well.
    let mut v = Virtual::create()?;
    let (mut d, c) = one()?;
    d.pointer(1, d.height as i32 - 2, 0, 0)?;
    time::sleep_ms(150)?;
    v.batch(&[(2, 0, got[0][0]), (2, 1, got[0][1])], 100)?;
    until(
        1000,
        "decoded relative deltas clamp at the mode edges",
        || Ok(Screen::read()?.pixel(0, d.height as i32 - 1)? == 0xff000000),
    )?;
    d.shown(&c)?;
    Ok(())
}
pub fn hid_wheel() -> CaseResult {
    check(
        pointer_decode(&[[0, 0, 0, 0xff], [0, 0, 0, 1], [0, 0, 0, 0]])?
            == [[0, 0, 0, -1], [0, 0, 0, 1], [0, 0, 0, 0]],
        "production signed wheel decode differs from literal steps",
    )
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
    let registered = |line: &str| -> Result<Vec<(u16, u16, i32, i32)>, String> {
        line.split_whitespace().skip(2).map(|entry| {
            let fields = entry.split(':').collect::<Vec<_>>();
            if fields.len() != 4 { return Err("invalid registered capability".into()); }
            Ok((fields[0].parse().map_err(|_| "invalid kind")?,
                fields[1].parse().map_err(|_| "invalid code")?,
                fields[2].parse().map_err(|_| "invalid minimum")?,
                fields[3].parse().map_err(|_| "invalid maximum")?))
        }).collect()
    };
    check(registered(keyboard)? == (1..=255).map(|code| (1, code, 0, 2)).collect::<Vec<_>>(),
        "registered keyboard key capabilities differ")?;
    check(registered(pointer)? == vec![
        (3, 0, 0, 32767), (3, 1, 0, 32767),
        (1, 272, 0, 1), (1, 273, 0, 1), (1, 274, 0, 1),
        (2, 6, i32::MIN, i32::MAX), (2, 8, i32::MIN, i32::MAX),
    ], "registered tablet axes/buttons/wheel capabilities differ")
}
