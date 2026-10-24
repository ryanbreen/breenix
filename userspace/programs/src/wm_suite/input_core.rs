use super::kit::*;

pub fn virtual_keyboard() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0)], 1_000_000)?;
    let events = stream.read_from(3, &[device.id])?;
    check(
        events
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 1),
        "core omitted virtual A press",
    )?;
    check(
        events
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 0),
        "core omitted virtual A release",
    )?;
    let press = events
        .iter()
        .find(|e| e.kind == 1 && e.code == 30 && e.value == 1)
        .unwrap()
        .serial;
    let release = events
        .iter()
        .find(|e| e.kind == 1 && e.code == 30 && e.value == 0)
        .unwrap()
        .serial;
    c.await_event("virtual A press through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30 && e.serial == press
    })?;
    c.await_event("virtual A release through bwm", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30 && e.serial == release
    })?;
    Ok(())
}
pub fn core_serials() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut a = Virtual::create()?;
    let mut b = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    b.batch(&[(1, 48, 1), (1, 48, 0)], 200)?;
    let events = stream.read_from(6, &[a.id, b.id])?;
    check(
        events
            .windows(2)
            .all(|v| v[0].serial > 0 && v[0].serial < v[1].serial),
        "core serials repeat or decrease across devices",
    )?;
    check(
        events
            .iter()
            .filter(|e| e.kind == 1)
            .all(|e| e.device != 0 && e.flags == 0),
        "core omitted device identity or set reserved flags",
    )?;
    c.await_event("first device A routed through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.await_event("second device B routed through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    Ok(())
}
pub fn core_timestamps() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1), (1, 30, 0)], 123_456_789)?;
    check(
        stream
            .read_from(3, &[a.id])?
            .iter()
            .all(|e| e.time == 123_456_789),
        "time=injector was replaced with a real clock",
    )?;
    check(
        a.batch(&[(1, 48, 1)], 123_456_788)
            .err()
            .is_some_and(|e| e.starts_with("uinput batch errno 22:")),
        "core accepted decreasing injector time",
    )?;
    c.await_event("injector-time A through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30 && e.time_ns == 123_456_789
    })?;
    Ok(())
}
pub fn core_modifiers() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 29, 1), (1, 30, 1), (1, 30, 0), (1, 29, 0)], 100)?;
    let e = c.await_event("virtual Ctrl+A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    check(
        e.modifiers & abi::modifier::CTRL != 0 && e.modifiers & abi::modifier::SUPER == 0,
        "Ctrl was lost or conflated with Super",
    )
}
pub fn core_text() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    let text = "héλ🙂".as_bytes();
    a.text(text, 100)?;
    let e = c.await_event("whole virtual EV_TEXT through bwm", |e| {
        e.kind == kind::TEXT
    })?;
    check(
        e.text_len as usize == text.len() && &e.text[..text.len()] == text,
        "EV_TEXT was split, translated or truncated",
    )?;
    check(
        a.text(&[0xff], 200)
            .err()
            .is_some_and(|e| e.starts_with("EV_TEXT batch errno 22:")),
        "invalid UTF-8 EV_TEXT was accepted",
    )
}
pub fn core_removal() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1)], 100)?;
    c.await_event("held virtual A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    drop(a);
    c.await_event("release on virtual device removal", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    Ok(())
}
pub fn core_ledger() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0), (3, 0, 0), (3, 1, 0)], 100)?;
    let events = stream.read_from(5, &[device.id])?;
    c.await_event("ledger script key reaches actual client", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.await_event("ledger script release reaches actual client", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    let serials = events.iter().map(|e| e.serial).collect::<Vec<_>>();
    until(
        2000,
        "exactly one terminal disposition per injected serial",
        || {
            let text = std::fs::read_to_string("/proc/breenix/input-ledger")
                .map_err(|e| format!("input dispositions ledger is absent: {e}"))?;
            for serial in &serials {
                let rows = text
                    .lines()
                    .map(|s| s.split_whitespace().collect::<Vec<_>>())
                    .filter(|v| v.first().and_then(|s| s.parse::<u64>().ok()) == Some(*serial))
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    return Ok(false);
                }
                if rows.len() != 1 {
                    return Err(format!(
                        "input serial {serial} has {} dispositions",
                        rows.len()
                    ));
                }
                let row = &rows[0];
                if row.len() < 3 {
                    return Err("malformed disposition row".into());
                }
                if ![
                    "wm-handled",
                    "delivered",
                    "rejected",
                    "client-rejected",
                    "lost",
                ]
                .contains(&row[1])
                {
                    return Err("unknown terminal disposition".into());
                }
                if row[1] == "lost" {
                    return Err(format!(
                        "input serial {serial} was lost in a non-overflow script"
                    ));
                }
                if row[1] == "delivered" {
                    let id = row[2].parse::<u64>().map_err(|e| e.to_string())?;
                    if id != c.id {
                        return Err(format!("serial {serial} was delivered to the wrong client"));
                    }
                    let (_, meta) = window::read_meta(c.id).map_err(|e| e.to_string())?;
                    if meta.processed_serial < *serial {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        },
    )?;
    Ok(())
}
pub fn core_capabilities() -> CaseResult {
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    c.await_event("positive declared key before capability refusal", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    check(
        device
            .batch(&[(1, 999, 1)], 200)
            .err()
            .is_some_and(|e| e.starts_with("uinput batch errno 22:")),
        "undeclared capability did not return EINVAL",
    )?;
    device.batch(&[(1, 48, 1), (1, 48, 0)], 300)?;
    c.await_event("valid key still routes after capability refusal", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    Ok(())
}
pub fn virtual_pointer() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (d, mut c) = one()?;
    let w = c.info()?;
    let x = w.x + 40;
    let y = w.y + 50;
    device.batch(
        &[
            (
                3,
                0,
                ((x as u64 * 32767 + (d.width - 2) as u64) / (d.width - 1) as u64) as i32,
            ),
            (
                3,
                1,
                ((y as u64 * 32767 + (d.height - 2) as u64) / (d.height - 1) as u64) as i32,
            ),
            (1, 0x110, 1),
        ],
        100,
    )?;
    check(
        stream
            .read_from(4, &[device.id])?
            .iter()
            .any(|e| e.kind == 3),
        "virtual pointer axes absent from core stream",
    )?;
    c.await_event("virtual pointer maps exactly into client content", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1 && (e.x, e.y) == (40, 50)
    })?;
    device.batch(&[(1, 0x110, 0)], 200)?;
    c.await_event("virtual pointer release", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    Ok(())
}

pub fn uinput_create() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut keyboard = Virtual::create()?;
    let mut pointer = Virtual::create()?;
    check(keyboard.id != pointer.id, "uinput reused a live device id")?;
    keyboard.batch(&[(1, 30, 1), (1, 30, 0)], 10)?;
    pointer.batch(
        &[(3, 0, 100), (3, 1, 200), (1, 0x110, 1), (1, 0x110, 0)],
        20,
    )?;
    let events = stream.read_from(8, &[keyboard.id, pointer.id])?;
    check(
        events
            .iter()
            .filter(|e| e.kind == 1 && e.code == 30)
            .count()
            == 2
            && events
                .iter()
                .filter(|e| e.kind == 1 && e.code == 0x110)
                .count()
                == 2,
        "created devices did not publish declared keyboard and pointer events",
    )
}
pub fn uinput_nonroot() -> CaseResult {
    let _root = Virtual::create()?;
    child_check(|| {
        let r = unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::SETUID, 1000) }
            as i64;
        if r != 0 {
            return 1;
        }
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
        {
            Err(e) if e.raw_os_error() == Some(13) => 0,
            _ => 2,
        }
    })
}
pub fn abi_layout() -> CaseResult {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let mut stream =
        std::fs::File::open("/dev/input/events").map_err(|e| format!("input ABI stream: {e}"))?;
    let mut device = Virtual::create()?;
    let stamp = 0x0102030405060708u64;
    device.batch(&[(1, 30, 1), (1, 30, 0)], stamp)?;
    let mut records = Vec::new();
    until(
        1000,
        "kernel emits three literal 32-byte ABI records",
        || {
            let mut poll = [libbreenix::io::PollFd {
                fd: stream.as_raw_fd(),
                events: 1,
                revents: 0,
            }];
            libbreenix::io::poll(&mut poll, 10).map_err(|e| e.to_string())?;
            if poll[0].revents & 1 == 0 {
                return Ok(false);
            }
            let mut bytes = [0; 32768];
            let n = stream.read(&mut bytes).map_err(|e| e.to_string())?;
            if n == 0 || n % 32 != 0 {
                return Err("kernel returned a partial ABI record".into());
            }
            for raw in bytes[..n].chunks_exact(32) {
                if u32::from_le_bytes(raw[16..20].try_into().unwrap()) == device.id {
                    records.push(raw.to_vec());
                }
            }
            Ok(records.len() >= 3)
        },
    )?;
    check(
        records.len() == 3,
        "key batch did not emit exactly three ABI records",
    )?;
    let mut previous = 0;
    for (raw, (kind, code, value)) in
        records
            .iter()
            .zip([(1u16, 30u16, 1i32), (1, 30, 0), (0, 0, 0)])
    {
        let serial = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        check(
            serial > previous,
            "serial at literal ABI offset 0 did not increase",
        )?;
        previous = serial;
        check(
            raw[8..16] == stamp.to_le_bytes()
                && raw[16..20] == device.id.to_le_bytes()
                && raw[20..22] == kind.to_le_bytes()
                && raw[22..24] == code.to_le_bytes()
                && raw[24..28] == value.to_le_bytes()
                && raw[28..32] == [0; 4],
            "kernel wire bytes differ from the literal size and field offsets",
        )?;
    }
    Ok(())
}

pub fn key_press_release() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    device.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    let es = stream.read_from(3, &[device.id])?;
    check(
        es.iter()
            .map(|e| (e.kind, e.code, e.value))
            .collect::<Vec<_>>()
            == [(1, 30, 1), (1, 30, 0), (0, 0, 0)],
        "key batch differs from literal A press, release, SYN_REPORT",
    )
}
pub fn modifiers_derived() -> CaseResult {
    for (code, mask) in [(42, 1), (29, 2), (56, 4), (125, 8)] {
        let mut stream = Stream::open()?;
        let mut a = Virtual::create()?;
        let mut b = Virtual::create()?;
        for d in [&mut a, &mut b] {
            d.batch(&[(1, code, 1), (1, 30, 1), (1, 30, 0), (1, code, 0)], 100)?;
        }
        let es = stream.read_from(10, &[a.id, b.id])?;
        for id in [a.id, b.id] {
            let keys: Vec<_> = es
                .iter()
                .filter(|e| e.device == id && e.kind == 1)
                .collect();
            check(
                keys.len() == 4
                    && keys[0].flags == mask
                    && keys[1].flags == mask
                    && keys[2].flags == mask
                    && keys[3].flags == 0,
                "modifier derivation differs between devices or release retains modifier",
            )?;
        }
    }
    Ok(())
}
pub fn pointer_absolute() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (d, mut c) = one()?;
    let w = c.info()?;
    let x = w.x + 20;
    let y = w.y + 30;
    let ax = ((x as u64 * 32767 + d.width as u64 - 2) / (d.width as u64 - 1)) as i32;
    let ay = ((y as u64 * 32767 + d.height as u64 - 2) / (d.height as u64 - 1)) as i32;
    v.batch(&[(3, 0, ax), (3, 1, ay)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    check(
        es[0].value == ax && es[1].value == ay,
        "absolute source coordinates were changed",
    )?;
    c.await_event("absolute mapping at literal local coordinates", |e| {
        e.kind == kind::POINTER_MOVE && e.x == 20 && e.y == 30
    })?;
    // W29: the same raw coordinate means different pixels for different
    // declared ranges. Query through the public reader, then judge bwm at the
    // independently computed physical point; source events must stay raw.
    use libbreenix::input::{self as input, Batch, Capability, Injector, Reader};
    let reader = Reader::open(true)?;
    let caps = [
        Capability { kind: input::kind::ABS, code: 0, minimum: -1000, maximum: 5000 },
        Capability { kind: input::kind::ABS, code: 1, minimum: 100, maximum: 10100 },
    ];
    let device = Injector::create(&caps, true)?;
    let mut queried = [Capability::default(); 2];
    let info = reader.device_info(device.id(), &mut queried)?;
    check(info.kind == input_abi::device_kind::VIRTUAL && info.generation != 0
        && info.count == 2 && queried == caps, "W29 query changed virtual capabilities or ranges")?;
    let x = w.x + 70;
    let y = w.y + 80;
    let raw_x = -1000 + ((x as i64 * 6000 + d.width as i64 - 2) / (d.width as i64 - 1)) as i32;
    let raw_y = 100 + ((y as i64 * 10000 + d.height as i64 - 2) / (d.height as i64 - 1)) as i32;
    let mut storage = [0; 128];
    let mut batch = Batch::new(&mut storage);
    batch.event(3, 0, raw_x, 200)?.event(3, 1, raw_y, 200)?.finish(200)?;
    device.inject(&batch)?;
    let source = stream.read_from(3, &[device.id()])?;
    check(source[0].value == raw_x && source[1].value == raw_y,
        "W29 non-default range source values were normalized in the core")?;
    c.await_event("W29 non-default absolute ranges mapped by bwm", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (70, 80)
    })?;
    let id = device.id();
    drop(device);
    until(1000, "W29 removed device query returns ENOENT", || {
        match reader.device_info(id, &mut queried) {
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Ok(true),
            Ok(_) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    })?;
    Ok(())
}
pub fn pointer_relative() -> CaseResult {
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    d.focus(&mut c)?;
    d.pointer(w.x + 40, w.y + 40, 0, 0)?;
    // W30: host transport acceptance is not delivery. Establish the physical
    // starting position before the in-guest relative device can move it.
    c.await_event("relative pointer starting position", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (40, 40)
    })?;
    c.clear()?;
    v.batch(&[(2, 0, 7), (2, 1, -9)], 100)?;
    c.await_event("relative axes accumulate", |e| {
        e.kind == kind::POINTER_MOVE && e.x == 47 && e.y == 31
    })?;
    v.batch(&[(2, 0, 32767), (2, 1, 32767)], 200)?;
    pause();
    let screen = Screen::read()?;
    check(
        screen.pixel(d.width as i32 - 1, d.height as i32 - 1)? != 0,
        "relative pointer clamp did not produce an observable edge pixel",
    )
}
pub fn buttons() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    for code in [0x110, 0x111, 0x112] {
        v.batch(&[(1, code, 1), (1, code, 0)], code as u64)?;
        let es = stream.read_from(3, &[v.id])?;
        check(
            es.iter()
                .map(|e| (e.kind, e.code, e.value))
                .collect::<Vec<_>>()
                == [(1, code, 1), (1, code, 0), (0, 0, 0)],
            "core changed button identity, edge or order",
        )?;
    }
    Ok(())
}
pub fn wheel() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    v.batch(&[(2, 8, -2), (2, 6, 3)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    check(
        es.iter()
            .map(|e| (e.kind, e.code, e.value))
            .collect::<Vec<_>>()
            == [(2, 8, -2), (2, 6, 3), (0, 0, 0)],
        "wheel axes or signs changed",
    )
}
pub fn syn_batches() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    check(
        es.len() == 3 && es[2].kind == 0 && es[2].code == 0,
        "reader split or merged the single SYN batch",
    )
}
pub fn time_real() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create_timed(0)?;
    let before = now();
    v.batch(&[(1, 30, 1), (1, 30, 0)], 0)?;
    let es = stream.read_from(3, &[v.id])?;
    let after = now();
    check(
        es.iter().all(|e| e.time >= before && e.time <= after),
        "kernel-timed input timestamp lies outside CLOCK_MONOTONIC capture interval",
    )
}
pub fn two_readers() -> CaseResult {
    let mut a = Stream::open()?;
    let mut b = Stream::open()?;
    let mut v = Virtual::create()?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    check(
        a.read_from(3, &[v.id])? == b.read_from(3, &[v.id])?,
        "two readers saw different source batches",
    )
}
pub fn syn_dropped() -> CaseResult {
    let _claim = InputClaim::new()?;
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    stream.grab(&[v.id])?;
    v.batch(&[(1, 30, 1)], 1)?;
    let first = stream.read_from(2, &[v.id])?[0].serial;
    for n in 2..=1101 {
        v.batch(&[(1, 30, (n & 1) as i32)], n)?;
    }
    let es = stream.read_from(1, &[v.id])?;
    let dropped = es.iter().find(|e| e.kind == 0 && e.code == 3)
        .ok_or("overflow reader omitted SYN_DROPPED")?;
    for serial in first + 2..=dropped.serial {
        check(ledger_has(serial, Some(("lost", "overflow")))?,
            "overflow omitted this holder's dropped serial disposition")?;
    }
    check(!ledger_has(first, None)?, "overflow changed an already-read pending serial")?;
    drop(stream);
    Ok(())
}
pub fn poll() -> CaseResult {
    let stream = Stream::open()?;
    let mut v = Virtual::create()?;
    check(!stream.poll(0)?, "new reader spuriously ready")?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    check(stream.poll(1000)?, "queued input did not report POLLIN")
}
pub fn grab() -> CaseResult {
    let mut v = Virtual::create()?;
    let (_d, mut c) = one()?;
    let mut stream = Stream::open()?;
    check(
        stream.grab(&[]).is_err(),
        "second reader stole bwm's compositor grab",
    )?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    let serial = es[0].serial;
    c.await_event("grab routes ordinary key to compositor client", |e| {
        e.kind == kind::KEY_PRESS && e.serial == serial
    })?;
    disposition(serial, "delivered", None)
}
pub fn grab_select() -> CaseResult {
    let _claim = InputClaim::new()?;
    let mut a = Virtual::create()?;
    let mut b = Virtual::create()?;
    let mut stream = Stream::open()?;
    stream.grab(&[a.id])?;
    a.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    b.batch(&[(1, 48, 1), (1, 48, 0)], 100)?;
    let es = stream.read_from(3, &[a.id])?;
    check(
        es.iter().all(|e| e.device == a.id),
        "grab selection admitted foreign device",
    )?;
    let rows = ledger()?;
    check(
        rows.iter()
            .any(|r| r.get(3).is_some_and(|s| s == "not-selected")),
        "excluded device has no not-selected disposition",
    )
}
pub fn grab_drain() -> CaseResult {
    let _claim = InputClaim::new()?;
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    v.batch(&[(1, 42, 1)], 100)?;
    stream.grab(&[v.id])?;
    check(!stream.poll(0)?, "taking grab did not drain queued input")?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 200)?;
    let es = stream.read_from(3, &[v.id])?;
    check(
        es.iter().filter(|e| e.kind == 1).all(|e| e.flags == 0),
        "taking grab retained held Shift state",
    )
}
pub fn text_invalid() -> CaseResult {
    let mut v = Virtual::create()?;
    let mut stream = Stream::open()?;
    for text in [&[0xff][..], &vec![b'x'; 257][..]] {
        check(
            v.text(text, 100)
                .err()
                .is_some_and(|e| e.starts_with("EV_TEXT batch errno 22:")),
            "invalid UTF-8 or oversize text was not atomically refused with EINVAL",
        )?;
    }
    let mut truncated = vec![0; 33];
    truncated[20..22].copy_from_slice(&6u16.to_le_bytes());
    truncated[24..28].copy_from_slice(&2i32.to_le_bytes());
    truncated[32] = b'a';
    check(
        v.raw(&truncated)
            .err()
            .is_some_and(|e| e.raw_os_error() == Some(22)),
        "truncated text accepted",
    )?;
    check(!stream.poll(0)?, "refused text published partial input")?;
    v.text(b"valid", 101)?;
    let es = stream.read_from(2, &[v.id])?;
    check(
        es[0].payload == b"valid",
        "refusal damaged subsequent valid text",
    )
}
pub fn malformed_batch() -> CaseResult {
    let mut v = Virtual::create()?;
    let stream = Stream::open()?;
    for bytes in [vec![0; 31], {
        let mut b = vec![0; 64];
        b[20..22].copy_from_slice(&65535u16.to_le_bytes());
        b
    }] {
        check(
            v.raw(&bytes)
                .err()
                .is_some_and(|e| e.raw_os_error() == Some(22)),
            "malformed input batch did not return EINVAL",
        )?;
        check(!stream.poll(0)?, "malformed batch published a prefix")?;
    }
    Ok(())
}
pub fn console_keymap() -> CaseResult {
    console_bytes(&[(30, 1), (30, 0)], b"a")?;
    console_bytes(&[(30, 1), (30, 2), (30, 0)], b"aa")
}
pub fn console_special_keys() -> CaseResult {
    console_bytes(
        &[
            (103, 1),
            (103, 0),
            (102, 1),
            (102, 0),
            (107, 1),
            (107, 0),
            (111, 1),
            (111, 0),
            (104, 1),
            (104, 0),
            (109, 1),
            (109, 0),
        ],
        b"\x1b[A\x1b[H\x1b[F\x1b[3~\x1b[5~\x1b[6~",
    )
}
pub fn console_modifiers() -> CaseResult {
    console_bytes(&[(29, 1), (46, 1), (46, 0), (29, 0)], &[3])
}
pub fn ledger_gap() -> CaseResult {
    let _claim = InputClaim::new()?;
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    stream.grab(&[v.id])?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    let first = es[0].serial;
    let rows = ledger()?;
    check(
        !rows
            .iter()
            .any(|r| r.first().is_some_and(|s| s == &first.to_string())),
        "held undecided serial already has a terminal disposition",
    )?;
    stream.dispose(es[1].serial, 0, input_abi::disposition::WM_HANDLED, 0)?;
    let rows = ledger()?;
    check(
        rows.iter()
            .any(|r| r.first().is_some_and(|s| s == &es[1].serial.to_string())),
        "later disposition was not recorded",
    )?;
    let gap = (first..=es[1].serial).find(|serial| {
        !rows
            .iter()
            .any(|r| r.first().is_some_and(|s| s == &serial.to_string()))
    });
    check(
        gap == Some(first),
        "later disposition masked earlier missing serial",
    )
}
pub fn console_after_compositor() -> CaseResult {
    let (_d, mut c) = one()?;
    let mut v = Virtual::create()?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    c.await_event("positive compositor delivery", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    drop(c);
    drop(_d);
    console_bytes(&[(30, 1), (30, 0)], b"a")
}

/// The Phase A public library exercises whole payloads and transactional refusal
/// without depending on lane 3's compositor routing.
pub fn library_batches() -> CaseResult {
    use libbreenix::input::{self as input, Batch, Capability, Injector, Reader};
    let reader = Reader::open(true)?;
    let device = Injector::create(&[
        Capability { kind: input::kind::KEY, code: 30, minimum: 0, maximum: 1 },
        Capability { kind: input::kind::TEXT, code: 0, minimum: 1, maximum: 256 },
        Capability { kind: input::kind::KEY, code: input::key::BUTTON_LEFT, minimum: 0, maximum: 1 },
    ], true)?;
    let mut storage = [0u8; 512];
    let mut batch = Batch::new(&mut storage);
    batch.event(1, 30, 1, 1234)?.text("hé\0λ🙂", 1234)?
        .event(1, 30, 0, 1234)?.finish(1234)?;
    device.inject(&batch)?;
    let mut short = [0u8; 32];
    let fault = unsafe { libbreenix::syscall::raw::syscall3(libbreenix::syscall::nr::READ, reader.fd().raw(), 1, 1024) } as i64;
    check(fault == -14, "bad read buffer did not return EFAULT")?;
    check(matches!(reader.read(&mut short), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINVAL))),
        "short read did not return EINVAL for a variable-payload batch")?;
    let mut bytes = [0u8; 1024];
    let events = reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    check(events.len() == 4 && events[1].payload == "hé\0λ🙂".as_bytes()
        && events.iter().all(|r| r.event.device == device.id() && r.event.time_ns == 1234)
        && events.windows(2).all(|r| r[0].event.serial < r[1].event.serial),
        "library reader changed text bytes, injector time, identity or serial order")?;
    let last = events[3].event.serial;
    batch.clear();
    let bad = &mut batch;
    bad.event(1, 30, 1, 1233)?.finish(1233)?;
    check(matches!(device.inject(&bad), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINVAL))),
        "decreasing injector time did not return EINVAL")?;
    bad.clear();
    bad.event(1, 48, 1, 1235)?.finish(1235)?;
    check(matches!(device.inject(&bad), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINVAL))),
        "undeclared capability did not return EINVAL")?;
    check(matches!(reader.read(&mut bytes), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN))),
        "refused batch published a prefix")?;
    let good = bad;
    good.clear();
    good.event(1, 30, 1, 1235)?.event(1, 30, 0, 1235)?.finish(1235)?;
    device.inject(&good)?;
    let events = reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    check(events.len() == 3 && events[0].event.serial == last + 1,
        "rejected write consumed serials or damaged subsequent batch")?;
    let second = Reader::open(true)?;
    good.clear();
    good.event(1, 30, 1, 1236)?.event(1, input::key::BUTTON_LEFT, 1, 1236)?.finish(1236)?;
    device.inject(&good)?;
    let presses = reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    let last = presses.last().unwrap().event.serial;
    let id = device.id();
    let mut other = [0u8; 1024];
    second.read(&mut other)?;
    drop(device);
    until(1000, "device close publishes key/button releases and closing SYN", || {
        match reader.read(&mut bytes) {
            Ok(records) => {
                let records = records.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
                check(records.len() == 3 && records.iter().all(|r| r.event.device == id)
                    && records[0].event.code == 30 && records[0].event.value == 0
                    && records[1].event.code == input::key::BUTTON_LEFT && records[1].event.value == 0
                    && records[2].event.kind == 0 && records[2].event.code == 0
                    && records.iter().enumerate().all(|(i, r)| r.event.serial == last + i as u64 + 1),
                    "device close omitted, reordered or misidentified its release batch").map_err(|e| format!("{e:?}"))?;
                let copy = second.read(&mut other).map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
                check(records == copy, "device close releases differ between readers")
                    .map_err(|e| format!("{e:?}"))?;
                Ok(true)
            }
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    })?;
    Ok(())
}

