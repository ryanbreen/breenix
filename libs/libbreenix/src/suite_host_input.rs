//! Native transports complete at guest input delivery. Open the observer before
//! emitting the request, so its queue contains no retained events from a case.
use libbreenix::{graphics, input, io};
use input::{axis, kind, syn, Event, Reader};

fn wait(reader: &Reader, mut delivered: impl FnMut(Event) -> Result<Option<u64>, String>) -> Result<u64, String> {
    let mut bytes = [0u8; 8192];
    loop {
        if super::case_ms_left() < 200 { return Err("native host input: case deadline".into()); }
        let mut ready = [io::PollFd { fd: reader.fd().raw() as i32, events: 1, revents: 0 }];
        io::poll(&mut ready, 5).map_err(|e| e.to_string())?;
        if ready[0].revents == 0 { continue; }
        let records = reader.read(&mut bytes).map_err(|e| e.to_string())?;
        for record in records {
            let event = record.map_err(|e| e.to_string())?.event;
            if event.kind == kind::SYN && event.code == syn::DROPPED {
                return Err("native host input observer overflowed".into());
            }
            if let Some(ns) = delivered(event)? {
                if ns != 0 && super::monotonic_ns().is_some_and(|now| i128::from(ns) <= now) { return Ok(ns); }
                return Err("native host input has an invalid timestamp".into());
            }
        }
    }
}

fn keycode(name: &str) -> Result<u16, String> {
    for (row, first) in [("1234567890", 2), ("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)] {
        if name.len() == 1 {
            if let Some(index) = row.find(name) { return Ok(first + index as u16); }
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|s| s.parse::<u16>().ok()) {
        if (1..=10).contains(&n) { return Ok(58 + n); }
        if (11..=12).contains(&n) { return Ok(76 + n); }
    }
    Ok(match name {
        "esc" => 1, "backspace" => 14, "tab" => 15, "enter" => 28,
        "space" => 57, "capslock" => 58, "home" => 102, "up" => 103,
        "pageup" => 104, "left" => 105, "right" => 106, "end" => 107,
        "down" => 108, "pagedown" => 109, "insert" => 110, "delete" => 111,
        _ => return Err(format!("no input code for host key {name}")),
    })
}

pub(super) fn keys(chords: &str, request: impl FnOnce() -> Result<u64, String>) -> Result<u64, String> {
    let expected: Vec<_> = chords.split(',').map(|c| keycode(c.rsplit('-').next().unwrap_or("")))
        .collect::<Result<_, _>>()?;
    let reader = Reader::open(true).map_err(|e| e.to_string())?;
    request()?;
    let mut index = 0;
    let mut pressed = None;
    wait(&reader, |e| {
        if e.kind == kind::KEY && e.code == expected[index] {
            if e.value == 1 { pressed = Some(e.device); }
            if e.value == 0 && pressed == Some(e.device) {
                pressed = None;
                index += 1;
                if index == expected.len() { return Ok(Some(e.time_ns)); }
            }
        }
        Ok(None)
    })
}

pub(super) fn pointer(x: u32, y: u32, width: u32, height: u32, buttons: u32, wheel: i32,
    request: impl FnOnce() -> Result<u64, String>) -> Result<u64, String>
{
    let reader = Reader::open(true).map_err(|e| e.to_string())?;
    request()?;
    let mut motion = std::collections::BTreeMap::new();
    wait(&reader, |e| {
        let batch = motion.entry(e.device).or_insert((None, None, 0i64));
        if e.kind == kind::ABS && (e.code == axis::X || e.code == axis::Y) {
            let mut caps = [input::Capability::default(); 512];
            let count = reader.capabilities(e.device, &mut caps).map_err(|e| e.to_string())?;
            let cap = caps.get(..count).and_then(|caps| caps.iter().find(|c| c.kind == kind::ABS && c.code == e.code))
                .ok_or("native pointer axis has no capability")?;
            let range = i64::from(cap.maximum) - i64::from(cap.minimum);
            if range <= 0 { return Err("native pointer axis has no range".into()); }
            let extent = if e.code == axis::X { width } else { height };
            let position = ((i64::from(e.value) - i64::from(cap.minimum)).clamp(0, range)
                * i64::from(extent.saturating_sub(1)) / range) as u32;
            if e.code == axis::X { batch.0 = Some(position); } else { batch.1 = Some(position); }
        }
        if e.kind == kind::REL && e.code == axis::WHEEL { batch.2 += i64::from(e.value); }
        if e.kind == kind::SYN && e.code == syn::REPORT {
            let matches = batch.0 == Some(x) && batch.1 == Some(y) && batch.2 == i64::from(wheel)
                && graphics::mouse_state().map_err(|e| e.to_string())? == (x, y, buttons);
            *batch = (None, None, 0);
            if matches { return Ok(Some(e.time_ns)); }
        }
        Ok(None)
    })
}
