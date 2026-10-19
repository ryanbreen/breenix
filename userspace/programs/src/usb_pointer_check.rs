//! USB pointer measurement for Parallels and VMware (ARM64), run by
//! `./run.sh --parallels|--vmware --program /bin/usb_pointer_check --program-arg MODE`
//! with a kernel built with the `usb_pointer_replay` feature.
//!
//! The observations come from the production graphics syscall that bwm
//! reads (`mouse_state_with_scroll`). MODE names what the run measures, words
//! joined by '-':
//!
//! - `absolute`, `relative`: the launcher's host pointer path, for the interface
//!   kind named. The check prints `USB POINTER HOST seq=.. axes=.. x=.. y=..
//!   buttons=.. wheel=..` and the host-key watcher (`scripts/hostkeys.py`) sends
//!   that pointer state: over VMware's VNC server (Fusion's configuration decides
//!   whether its tablet or its mouse reports it), or through the Parallels SDK
//!   (an absolute position for the tablet, a relative move for the mouse). Each
//!   motion case requires the guest to move to the requested position (absolute)
//!   or by the requested distance (relative). An unscored first request gives the
//!   host its origin.
//! - `replay`: for each interface of a kind the run measured, the reports that
//!   interface's device sent during the host cases (`recorded=` in
//!   `/proc/xhci/pointers`) go back through the kernel's replay control, which
//!   completes one of the interface's posted interrupt-IN TDs with each: the
//!   Transfer Event, the report read from the TD's buffer, the decode, the input
//!   report and the TDs posted again. Each report's effect is computed by this
//!   program's own reading of the report descriptor.
use libbreenix::graphics;
use libbreenix::syscall::{nr, raw::syscall3};
use std::time::{Duration, Instant};

/// Kernel replay control (`drivers::xhci::replay`).
const POINTER_REPLAY: u64 = 0xB813_0006;
const REPORT_MAX: usize = 64;

#[repr(C)]
struct ReplayRequest {
    port: u32,
    endpoint: u32,
    len: u32,
    report: [u8; REPORT_MAX],
}

/// One attached pointer interface from `/proc/xhci/pointers`.
struct Pointer {
    port: u32,
    interface: u32,
    endpoint: u32,
    descriptor: Vec<u8>,
    /// The reports the device sent this interface, oldest first.
    recorded: Vec<Vec<u8>>,
    received: u64,
}