fn ledger_has(serial: u64, expected: Option<(&str, &str)>) -> Result<bool, String> {
    let text = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
    Ok(text.lines().any(|line| {
        let mut fields = line.split_whitespace();
        if fields.next().and_then(|s| s.parse::<u64>().ok()) != Some(serial) { return false; }
        expected.is_none_or(|(outcome, reason)| {
            let actual = fields.next();
            fields.next();
            actual == Some(outcome) && fields.next() == Some(reason)
        })
    }))
}

pub fn reader_lifetime() -> CaseResult {
    use libbreenix::{input::{self as input, Batch, Capability, Injector, Reader}, io};
    // Discard only input that predates this case, before its first grab.
    libbreenix::termios::tcflush(libbreenix::types::Fd::STDIN, libbreenix::termios::TCIFLUSH)?;
    let _claim = InputClaim::new()?;
    let device = Injector::create(&[Capability { kind: 1, code: 30, minimum: 0, maximum: 1 }], true)?;
    let reader = Reader::open(true)?;
    let contender = Reader::open(true)?;
    reader.grab(&[device.id()])?;
    let duplicate = io::dup(reader.fd())?;
    // An unrelated reader and one duplicate may close without ending custody.
    drop(Reader::open(true)?);
    let mut storage = [0u8; 512];
    let mut batch = Batch::new(&mut storage); batch.event(1, 30, 1, 100)?.finish(100)?;
    device.inject(&batch)?;
    let mut bytes = [0; 256];
    let events = reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    let press = events[0].event.serial;
    let syn = events[1].event.serial;
    reader.dispose(&input::Disposition { serial: syn, disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
    drop(reader);
    check(matches!(contender.grab(&[device.id()]), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EBUSY))),
        "closing one duplicated fd released the grab")?;
    // The pending press ends lost only at the last reference, even though SYN
    // already has a terminal outcome.
    check(!ledger_has(press, None)?, "later SYN hid the pending press")?;
    io::close(duplicate)?;
    contender.grab(&[device.id()])?;
    check(ledger_has(press, Some(("lost", "compositor-died")))?,
        "last reader close omitted the pending serial's death disposition")?;
    contender.ungrab()?;
    drop(_claim);
    let mut observer = Stream::open()?;
    let mut child = || {
        let run = || -> Result<(), libbreenix::error::Error> {
            window::claim_compositor()?;
            let owned = Reader::open(true)?;
            owned.grab(&[device.id()])?;
            let mut data = [0; 128];
            let mut events = Batch::new(&mut data);
            events.event(1, 30, 1, 200)?.finish(200)?;
            device.inject(&events)?;
            // Kernel process teardown, rather than a Rust destructor, closes
            // this final reader reference and drops the compositor claim.
            core::mem::forget(owned);
            Ok(())
        };
        match run() { Ok(()) => 0, Err(libbreenix::error::Error::Os(errno)) => errno as i32 }
    };
    let pid = breenix_suite::suite::spawn(
        &[(device.fd(), device.fd())], breenix_suite::suite::Run::Call(&mut child),
    )?;
    let mut status = 0;
    libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0)?;
    check(status == 0, &format!("reader-exit helper failed with wait status {status}"))?;
    let dead = observer.read_from(2, &[device.id()])?;
    let _new_claim = InputClaim::new()?;
    contender.grab(&[device.id()])?;
    check(ledger_has(dead[0].serial, Some(("lost", "compositor-died")))?,
        "process teardown kept the grab or lost the pending press's death disposition")?;

    // Exercise W2 against an actual window ring under this isolated claimant.
    struct LocalWindow(u64);
    impl Drop for LocalWindow {
        fn drop(&mut self) { let _ = window::destroy(self.0); }
    }
    let target = LocalWindow(window::create(64, 64, 2, 0)?.id);
    window::register(target.0, b"input fd lifetime")?;
    let mut delivered = [abi::WindowEvent::ZERO; 8];
    window::read_events(target.0, &mut delivered, false, 0)?;
    batch.clear();
    batch.event(1, 30, 1, 300)?.finish(300)?;
    device.inject(&batch)?;
    let events = contender.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    contender.dispose(&input::Disposition { serial: events[0].event.serial, window: target.0,
        disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
    contender.dispose(&input::Disposition { serial: events[1].event.serial,
        disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
    drop(contender);
    time::sleep_ms(100)?;
    check(window::read_events(target.0, &mut delivered, false, 0)? == 0,
        "a WM-handled press manufactured a client release")?;

    let recipient_reader = Reader::open(true)?;
    recipient_reader.grab(&[device.id()])?;
    batch.clear();
    batch.event(1, 30, 1, 400)?.finish(400)?;
    device.inject(&batch)?;
    let events = recipient_reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
    let source = events[0].event;
    let mut press_event = abi::WindowEvent::ZERO;
    press_event.kind = kind::KEY_PRESS;
    press_event.keycode = 30;
    press_event.serial = source.serial;
    press_event.time_ns = source.time_ns;
    check(window::write_events(target.0, &[press_event])? == 1,
        "the positive press did not enter its actual window ring")?;
    check(window::read_events(target.0, &mut delivered, false, 0)? == 1 && delivered[0] == press_event,
        "the original recipient did not receive its press")?;
    recipient_reader.dispose(&input::Disposition { serial: source.serial, window: target.0,
        disposition: input::disposition::DELIVERED, ..input::Disposition::default() })?;
    recipient_reader.dispose(&input::Disposition { serial: events[1].event.serial,
        disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
    {
        use libbreenix::{termios, types::Fd};
        struct Restore(termios::Termios);
        impl Drop for Restore {
            fn drop(&mut self) { let _ = termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &self.0); }
        }
        let mut saved = termios::Termios::default();
        termios::tcgetattr(Fd::STDIN, &mut saved)?;
        let _restore = Restore(saved);
        let mut raw = saved;
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &raw)?;
        let mut poll = [io::PollFd { fd: 0, events: 1, revents: 0 }];
        io::poll(&mut poll, 0)?;
        check(poll[0].revents & 1 == 0, "console has unexpected input before grabbed repeat")?;
        batch.clear(); batch.event(1, 30, 2, 400)?.finish(400)?;
        device.inject(&batch)?;
        let repeated = recipient_reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
        check(repeated.len() == 1 && repeated[0].event.kind == 0,
            "device repeat reached grabbed reader")?;
        recipient_reader.dispose(&input::Disposition { serial: repeated[0].event.serial,
            disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
        io::poll(&mut poll, 50)?;
        check(poll[0].revents & 1 == 0, "grabbed device repeat leaked into console")?;
    }
    let idle = Reader::open(true)?;
    let mut last_serial = 0;
    for _ in 0..1100 {
        batch.clear();
        batch.finish(400)?;
        device.inject(&batch)?;
        let seen = recipient_reader.read(&mut bytes)?.collect::<Result<Vec<_>, _>>()?;
        for r in seen {
            last_serial = r.event.serial;
            recipient_reader.dispose(&input::Disposition { serial: last_serial,
                disposition: input::disposition::WM_HANDLED, ..input::Disposition::default() })?;
        }
    }
    check(ledger_has(source.serial, Some(("delivered", "none")))?,
        "observer overflow overwrote the delivered press")?;
    drop(idle);
    // Leave a physical release queued when custody ends. The recipient still
    // needs its release even though the device is no longer physically held.
    batch.clear();
    batch.event(1, 30, 0, 401)?.finish(401)?;
    device.inject(&batch)?;
    let queued_release = last_serial + 1;
    drop(recipient_reader);
    let mut release_id = 0;
    until(1000, "last reader close releases the original recipient", || {
        let count = window::read_events(target.0, &mut delivered, false, 0).map_err(|e| e.to_string())?;
        for event in &delivered[..count] {
            check(event.kind == kind::KEY_RELEASE && event.keycode == 30 && event.serial != source.serial,
                "final close produced the wrong release kind or reused the source id").map_err(|e| format!("{e:?}"))?;
            let mut tag = abi::EventTag { id: target.0, serial: event.serial, ..abi::EventTag::default() };
            window::call(abi::op::EVENT_TAG, &mut tag).map_err(|e| e.to_string())?;
            check(tag.source_serial == source.serial && tag.derived != 0,
                "the recipient release lost its source/derived identity").map_err(|e| format!("{e:?}"))?;
            release_id = event.serial;
        }
        Ok(release_id != 0)
    })?;
    check(!ledger_has(release_id, None)?, "a derived release consumed a source ledger row")?;
    check(ledger_has(queued_release, Some(("lost", "compositor-died")))?,
        "queued release did not retain its death outcome")?;
    batch.clear();
    let mut observer = Stream::open()?;
    batch.event(1, 30, 1, 402)?.event(1, 30, 0, 402)?.finish(402)?;
    device.inject(&batch)?;
    let after = observer.read_from(1, &[device.id()])?;
    check(after.iter().any(|e| e.serial == queued_release + 2 && e.kind == 1 && e.value == 1),
        "derived release consumed a source serial")?;
    let mut meta = abi::Meta::ZERO;
    meta.processed_serial = source.serial;
    window::commit_no_change(target.0, Some(&meta))?;
    check(window::read_meta(target.0)?.1.processed_serial == source.serial,
        "the original recipient did not publish its source acknowledgment")?;
    Ok(())
}
