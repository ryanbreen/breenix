//! USB pointer measurement for Parallels and VMware (ARM64), run by
//! `./run.sh --parallels|--vmware --program /bin/usb_pointer_check --program-arg MODE`.
//!
//! The observations come from the production graphics syscall that bwm
//! reads (`mouse_state_with_scroll`). The inputs come from one of two places:
//!
//! - `host`: the launcher's host pointer path. The check prints
//!   `USB POINTER HOST seq=.. x=.. y=.. buttons=.. wheel=..` and the host-key
//!   watcher (`scripts/hostkeys.py`) sends that pointer state over VMware's VNC
//!   server, which the virtual USB device turns into HID reports. Each motion case
//!   says whether the guest moved to the requested position (absolute) or by the
//!   requested distance (relative).
//! - `replay`: for a target with no host pointer path (Parallels). For each
//!   attached pointer interface, reports are encoded from the report descriptor
//!   the device itself returned (`/proc/xhci/pointers`) and handed to the kernel's
//!   replay control, which runs them through the xHCI pointer completion path
//!   (decode, button merge, input report, compositor wake). That control exists
//!   only in a kernel built with the `usb_pointer_replay` feature.
//!
//! `both` runs `host` and then `replay`.
use libbreenix::graphics;
use libbreenix::syscall::{nr, raw::syscall3};
use std::time::{Duration, Instant};

/// Kernel replay control (`drivers::xhci::replay`).
const POINTER_REPLAY: u64 = 0xB813_0006;
const REPORT_MAX: usize = 64;

#[repr(C)]
struct ReplayRequest {
    port: u32,
    len: u32,
    report: [u8; REPORT_MAX],
}

/// One attached pointer interface from `/proc/xhci/pointers`.
struct Pointer {
    port: u32,
    interface: u32,
    descriptor: Vec<u8>,
}

/// The attached pointers, once the xHCI worker has attached them: the program
/// can start before the controller's port census ends, so wait up to 30 s.
fn pointers() -> Result<Vec<Pointer>, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let text = std::fs::read_to_string("/proc/xhci/pointers")
            .map_err(|error| format!("reading /proc/xhci/pointers: {error}"))?;
        if !text.is_empty() || Instant::now() >= deadline {
            break text;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut found = Vec::new();
    for line in text.lines() {
        println!("USB POINTER DEVICE {line}");
        let field = |name: &str| {
            line.split(' ')
                .find_map(|word| word.strip_prefix(name))
                .map(str::to_string)
        };
        let number = |name: &str| field(name).and_then(|v| v.parse::<u32>().ok());
        let hex = field("descriptor=").unwrap_or_default();
        if hex.len() % 2 != 0 {
            return Err(format!("odd descriptor hex in {line:?}"));
        }
        let descriptor = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| format!("bad descriptor hex in {line:?}"))?;
        match (number("port="), number("interface=")) {
            (Some(port), Some(interface)) => found.push(Pointer {
                port,
                interface,
                descriptor,
            }),
            _ => return Err(format!("unreadable pointer line {line:?}")),
        }
    }
    Ok(found)
}

/// One report field: where it sits in its report and its logical range.
#[derive(Clone, Copy)]
struct Field {
    id: u8,
    offset: usize,
    size: usize,
    minimum: i64,
    maximum: i64,
    relative: bool,
}

/// The pointer fields of one report descriptor (HID 1.11 6.2.2; Usage Tables
/// Generic Desktop X 0x30, Y 0x31, Wheel 0x38, Button page 9).
struct Layout {
    x: Option<Field>,
    y: Option<Field>,
    wheel: Option<Field>,
    buttons: Vec<(u8, Field)>,
    ids: bool,
    bits: [usize; 256],
}