fn unhex(hex: &str, line: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err(format!("odd hex in {line:?}"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| format!("bad hex in {line:?}"))
}

/// The attached pointers, once the xHCI worker has attached them: the program
/// can start before the controller's port census ends, so wait up to 30 s.
fn pointers() -> Result<Vec<Pointer>, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let trace = std::fs::read_to_string("/proc/xhci/trace")
            .map_err(|error| format!("reading /proc/xhci/trace: {error}"))?;
        if trace.contains("state=ready ") {
            break std::fs::read_to_string("/proc/xhci/pointers")
                .map_err(|error| format!("reading /proc/xhci/pointers: {error}"))?;
        }
        if !trace.contains("state=enumerating ") || Instant::now() >= deadline {
            return Err(format!("pointer census did not complete: {trace}"));
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
        let descriptor = unhex(&field("descriptor=").unwrap_or_default(), line)?;
        let Some(recorded) = field("recorded=") else {
            return Err(format!("no recorded= field in {line:?}: the kernel needs the usb_pointer_replay feature"));
        };
        let recorded = recorded
            .split(',')
            .filter(|hex| !hex.is_empty())
            .map(|hex| unhex(hex, line))
            .collect::<Result<Vec<_>, _>>()?;
        let endpoint = field("endpoint=")
            .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
        match (number("port="), number("interface="), endpoint) {
            (Some(port), Some(interface), Some(endpoint)) => found.push(Pointer {
                port,
                interface,
                endpoint,
                descriptor,
                recorded,
                received: field("received=").and_then(|v| v.parse().ok())
                    .ok_or_else(|| format!("missing received counter in {line:?}"))?,
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

    /// The value of `field` in `report`, which starts with its report ID
    /// when the descriptor declares IDs.
    fn value(&self, report: &[u8], field: Field) -> i64 {
        let data = if self.ids { &report[1..] } else { report };
        let mut raw = 0i64;
        for bit in 0..field.size {
            let at = field.offset + bit;
            if data.get(at / 8).is_some_and(|byte| byte >> (at % 8) & 1 != 0) {
                raw |= 1 << bit;
            }
        }
        if field.minimum < 0 && field.size < 64 && raw >> (field.size - 1) & 1 != 0 {
            raw -= 1 << field.size;
        }
        raw
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

/// The host cases for interface kind `axes`, numbered from `seq`; returns the next seq.
fn host(axes: &str, mut seq: usize, width: u32, height: u32, tally: &mut Tally) -> usize {
    let start = observe();
    let (a, b) = if axes == "relative" {
        // The guest moves by the requests' differences: a step of a quarter of
        // the width and a sixth of the height away from the guest pointer's
        // nearer edges, and back, which no edge clamps.
        let b = (width / 2, height / 2);
        let x = if start.0 < width / 2 { b.0 + width / 4 } else { b.0 - width / 4 };
        let y = if start.1 < height / 2 { b.1 + height / 6 } else { b.1 - height / 6 };
        ((x, y), b)
    } else {
        let (a, b) = ((width / 4, height / 3), (width * 3 / 4, height * 2 / 3));
        // The first request goes to b, away from where the pointer starts.
        if start.0.abs_diff(b.0) <= 1 && start.1.abs_diff(b.1) <= 1 { (b, a) } else { (a, b) }
    };
    let near = |s: (u32, u32, u32, i32), p: (u32, u32)| s.0.abs_diff(p.0) <= 1 && s.1.abs_diff(p.1) <= 1;
    // Snapshot native reports before the origin request. Position alone
    // cannot distinguish absolute input when host and guest both start at zero.
    let native_before = if axes == "absolute" { pointers().unwrap_or_default() } else { Vec::new() };
    println!("USB POINTER HOST seq={seq} axes={axes} x={} y={} buttons=0 wheel=0", b.0, b.1);
    seq += 1;
    let landed = if axes == "absolute" {
        let moved = !near(start, b) && wait_for(&|s| near(s, b)).1;
        moved && pointers().unwrap_or_default().iter().any(|p| {
            let Some(old) = native_before.iter().find(|old| old.port == p.port && old.interface == p.interface) else { return false; };
            let Some(count) = p.received.checked_sub(old.received) else { return false; };
            if count == 0 || count > p.recorded.len() as u64 { return false; }
            let Ok(layout) = Layout::parse(&p.descriptor) else { return false; };
            let (Some(x), Some(y)) = (layout.x, layout.y) else { return false; };
            if x.relative || y.relative || x.id != y.id { return false; }
            p.recorded[p.recorded.len() - count as usize..].iter().any(|report| {
                if layout.ids && report.first() != Some(&x.id) { return false; }
                let position = |field: Field, extent: u32| {
                    let range = field.maximum - field.minimum;
                    if range <= 0 { return None; }
                    Some(((layout.value(report, field) - field.minimum).clamp(0, range)
                        * (extent as i64 - 1) / range) as u32)
                };
                matches!((position(x, width), position(y, height)), (Some(px), Some(py))
                    if px.abs_diff(b.0) <= 1 && py.abs_diff(b.1) <= 1)
            })
        })
    } else {
        std::thread::sleep(Duration::from_millis(500));
        false
    };
    let cases = [
        ("move-a", a, 0, 0),
        ("left-press", a, 1, 0),
        ("left-release", a, 0, 0),
        ("move-b", b, 0, 0),
        ("right-press", b, 2, 0),
        ("right-release", b, 0, 0),
        ("middle-press", b, 4, 0),
        ("middle-release", b, 0, 0),
        ("wheel-up", b, 0, 1),
        ("wheel-down", b, 0, -1),
    ];
    let mut previous = (b, observe());
    // Absolute coverage requires a fresh native absolute report at the target.
    let mut distinguished = landed;
    for (name, (x, y), buttons, wheel) in cases {
        let before = observe();
        println!("USB POINTER HOST seq={seq} axes={axes} x={x} y={y} buttons={buttons} wheel={wheel}");
        seq += 1;
        let ((px, py), observed) = previous;
        let relative_target = (
            (observed.0 as i64 + x as i64 - px as i64).clamp(0, width as i64 - 1) as u32,
            (observed.1 as i64 + y as i64 - py as i64).clamp(0, height as i64 - 1) as u32,
        );
        let target = if axes == "relative" { relative_target } else { (x, y) };
        let moved = name.starts_with("move");
        // A move case whose expected position is where the pointer already is
        // (an earlier step having failed) measures nothing, and fails.
        let measurable = !moved || !near(before, target);
        let (state, passed) = wait_for(&|s| {
            // Button and wheel cases must not move the pointer.
            let position = if moved { near(s, target) } else { near(s, (before.0, before.1)) };
            measurable && position && s.2 == buttons && s.3 == wheel
        });
        // A relative move shows its kind when the pointer moved somewhere the
        // requested position does not explain.
        distinguished |= axes == "relative"
            && moved
            && passed
            && !near(state, (before.0, before.1))
            && !near(state, (x, y));
        tally.case(
            &format!("host/{axes}/{name}"),
            passed,
            &format!(
                "requested={x},{y},{buttons} expected={},{} observed={},{},{} wheel={}",
                target.0, target.1, state.0, state.1, state.2, state.3
            ),
        );
        previous = ((x, y), state);
    }
    tally.case(
        &format!("host/{axes}/coverage"),
        distinguished,
        if axes == "absolute" {
            "requires motion onto the first target and a fresh native absolute X/Y report at that target"
        } else {
            "requires a move to where the requested position does not explain"
        },
    );
    seq
}

fn replay_one(pointer: &Pointer, report: &[u8]) -> Result<u64, i64> {
    let mut request = ReplayRequest {
        port: pointer.port,
        endpoint: pointer.endpoint,
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

/// What one interface's replayed reports covered.
#[derive(Default)]
struct Covered {
    reports: usize,
    moved: bool,
    pressed: bool,
    turned: bool,
    wheel: bool,
}

/// Replay `pointer`'s recorded reports.
fn replay(pointer: &Pointer, width: u32, height: u32, tally: &mut Tally) -> Covered {
    let prefix = format!("replay/port{}-if{}", pointer.port, pointer.interface);
    let Ok(layout) = Layout::parse(&pointer.descriptor) else {
        tally.case(&prefix, false, "descriptor unreadable");
        return Covered::default();
    };
    let (Some(x), Some(y)) = (layout.x, layout.y) else {
        tally.case(&prefix, false, "missing X/Y");
        return Covered::default();
    };
    let id = x.id;
    let wheel = layout.wheel.filter(|f| f.id == id && f.relative);
    let buttons: Vec<(u8, Field)> = layout.buttons.iter().copied().filter(|(_, f)| f.id == id).collect();
    let reports: Vec<&Vec<u8>> = pointer
        .recorded
        .iter()
        .filter(|r| !layout.ids || r.first() == Some(&id))
        .collect();
    let mut covered = Covered { wheel: wheel.is_some(), ..Covered::default() };
    if reports.is_empty() {
        return covered;
    }
    let scale = |field: Field, value: i64, extent: u32| {
        let range = (field.maximum - field.minimum).max(1);
        ((value - field.minimum).clamp(0, range) * (extent as i64 - 1) / range) as u32
    };
    for (n, report) in reports.iter().enumerate() {
        let (vx, vy) = (layout.value(report, x), layout.value(report, y));
        let held = buttons
            .iter()
            .fold(0u32, |held, (button, field)| held | ((layout.value(report, *field) & 1) as u32) << (button - 1));
        let turn = wheel.map_or(0, |field| layout.value(report, field)).signum() as i32;
        let _ = observe(); // drain press and wheel latches from earlier cases
        let before = observe();
        let target = if x.relative {
            (
                (before.0 as i64 + vx).clamp(0, width as i64 - 1) as u32,
                (before.1 as i64 + vy).clamp(0, height as i64 - 1) as u32,
            )
        } else {
            (scale(x, vx, width), scale(y, vy, height))
        };
        covered.moved |= target.0.abs_diff(before.0) > 1 || target.1.abs_diff(before.1) > 1;
        covered.pressed |= held != 0;
        covered.turned |= turn != 0;
        let hex: String = report.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("{prefix}/report-{n}");
        match replay_one(pointer, report) {
            Ok(answer) => {
                let (state, seen) = wait_for(&|s| {
                    s.0.abs_diff(target.0) <= 1 && s.1.abs_diff(target.1) <= 1 && s.2 == held && s.3 == turn
                });
                tally.case(
                    &name,
                    seen && answer & 1 != 0 && answer & 2 != 0,
                    &format!(
                        "report={hex} expected={},{},{held} wheel={turn} delivered={} rearmed={} observed={},{},{} wheel={}",
                        target.0,
                        target.1,
                        answer & 1,
                        answer >> 1 & 1,
                        state.0,
                        state.1,
                        state.2,
                        state.3
                    ),
                );
            }
            Err(errno) => tally.case(&name, false, &format!("report={hex} replay control returned {errno}")),
        }
    }
    covered.reports = reports.len();
    covered
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let words: Vec<&str> = mode.split('-').collect();
    let kinds: Vec<&str> = words.iter().copied().filter(|w| matches!(*w, "absolute" | "relative")).collect();
    let replaying = words.contains(&"replay");
    let mut unique = words.clone();
    unique.sort_unstable();
    unique.dedup();
    if kinds.is_empty()
        || unique.len() != words.len()
        || words.iter().any(|w| !matches!(*w, "absolute" | "relative" | "replay"))
    {
        println!("USB POINTER FAIL usage: usb_pointer_check absolute|relative[-absolute|-relative][-replay]");
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
    let census = || match pointers() {
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
    let attached = census();
    let kind_of = |pointer: &Pointer| {
        let layout = Layout::parse(&pointer.descriptor).ok()?;
        let (x, y) = (layout.x?, layout.y?);
        (x.id == y.id && x.relative == y.relative).then_some(if x.relative { "relative" } else { "absolute" })
    };
    let mut tally = Tally { passed: 0, failed: 0 };
    for kind in &kinds {
        tally.case(
            &format!("{kind}/interface"),
            attached.iter().any(|p| kind_of(p) == Some(kind)),
            &format!("requires an attached interface with {kind} X/Y"),
        );
    }
    let mut seq = 0;
    for kind in &kinds {
        seq = host(kind, seq, width, height, &mut tally);
    }
    if replaying {
        // The census again, for the reports the host cases produced. A device
        // may report one pointer's motion and its buttons or wheel on different
        // interfaces (VMware's tablet and mouse), so each interface's reports
        // go back and coverage is the device's.
        let attached = census();
        let mut ports: Vec<u32> = attached.iter().map(|p| p.port).collect();
        ports.sort_unstable();
        ports.dedup();
        let mut by_kind = vec![0usize; kinds.len()];
        for port in ports {
            let mut device = Covered::default();
            for pointer in attached.iter().filter(|p| p.port == port) {
                let covered = replay(pointer, width, height, &mut tally);
                if let Some(i) = kinds.iter().position(|k| kind_of(pointer) == Some(k)) {
                    by_kind[i] += covered.reports;
                }
                device.reports += covered.reports;
                device.moved |= covered.moved;
                device.pressed |= covered.pressed;
                device.turned |= covered.turned;
                device.wheel |= covered.wheel;
            }
            if device.reports != 0 {
                tally.case(
                    &format!("replay/port{port}/coverage"),
                    device.moved && device.pressed && (device.turned || !device.wheel),
                    &format!(
                        "{} reports replayed: moved={} pressed={} wheel={}",
                        device.reports, device.moved, device.pressed, device.turned
                    ),
                );
            }
        }
        for (kind, replayed) in kinds.iter().zip(by_kind) {
            tally.case(
                &format!("replay/{kind}/recorded"),
                replayed > 0,
                &format!("{replayed} recorded reports of {kind} interfaces replayed"),
            );
        }
    }
    println!(
        "USB POINTER DONE mode={mode} passed={} failed={}",
        tally.passed, tally.failed
    );
    std::process::exit(if tally.failed == 0 && tally.passed > 0 { 0 } else { 1 });
}