impl Layout {
    fn parse(descriptor: &[u8]) -> Result<Layout, String> {
        #[derive(Clone, Copy, Default)]
        struct Globals {
            page: u32,
            minimum: i64,
            maximum_raw: u32,
            maximum_bytes: usize,
            size: usize,
            count: usize,
            id: u8,
        }
        let mut layout = Layout {
            x: None,
            y: None,
            wheel: None,
            buttons: Vec::new(),
            ids: false,
            bits: [0; 256],
        };
        let mut globals = Globals::default();
        let mut stack = Vec::new();
        let mut usages: Vec<u32> = Vec::new();
        let (mut usage_min, mut usage_max) = (None, None);
        let mut at = 0;
        while at < descriptor.len() {
            let prefix = descriptor[at];
            at += 1;
            if prefix == 0xfe {
                return Err("long item".into());
            }
            let bytes = [0, 1, 2, 4][(prefix & 3) as usize];
            if at + bytes > descriptor.len() {
                return Err("truncated item".into());
            }
            let mut value = 0u32;
            for i in 0..bytes {
                value |= (descriptor[at + i] as u32) << (8 * i);
            }
            at += bytes;
            let signed = |v: u32| -> i64 {
                match bytes {
                    1 => v as u8 as i8 as i64,
                    2 => v as u16 as i16 as i64,
                    4 => v as i32 as i64,
                    _ => 0,
                }
            };
            let usage = |v: u32, page: u32| if bytes == 4 { v } else { page << 16 | v };
            match (prefix >> 2 & 3, prefix >> 4) {
                (1, 0) => globals.page = value,
                (1, 1) => globals.minimum = signed(value),
                (1, 2) => {
                    globals.maximum_raw = value;
                    globals.maximum_bytes = bytes;
                }
                (1, 7) => globals.size = value as usize,
                (1, 8) => {
                    globals.id = value as u8;
                    layout.ids = true;
                }
                (1, 9) => globals.count = value as usize,
                (1, 10) => stack.push(globals),
                (1, 11) => globals = stack.pop().ok_or("pop without push")?,
                (2, 0) => usages.push(usage(value, globals.page)),
                (2, 1) => usage_min = Some(usage(value, globals.page)),
                (2, 2) => usage_max = Some(usage(value, globals.page)),
                (0, tag) => {
                    if tag == 8 {
                        let start = layout.bits[globals.id as usize];
                        let maximum = if globals.minimum < 0 && globals.maximum_bytes != 0 {
                            match globals.maximum_bytes {
                                1 => globals.maximum_raw as u8 as i8 as i64,
                                2 => globals.maximum_raw as u16 as i16 as i64,
                                _ => globals.maximum_raw as i32 as i64,
                            }
                        } else {
                            globals.maximum_raw as i64
                        };
                        // Variable (not constant, not array) inputs carry one usage each.
                        if value & 3 == 2 {
                            for n in 0..globals.count {
                                let usage = if n < usages.len() {
                                    usages[n]
                                } else if let (Some(low), Some(high)) = (usage_min, usage_max) {
                                    (low + n as u32).min(high)
                                } else {
                                    usages.last().copied().unwrap_or(0)
                                };
                                let field = Field {
                                    id: globals.id,
                                    offset: start + n * globals.size,
                                    size: globals.size,
                                    minimum: globals.minimum,
                                    maximum,
                                    relative: value & 4 != 0,
                                };
                                match (usage >> 16, usage & 0xffff) {
                                    (1, 0x30) if layout.x.is_none() => layout.x = Some(field),
                                    (1, 0x31) if layout.y.is_none() => layout.y = Some(field),
                                    (1, 0x38) if layout.wheel.is_none() => {
                                        layout.wheel = Some(field)
                                    }
                                    (9, button @ 1..=3) => layout.buttons.push((button as u8, field)),
                                    _ => {}
                                }
                            }
                        }
                        layout.bits[globals.id as usize] = start + globals.size * globals.count;
                    }
                    usages.clear();
                    usage_min = None;
                    usage_max = None;
                }
                _ => {}
            }
        }
        Ok(layout)
    }

    /// A report for report ID `id` carrying the given field values; the
    /// report's other fields are cleared.
    fn encode(&self, id: u8, values: &[(Field, i64)]) -> Vec<u8> {
        let bytes = self.bits[id as usize].div_ceil(8);
        let mut data = vec![0u8; bytes];
        for (field, value) in values {
            for bit in 0..field.size {
                if (value >> bit) & 1 != 0 {
                    let at = field.offset + bit;
                    data[at / 8] |= 1 << (at % 8);
                }
            }
        }
        if self.ids {
            data.insert(0, id);
        }
        data
    }

    /// The report carrying X/Y, or another field when X/Y are absent.
    fn primary_id(&self) -> Option<u8> {
        self.x.map(|f| f.id)
    }
}

fn observe() -> (u32, u32, u32, i32) {
    match graphics::mouse_state_with_scroll() {
        Ok(state) => state,
        Err(error) => {
            println!("USB POINTER FAIL syscall: {error:?}");
            std::process::exit(1);
        }
    }
}

/// Poll the graphics syscall until `accept` holds or 3 seconds pass. Returns the
/// last state, with the wheel summed across polls, and whether it was accepted.
fn wait_for(accept: &dyn Fn((u32, u32, u32, i32)) -> bool) -> ((u32, u32, u32, i32), bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut wheel = 0;
    loop {
        let mut state = observe();
        wheel += state.3;
        state.3 = wheel;
        if accept(state) {
            return (state, true);
        }
        if Instant::now() >= deadline {
            return (state, false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Tally {
    passed: usize,
    failed: usize,
}

impl Tally {
    fn case(&mut self, name: &str, passed: bool, detail: &str) {
        if passed {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        println!(
            "USB POINTER CASE {name} {} {detail}",
            if passed { "PASS" } else { "FAIL" }
        );
    }
}

fn host(width: u32, height: u32, tally: &mut Tally) {
    let mut a = (width / 4, height / 3);
    let mut b = (width * 3 / 4, height * 2 / 3);
    let start = observe();
    if start.0.abs_diff(a.0) <= 1 && start.1.abs_diff(a.1) <= 1 {
        std::mem::swap(&mut a, &mut b);
    }
    let cases = [
        ("host/move-a", a, 0, 0),
        ("host/left-press", a, 1, 0),
        ("host/left-release", a, 0, 0),
        ("host/move-b", b, 0, 0),
        ("host/right-press", b, 2, 0),
        ("host/right-release", b, 0, 0),
        ("host/middle-press", b, 4, 0),
        ("host/middle-release", b, 0, 0),
        ("host/wheel-up", b, 0, 1),
        ("host/wheel-down", b, 0, -1),
    ];
    // The host's previous pointer position is unknown before the first request;
    // a relative device moves by the difference between requested positions.
    let mut previous: Option<((u32, u32), (u32, u32))> = None;
    for (seq, (name, (x, y), buttons, wheel)) in cases.iter().enumerate() {
        let before = observe();
        println!("USB POINTER HOST seq={seq} x={x} y={y} buttons={buttons} wheel={wheel}");
        let relative_target = previous.map(|((px, py), (gx, gy))| {
            (
                (gx as i64 + *x as i64 - px as i64).clamp(0, width as i64 - 1) as u32,
                (gy as i64 + *y as i64 - py as i64).clamp(0, height as i64 - 1) as u32,
            )
        });
        let moved = name.starts_with("host/move");
        let (state, passed) = wait_for(&|s| {
            let absolute = s.0.abs_diff(*x) <= 1 && s.1.abs_diff(*y) <= 1;
            let relative = relative_target
                .is_some_and(|(rx, ry)| s.0.abs_diff(rx) <= 1 && s.1.abs_diff(ry) <= 1);
            let position = if moved {
                absolute || relative
            } else {
                // Button and wheel cases must not move the pointer.
                s.0.abs_diff(before.0) <= 1 && s.1.abs_diff(before.1) <= 1
            };
            position && s.2 == *buttons && s.3 == *wheel
        });
        let kind = if !moved || !passed {
            ""
        } else if state.0.abs_diff(*x) <= 1 && state.1.abs_diff(*y) <= 1 {
            " motion=absolute"
        } else {
            " motion=relative"
        };
        tally.case(
            name,
            passed,
            &format!(
                "requested={x},{y},{buttons} observed={},{},{} wheel={}{kind}",
                state.0, state.1, state.2, state.3
            ),
        );
        previous = Some(((*x, *y), (state.0, state.1)));
    }
}

fn replay_one(port: u32, report: &[u8]) -> Result<u64, i64> {
    let mut request = ReplayRequest {
        port,
        len: report.len() as u32,
        report: [0; REPORT_MAX],
    };
    request.report[..report.len()].copy_from_slice(report);
    let result = unsafe {
        syscall3(
            nr::IOCTL,
            0,
            POINTER_REPLAY,
            &mut request as *mut ReplayRequest as u64,
        ) as i64
    };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as u64)
    }
}

fn replay(pointer: &Pointer, width: u32, height: u32, tally: &mut Tally) {
    let prefix = format!("replay/port{}-if{}", pointer.port, pointer.interface);
    let layout = match Layout::parse(&pointer.descriptor) {
        Ok(layout) => layout,
        Err(error) => {
            tally.case(&prefix, false, &format!("descriptor unreadable: {error}"));
            return;
        }
    };
    let Some(id) = layout.primary_id() else {
        println!("USB POINTER NOTE {prefix} has no X axis; motion is not measured on it");
        return;
    };
    let (Some(x), Some(y)) = (layout.x, layout.y) else {
        tally.case(&prefix, false, "X without Y");
        return;
    };
    let buttons: Vec<(u8, Field)> = layout
        .buttons
        .iter()
        .copied()
        .filter(|(_, f)| f.id == id)
        .collect();
    let wheel = layout.wheel.filter(|f| f.id == id && f.relative);
    // A report for one case: relative axes moved by `move_by`, absolute axes at
    // `position`, the given buttons and wheel.
    let report = |position: (u32, u32), move_by: (i64, i64), held: u8, turn: i64| {
        let axis = |field: Field, pixel: u32, delta: i64, extent: u32| {
            if field.relative {
                delta
            } else {
                let range = field.maximum - field.minimum;
                field.minimum + (pixel as i64 * range + (extent as i64 - 1) / 2) / (extent as i64 - 1)
            }
        };
        let mut values = vec![
            (x, axis(x, position.0, move_by.0, width)),
            (y, axis(y, position.1, move_by.1, height)),
        ];
        for (button, field) in &buttons {
            values.push((*field, (held >> (button - 1) & 1) as i64));
        }
        if let Some(field) = wheel {
            values.push((field, turn));
        }
        layout.encode(id, &values)
    };
    let send = |tally: &mut Tally, name: &str, data: Vec<u8>, accept: &dyn Fn((u32, u32, u32, i32)) -> bool| {
        let _ = observe(); // drain press and wheel latches from earlier cases
        match replay_one(pointer.port, &data) {
            Ok(answer) => {
                let (state, passed) = wait_for(accept);
                let passed = passed && answer & 1 != 0 && answer & 2 != 0;
                tally.case(
                    &format!("{prefix}/{name}"),
                    passed,
                    &format!(
                        "report={} delivered={} armed={} observed={},{},{} wheel={}",
                        data.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                        answer & 1,
                        answer >> 1 & 1,
                        state.0,
                        state.1,
                        state.2,
                        state.3
                    ),
                );
            }
            Err(errno) => tally.case(
                &format!("{prefix}/{name}"),
                false,
                &format!("replay control returned {errno}"),
            ),
        }
    };
    let axes = if x.relative { "relative" } else { "absolute" };
    println!("USB POINTER NOTE {prefix} axes={axes} buttons={} wheel={}", buttons.len(), wheel.is_some());
    let start = observe();
    // Where the pointer rests for the button and wheel cases.
    let b = if x.relative {
        // Move toward the centre so no step is clamped at an edge.
        let dx: i64 = if start.0 < width / 2 { 40 } else { -40 };
        let dy: i64 = if start.1 < height / 2 { 30 } else { -30 };
        let a = ((start.0 as i64 + dx) as u32, (start.1 as i64 + dy) as u32);
        send(tally, "move-relative", report(a, (dx, dy), 0, 0), &|s| s.0 == a.0 && s.1 == a.1);
        let back = (start.0, start.1);
        send(tally, "move-relative-back", report(back, (-dx, -dy), 0, 0), &|s| s.0 == back.0 && s.1 == back.1);
        back
    } else {
        let (mut a, mut b) = ((width / 4, height / 3), (width * 3 / 4, height * 2 / 3));
        if start.0.abs_diff(a.0) <= 1 && start.1.abs_diff(a.1) <= 1 {
            std::mem::swap(&mut a, &mut b);
        }
        let near = |p: (u32, u32)| move |s: (u32, u32, u32, i32)| s.0.abs_diff(p.0) <= 1 && s.1.abs_diff(p.1) <= 1;
        send(tally, "move-absolute-a", report(a, (0, 0), 0, 0), &near(a));
        send(tally, "move-absolute-b", report(b, (0, 0), 0, 0), &near(b));
        b
    };
    for (button, name) in [(1u8, "left"), (2, "right"), (3, "middle")] {
        if !buttons.iter().any(|(n, _)| *n == button) {
            println!("USB POINTER NOTE {prefix} declares no button {button}");
            continue;
        }
        let mask = 1u32 << (button - 1);
        send(tally, &format!("{name}-press"), report(b, (0, 0), mask as u8, 0), &|s| s.2 == mask && s.0.abs_diff(b.0) <= 1);
        send(tally, &format!("{name}-release"), report(b, (0, 0), 0, 0), &|s| s.2 == 0 && s.0.abs_diff(b.0) <= 1);
    }
    if wheel.is_some() {
        send(tally, "wheel-up", report(b, (0, 0), 0, 1), &|s| s.3 == 1);
        send(tally, "wheel-down", report(b, (0, 0), 0, -1), &|s| s.3 == -1);
    } else {
        println!("USB POINTER NOTE {prefix} declares no relative wheel in its pointer report");
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if !matches!(mode.as_str(), "host" | "replay" | "both") {
        println!("USB POINTER FAIL usage: usb_pointer_check host|replay|both");
        std::process::exit(2);
    }
    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(error) => {
            println!("USB POINTER FAIL framebuffer: {error:?}");
            std::process::exit(1);
        }
    };
    let (width, height) = (info.width as u32, info.height as u32);
    let attached = match pointers() {
        Ok(attached) if !attached.is_empty() => attached,
        Ok(_) => {
            println!("USB POINTER FAIL no USB HID pointer attached within 30 s");
            std::process::exit(1);
        }
        Err(error) => {
            println!("USB POINTER FAIL {error}");
            std::process::exit(1);
        }
    };
    let mut tally = Tally { passed: 0, failed: 0 };
    if mode != "replay" {
        host(width, height, &mut tally);
    }
    if mode != "host" {
        for pointer in &attached {
            replay(pointer, width, height, &mut tally);
        }
    }
    println!(
        "USB POINTER DONE mode={mode} passed={} failed={}",
        tally.passed, tally.failed
    );
    std::process::exit(if tally.failed == 0 { 0 } else { 1 });
}
