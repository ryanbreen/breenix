//! events: window input events (contract section 19).

use crate::*;

pub(crate) const EVENTS: &[Case] = &[
    pictured!("key-press-release", "A key press and its release arrive with the same keycode", events_key_press_release),
    pictured!("key-text", "A key event carries the UTF-8 text the compositor attached", events_key_text),
    pictured!("key-modifiers", "Shift, Ctrl, Alt and Super arrive as separate modifier bits", events_key_modifiers),
    pictured!("key-repeat", "A repeat arrives marked as a repeat", events_key_repeat),
    pictured!("text-event", "A text event delivers its UTF-8 string whole", events_text_event),
    pictured!("mouse-move", "Pointer motion arrives in window-local coordinates", events_mouse_move),
    pictured!("mouse-buttons", "Left, right and middle press and release arrive with the right button", events_mouse_buttons),
    pictured!("scroll", "Vertical and horizontal scroll arrive with their signed amounts", events_scroll),
    pictured!("focus", "FocusGained and FocusLost arrive as sent", events_focus),
    pictured!("close-requested", "CloseRequested arrives as sent", events_close_requested),
    pictured!("resized", "Resized arrives only after the buffers have the new size, and says that size", events_resized),
    pictured!("resize-failed", "A failed resize delivers ResizeFailed with the old size, never Resized", events_resize_failed),
    pictured!("font-changed", "FontChanged arrives when the configured font changes", events_font_changed),
    pictured!("compositor-changed", "A new compositor claim delivers CompositorChanged with the new epoch", events_compositor_changed),
    pictured!("unknown-type", "An unknown event type arrives as Unknown, not as a key press", events_unknown_type),
    pictured!("order", "100 events arrive in the order sent", events_order),
    pictured!("serial", "Each event carries its input serial", events_serial),
    pictured!("processed-serial", "After the app's handlers return and it acquires, draws and commits, the commit's processed serial is the highest serial whose handler had returned at acquire", events_processed_serial),
    pictured!("processed-not-polled", "An event polled but not yet handled when an animation frame is committed is not acknowledged by that commit", events_processed_not_polled),
    pictured!("processed-at-acquire", "An event whose handler returns after frame N was acquired is acknowledged by frame N + 1, not by frame N", events_processed_at_acquire),
    pictured!("no-change-after-redraw", "commit_no_change does not acknowledge an event whose handler returned Redraw until a frame acquired after it is committed", events_no_change_after_redraw),
    pictured!("processed-ack", "commit_no_change publishes the processed serial without changing pixels", events_processed_ack),
    pictured!("overflow", "A full ring reports how many events were lost instead of dropping them silently", events_overflow),
    pictured!("wait-timeout", "wait_event with a timeout returns no earlier than the timeout and within 100 ms after it when nothing arrives", events_wait_timeout),
    pictured!("wait-eintr", "A signal interrupts a blocked wait_event with EINTR", events_wait_eintr),
    pictured!("wait-targeted", "An event for one window does not wake a reader blocked on another", events_wait_targeted),
    pictured!("pollable", "A window's event descriptor reports POLLIN through poll when an event is queued", events_pollable),
    pictured!("per-window", "Two windows of one process each receive only their own events", events_per_window),
    pictured!("after-destroy", "Writing to a destroyed window fails with ENOENT", events_after_destroy),
    pictured!("abi-layout", "The event struct has the contract's literal size and field offsets", events_abi_layout),
    pictured!("no-overloaded-fields", "Resize size, button state and text travel in named fields, not borrowed ones", events_no_overloaded_fields),
    pictured!("malformed-write", "A compositor write with an invalid type or length fails with EINVAL and delivers nothing", events_malformed_write),
];

// ---------------------------------------------------------------------------------------
// events: window input events (contract section 19)
// ---------------------------------------------------------------------------------------

use super::kit::*;
use abi::{event_kind as kind, op, Plain};
use breengel::{Delivered, Event, Handled};

/// An event of `k` with nothing else set.
fn ev(k: u16) -> abi::WindowEvent {
    let mut e = abi::WindowEvent::ZERO;
    e.kind = k;
    e
}

fn key(k: u16, keycode: u32) -> abi::WindowEvent {
    let mut e = ev(k);
    e.keycode = keycode;
    e
}

fn with_text(mut e: abi::WindowEvent, text: &[u8]) -> abi::WindowEvent {
    e.text[..text.len()].copy_from_slice(text);
    e.text_len = text.len() as u32;
    e
}

/// Have kiosk_test write `events` to window `id` in one call: the first serial it gave them.
fn send(k: &mut Helper, id: u64, events: &[abi::WindowEvent]) -> Result<u64, String> {
    let words: Vec<String> = events.iter().map(event_hex).collect();
    let answer = k.ask(&format!("event {id} {}", words.join(" ")))?;
    let mut parts = answer.split(' ');
    match (parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok()), parts.next().and_then(|s| s.parse::<u64>().ok())) {
        (Some("ok"), Some(n), Some(first)) if n == events.len() => Ok(first),
        _ => Err(format!("kiosk_test's WRITE_EVENTS of {} event(s) answered `{answer}`", events.len())),
    }
}

/// A Breengel window and a compositor that claimed before it was made, so the window's queue
/// starts empty.
fn setup(title: &[u8]) -> Result<(Helper, breengel::Window), libbreenix::suite::CaseError> {
    let (k, _) = kiosk().cite("24")?;
    let w = breengel::Window::new_without_fonts(title, 32, 24).cite("17.1")?;
    Ok((k, w))
}

/// Dispatch until `n` events have been handled (within 2 s), every handler answering `answer`.
fn collect(w: &mut breengel::Window, n: usize, answer: Handled) -> Result<Vec<Delivered>, libbreenix::suite::CaseError> {
    let w_id=w.id();
    let mut buttons=0u32;
    let mut got = Vec::new();
    let start = now_ms();
    while got.len() < n && since(start) < 2_000 {
        w.wait_dispatch(100_000_000, |d| {
            match &d.event {
                Event::MouseMove{x,y,..} => libbreenix::suite::input_step(*x,*y,buttons!=0,&format!("window {}: {:?}",w_id,d.event)),
                Event::MouseButton{x,y,pressed,button} => {
                    if (breengel::buttons::LEFT..=breengel::buttons::MIDDLE).contains(button) {
                        let bit=1u32 << (*button-breengel::buttons::LEFT);
                        if *pressed {buttons|=bit;}else{buttons&=!bit;}
                    }
                    libbreenix::suite::input_step(*x,*y,buttons!=0,&format!("window {}: {:?}",w_id,d.event));
                }
                _ => libbreenix::suite::input_key(&format!("window {}: {:?}",w_id,d.event)),
            }
            got.push(d.clone());
            answer
        }).map_err(|e| format!("dispatch: {e}")).cite("19.4")?;
    }
    ensure(got.len() >= n, "19.2", format!("{} of {n} events arrived within 2 s", got.len()))?;
    Ok(got)
}

fn kinds(got: &[Delivered]) -> String {
    got.iter().map(|d| format!("{:?}", d.event)).collect::<Vec<_>>().join(", ")
}

/// Send `events` and require Breengel to deliver exactly `want`, in order.
fn round_trip(title: &[u8], events: &[abi::WindowEvent], want: &[Event], section: &str) -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(title)?;
    send(&mut k, w.id(), events).cite("19.2")?;
    let got = collect(&mut w, want.len(), Handled::NoChange)?;
    let events: Vec<Event> = got.iter().map(|d| d.event.clone()).collect();
    ensure(events == want, section, format!("Breengel delivered [{}]; the compositor sent {:?}", kinds(&got), want))
}

/// `READ_META`: the newest commit's id and the metadata it published.
fn read_meta(id: u64) -> Result<(u64, abi::Meta), String> {
    let mut meta = abi::Meta::ZERO;
    let mut q = abi::MetaQuery::ZERO;
    q.id = id;
    q.meta = &mut meta as *mut abi::Meta as u64;
    let r = call(op::READ_META, &mut q);
    if r < 0 {
        return Err(format!("READ_META {}", outcome(r)));
    }
    Ok((q.commit_id, meta))
}

fn events_key_press_release() -> CaseResult {
    let m = breengel::Modifiers::default();
    round_trip(b"key-press-release", &[key(kind::KEY_PRESS, 30), key(kind::KEY_RELEASE, 30)], &[
        Event::KeyPress { ascii: 0, keycode: 30, modifiers: m, text: String::new(), repeat: false },
        Event::KeyRelease { keycode: 30, modifiers: m },
    ], "19.1")
}

fn events_key_text() -> CaseResult {
    let m = breengel::Modifiers::default();
    let accent = "é".as_bytes();
    round_trip(b"key-text", &[with_text(key(kind::KEY_PRESS, 18), accent), with_text(key(kind::KEY_PRESS, 30), b"a")], &[
        Event::KeyPress { ascii: 0, keycode: 18, modifiers: m, text: String::from("é"), repeat: false },
        Event::KeyPress { ascii: b'a', keycode: 30, modifiers: m, text: String::from("a"), repeat: false },
    ], "19.1")
}

fn events_key_modifiers() -> CaseResult {
    use abi::modifier::{ALT, CTRL, SHIFT, SUPER};
    let bits = [SHIFT, CTRL, ALT, SUPER, SHIFT | CTRL | ALT | SUPER];
    let events: Vec<abi::WindowEvent> = bits.iter().map(|&b| {
        let mut e = key(kind::KEY_PRESS, 30);
        e.modifiers = b;
        e
    }).collect();
    let want: Vec<Event> = bits.iter().map(|&b| Event::KeyPress {
        ascii: 0, keycode: 30, modifiers: breengel::Modifiers::from_bits(b), text: String::new(), repeat: false,
    }).collect();
    for (b, e) in bits.iter().zip(want.iter()) {
        if let Event::KeyPress { modifiers, .. } = e {
            let set = [modifiers.shift, modifiers.ctrl, modifiers.alt, modifiers.logo];
            let expect = [b & SHIFT != 0, b & CTRL != 0, b & ALT != 0, b & SUPER != 0];
            ensure(set == expect, "19.1", format!("modifier bits {b:#x} decode to shift/ctrl/alt/super {set:?}"))?;
        }
    }
    round_trip(b"key-modifiers", &events, &want, "19.1")
}

fn events_key_repeat() -> CaseResult {
    let m = breengel::Modifiers::default();
    let mut repeat = key(kind::KEY_PRESS, 30);
    repeat.flags = abi::EVENT_REPEAT;
    round_trip(b"key-repeat", &[key(kind::KEY_PRESS, 30), repeat], &[
        Event::KeyPress { ascii: 0, keycode: 30, modifiers: m, text: String::new(), repeat: false },
        Event::KeyPress { ascii: 0, keycode: 30, modifiers: m, text: String::new(), repeat: true },
    ], "19.1")
}

fn events_text_event() -> CaseResult {
    let long = "ü".repeat(120);
    round_trip(b"text-event", &[with_text(ev(kind::TEXT), long.as_bytes()), with_text(ev(kind::TEXT), "窓 ok".as_bytes())], &[
        Event::Text { text: long.clone() },
        Event::Text { text: String::from("窓 ok") },
    ], "19.1")
}

fn events_mouse_move() -> CaseResult {
    let at = |x: i32, y: i32| {
        let mut e = ev(kind::POINTER_MOVE);
        e.x = x;
        e.y = y;
        e
    };
    round_trip(b"mouse-move", &[at(5, 17), at(31, 0), at(-3, -9)], &[
        Event::MouseMove { x: 5, y: 17 }, Event::MouseMove { x: 31, y: 0 }, Event::MouseMove { x: -3, y: -9 },
    ], "19.1")
}

fn events_mouse_buttons() -> CaseResult {
    let mut events = Vec::new();
    let mut want = Vec::new();
    for button in [abi::button::LEFT, abi::button::RIGHT, abi::button::MIDDLE] {
        for pressed in [1u32, 0] {
            let mut e = ev(kind::POINTER_BUTTON);
            e.button = button;
            e.pressed = pressed;
            e.x = 7;
            e.y = 9;
            events.push(e);
            want.push(Event::MouseButton { button, pressed: pressed == 1, x: 7, y: 9 });
        }
    }
    round_trip(b"mouse-buttons", &events, &want, "19.1")
}

fn events_scroll() -> CaseResult {
    let s = |x: i32, y: i32| {
        let mut e = ev(kind::SCROLL);
        e.scroll_x = x;
        e.scroll_y = y;
        e
    };
    round_trip(b"scroll", &[s(0, 3), s(-2, 0), s(4, -1)], &[
        Event::Scroll { delta_x: 0, delta_y: 3 }, Event::Scroll { delta_x: -2, delta_y: 0 }, Event::Scroll { delta_x: 4, delta_y: -1 },
    ], "19.1")
}

fn events_focus() -> CaseResult {
    round_trip(b"focus", &[ev(kind::FOCUS_GAINED), ev(kind::FOCUS_LOST), ev(kind::FOCUS_GAINED)],
        &[Event::FocusGained, Event::FocusLost, Event::FocusGained], "19.1")
}

fn events_close_requested() -> CaseResult {
    round_trip(b"close-requested", &[ev(kind::CLOSE_REQUESTED)], &[Event::CloseRequested], "19.1")
}

fn events_resized() -> CaseResult {
    interface()?;
    let w = W::new(32, 24, 2)?;
    w.frame(3).cite("17.3")?;
    let mut r = abi::Resize::ZERO;
    r.id = w.id();
    r.width = 48;
    r.height = 20;
    want_ok(call(op::RESIZE, &mut r), "17.1", "RESIZE to 48x20")?;
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    let resized: Vec<&abi::WindowEvent> = got.iter().filter(|e| e.kind == kind::RESIZED).collect();
    ensure(resized.len() == 1 && (resized[0].width, resized[0].height) == (48, 20), "19.2", format!(
        "RESIZE to 48x20 delivered {} Resized event(s), the first saying {:?}", resized.len(), resized.first().map(|e| (e.width, e.height))))?;
    // When it arrives, the buffers have that size: a capture and the listing say so.
    let (c, _) = capture(w.id(), 48, 20, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    ensure((c.width, c.height) == (48, 20), "19.2", format!("after Resized(48x20) arrived, CAPTURE gives {}x{}", c.width, c.height))?;
    let mut m = abi::MapBuffers::ZERO;
    m.id = w.id();
    want_ok(call(op::MAP_COMMITTED, &mut m), "17.1", "MAP_COMMITTED")?;
    ensure(m.stride == 48 * 4, "19.2", format!("after Resized(48x20) arrived, the buffers' stride is {}; 48 pixels are 192 bytes", m.stride))?;
    // And through Breengel, as Event::Resized.
    let (_k, mut bw) = setup(b"resized")?;
    bw.apply_resize(40, 30).cite("17.1")?;
    let got = collect(&mut bw, 1, Handled::NoChange)?;
    ensure(got.iter().any(|d| d.event == Event::Resized { width: 40, height: 30 }), "19.2", format!("Breengel delivered [{}] after a resize to 40x30", kinds(&got)))?;
    ensure((bw.width(), bw.height()) == (40, 30), "19.2", format!("Breengel's window is {}x{} after Resized(40x30)", bw.width(), bw.height()))
}

fn events_resize_failed() -> CaseResult {
    interface()?;
    let w = W::new(32, 24, 2)?;
    for (width, height) in [(0u32, 10u32), (5000, 10)] {
        let mut r = abi::Resize::ZERO;
        r.id = w.id();
        r.width = width;
        r.height = height;
        want_errno(call(op::RESIZE, &mut r), EINVAL, "17.2", &format!("RESIZE to {width}x{height}"))?;
    }
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    ensure(!got.iter().any(|e| e.kind == kind::RESIZED), "19.2", "a refused RESIZE delivered Resized")?;
    let failed: Vec<(u32, u32)> = got.iter().filter(|e| e.kind == kind::RESIZE_FAILED).map(|e| (e.width, e.height)).collect();
    ensure(failed == [(32, 24), (32, 24)], "19.2", format!("two refused RESIZEs delivered ResizeFailed with sizes {failed:?}; each carries the old size 32x24"))?;
    ensure(Event::from_raw(got.iter().find(|e| e.kind == kind::RESIZE_FAILED).unwrap()) == Event::ResizeFailed { width: 32, height: 24 }, "19.1",
        "Breengel does not decode ResizeFailed with its size")
}

/// A font configuration directory for this process (`BREENIX_CONFIG_ROOT`), restored on drop.
struct FontRoot {
    root: std::path::PathBuf,
    old: Option<std::ffi::OsString>,
}

impl FontRoot {
    fn new() -> Result<FontRoot, String> {
        let root = std::path::PathBuf::from(format!("/tmp/breengel-events-font-{}", getpid()));
        std::fs::create_dir_all(root.join("etc")).map_err(|e| format!("create {}: {e}", root.display()))?;
        let f = FontRoot { root, old: std::env::var_os("BREENIX_CONFIG_ROOT") };
        f.config(14)?;
        std::env::set_var("BREENIX_CONFIG_ROOT", &f.root);
        Ok(f)
    }

    fn config(&self, mono_size: u32) -> Result<(), String> {
        let text = format!("mono.font=/usr/share/fonts/DejaVuSansMono.ttf\nmono.size={mono_size}\ndisplay.font=/usr/share/fonts/DejaVuSans.ttf\ndisplay.size=16\n");
        std::fs::write(self.root.join("etc/pending"), text).map_err(|e| format!("write fonts.conf: {e}"))?;
        std::fs::rename(self.root.join("etc/pending"), self.root.join("etc/fonts.conf")).map_err(|e| format!("replace fonts.conf: {e}"))
    }
}

impl Drop for FontRoot {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => std::env::set_var("BREENIX_CONFIG_ROOT", v),
            None => std::env::remove_var("BREENIX_CONFIG_ROOT"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn events_font_changed() -> CaseResult {
    interface()?;
    let fonts = FontRoot::new().cite("9.2")?;
    let (_k, _) = kiosk().cite("24")?;
    let mut w = breengel::Window::new(b"font-changed", 32, 24).cite("17.1")?;
    // Nothing changed yet: no FontChanged after the first check interval.
    let _ = time::sleep_ms(300);
    let mut seen = Vec::new();
    w.dispatch(|d| {
        seen.push(d.event.clone());
        Handled::NoChange
    }).map_err(|e| format!("dispatch: {e}")).cite("19.4")?;
    ensure(!seen.contains(&Event::FontChanged), "9.3", "FontChanged arrived before the configured font changed")?;
    fonts.config(20).cite("9.2")?;
    let _ = time::sleep_ms(300);
    let mut seen = Vec::new();
    w.dispatch(|d| {
        seen.push(d.event.clone());
        Handled::NoChange
    }).map_err(|e| format!("dispatch: {e}")).cite("19.4")?;
    ensure(seen.contains(&Event::FontChanged), "9.3", format!("after fonts.conf changed the mono size and 300 ms passed, dispatch delivered [{seen:?}] without FontChanged"))?;
    ensure((w.mono_size() - 20.0).abs() < 0.01, "9.3", format!("after FontChanged the window's mono size is {}; the configuration says 20", w.mono_size()))
}

fn events_compositor_changed() -> CaseResult {
    interface()?;
    let mut w = breengel::Window::new_without_fonts(b"compositor-changed", 32, 24).cite("17.1")?;
    let (mut first, e1) = kiosk().cite("24")?;
    let got = collect(&mut w, 1, Handled::NoChange)?;
    ensure(got.iter().any(|d| d.event == Event::CompositorChanged { epoch: e1 }), "18", format!(
        "after kiosk_test claimed with epoch {e1}, Breengel delivered [{}]", kinds(&got)))?;
    first.kill_checked().cite("24")?;
    let (_second, e2) = kiosk().cite("24")?;
    let got = collect(&mut w, 1, Handled::NoChange)?;
    ensure(got.iter().any(|d| d.event == Event::CompositorChanged { epoch: e2 }) && e2 > e1, "18", format!(
        "after a second claim with epoch {e2} (the first was {e1}), Breengel delivered [{}]", kinds(&got)))
}

fn events_unknown_type() -> CaseResult {
    for k in [0u16, 15, 99, u16::MAX] {
        let mut e = key(k, 30);
        e.text = [b'x'; 240];
        e.text_len = 1;
        let got = Event::from_raw(&e);
        ensure(got == Event::Unknown { kind: k }, "19.1", format!("a WindowEvent of kind {k} decodes as {got:?}; an unknown kind is Event::Unknown"))?;
    }
    Ok(())
}

fn burst(k: &mut Helper, id: u64, count: u32, code: u32) -> Result<u64, String> {
    let answer = k.ask(&format!("burst {id} {count} {} {code}", kind::KEY_PRESS))?;
    let mut parts = answer.split(' ');
    match (parts.next(), parts.next().and_then(|n| n.parse::<u32>().ok()), parts.next().and_then(|s| s.parse::<u64>().ok())) {
        (Some("ok"), Some(n), Some(first)) if n == count => Ok(first),
        _ => Err(format!("kiosk_test's burst of {count} answered `{answer}`")),
    }
}

fn events_order() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let mut codes = Vec::new();
    for half in 0..2u32 {
        burst(&mut k, w.id(), 50, 1 + 50 * half).cite("24")?;
        let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
        codes.extend(got.iter().map(|e| e.keycode));
    }
    let want: Vec<u32> = (1..=100).collect();
    ensure(codes == want, "19.2", format!("100 events arrived with keycodes {:?}...; they were sent 1 to 100 in order",
        codes.iter().take(12).collect::<Vec<_>>()))
}

fn events_serial() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let mut all = Vec::new();
    for _ in 0..3 {
        let first = burst(&mut k, w.id(), 10, 30).cite("24")?;
        let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
        let serials: Vec<u64> = got.iter().map(|e| e.serial).collect();
        let want: Vec<u64> = (first..first + 10).collect();
        ensure(serials == want, "19.3", format!("events written with serials {first}..{} arrived with {serials:?}", first + 9))?;
        all.extend(serials);
    }
    ensure(all.windows(2).all(|p| p[1] > p[0]), "19.3", format!("serials across writes do not increase strictly: {all:?}"))?;
    // One compositor at a time: this one goes before the Breengel check claims its own.
    drop(k);
    let (mut bw_k, mut bw) = setup(b"serial")?;
    let s = send(&mut bw_k, bw.id(), &[key(kind::KEY_PRESS, 30)]).cite("19.3")?;
    let got = collect(&mut bw, 1, Handled::NoChange)?;
    ensure(got[0].serial == s, "19.3", format!("Breengel delivered serial {} for the event written with {s}", got[0].serial))
}

/// The processed serial the window's newest commit published, read back with READ_META.
fn processed(id: u64) -> Result<u64, libbreenix::suite::CaseError> {
    Ok(read_meta(id).cite("19.4")?.1.processed_serial)
}

fn events_processed_serial() -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(b"processed-serial")?;
    let first = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30), key(kind::KEY_PRESS, 31), key(kind::KEY_PRESS, 32)]).cite("19.2")?;
    let got = collect(&mut w, 3, Handled::Redraw)?;
    let last = got.iter().map(|d| d.serial).max().unwrap_or(0);
    ensure(last == first + 2, "19.3", format!("the handled serials end at {last}; they were written {first}..{}", first + 2))?;
    w.acquire(1_000_000_000).cite("17.3")?;
    w.framebuf().clear(libgfx::color::Color::rgb(1, 2, 3));
    let c = w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let (commit_id, meta) = read_meta(w.id()).cite("19.4")?;
    ensure(commit_id == c.commit_id, "19.4", format!("READ_META gives commit {commit_id}; the newest commit is {}", c.commit_id))?;
    ensure(meta.processed_serial == first + 2, "19.4", format!(
        "the frame acquired after the handlers for serials {first}..{} returned published processed serial {}", first + 2, meta.processed_serial))
}

fn events_processed_not_polled() -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(b"processed-not-polled")?;
    let s = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30)]).cite("19.2")?;
    let start = now_ms();
    let mut polled = Vec::new();
    while polled.is_empty() && since(start) < 2_000 {
        polled = w.poll_events();
        let _ = time::sleep_ms(2);
    }
    ensure(!polled.is_empty(), "19.2", "the event never arrived to poll")?;
    // An animation frame, committed with the event polled but not handled.
    w.acquire(1_000_000_000).cite("17.3")?;
    w.framebuf().clear(libgfx::color::Color::rgb(9, 9, 9));
    w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let p = processed(w.id())?;
    ensure(p < s, "19.4", format!("an animation frame committed with serial {s} polled but not handled published processed serial {p}"))
}

fn events_processed_at_acquire() -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(b"processed-at-acquire")?;
    // Frame N is acquired first; the event is handled after that.
    w.acquire(1_000_000_000).cite("17.3")?;
    let s = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30)]).cite("19.2")?;
    collect(&mut w, 1, Handled::Redraw)?;
    w.framebuf().clear(libgfx::color::Color::rgb(4, 4, 4));
    w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let n = processed(w.id())?;
    ensure(n < s, "19.4", format!("frame N, acquired before serial {s}'s handler returned, published processed serial {n}; it must not acknowledge it"))?;
    w.acquire(1_000_000_000).cite("17.3")?;
    w.framebuf().clear(libgfx::color::Color::rgb(5, 5, 5));
    w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let n1 = processed(w.id())?;
    ensure(n1 == s, "19.4", format!("frame N + 1, acquired after serial {s}'s handler returned, published processed serial {n1}"))
}

fn events_no_change_after_redraw() -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(b"no-change-after-redraw")?;
    let first = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30), key(kind::KEY_PRESS, 31), key(kind::KEY_PRESS, 32)]).cite("19.2")?;
    // The first handler answers NoChange, the second Redraw, the third NoChange.
    let mut handled = 0;
    let start = now_ms();
    while handled < 3 && since(start) < 2_000 {
        w.wait_dispatch(100_000_000, |d| {
            handled += 1;
            if d.serial == first + 1 { Handled::Redraw } else { Handled::NoChange }
        }).map_err(|e| format!("dispatch: {e}")).cite("19.4")?;
    }
    ensure(handled == 3, "19.2", format!("{handled} of 3 events arrived"))?;
    w.commit_no_change().cite("17.3")?;
    let p = processed(w.id())?;
    ensure(p == first, "19.4", format!(
        "commit_no_change after serials {first} (NoChange), {} (Redraw) and {} (NoChange) published {p}; it acknowledges only {first}, up to the Redraw", first + 1, first + 2))?;
    w.acquire(1_000_000_000).cite("17.3")?;
    w.framebuf().clear(libgfx::color::Color::rgb(6, 6, 6));
    w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let p = processed(w.id())?;
    ensure(p == first + 2, "19.4", format!("the frame acquired after all three handlers returned published {p}; it acknowledges {}", first + 2))
}

fn events_processed_ack() -> CaseResult {
    interface()?;
    let (mut k, mut w) = setup(b"processed-ack")?;
    w.acquire(1_000_000_000).cite("17.3")?;
    w.framebuf().clear(libgfx::color::Color::rgb(7, 8, 9));
    let frame = w.commit(&[abi::Rect::new(0, 0, 32, 24)]).cite("17.3")?;
    let s = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30)]).cite("19.2")?;
    collect(&mut w, 1, Handled::NoChange)?;
    let before = info_of(w.id())?;
    let c = w.commit_no_change().cite("17.3")?;
    let after = info_of(w.id())?;
    let (commit_id, meta) = read_meta(w.id()).cite("19.4")?;
    ensure(c.commit_id == frame.commit_id + 1 && commit_id == c.commit_id, "17.3", format!(
        "commit_no_change after commit {} returned commit {} and READ_META reads commit {commit_id}", frame.commit_id, c.commit_id))?;
    ensure(meta.processed_serial == s, "19.4", format!("commit_no_change after serial {s}'s NoChange handler published processed serial {}", meta.processed_serial))?;
    ensure(after.pixel_gen == before.pixel_gen, "17.3", format!("commit_no_change moved pixel_gen from {} to {}", before.pixel_gen, after.pixel_gen))?;
    let (_, px) = capture(w.id(), 32, 24, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    same(&px, 32, 24, false, "the frame after commit_no_change", "17.3", |_, _| [9, 8, 7, 0])
}

fn info_of(id: u64) -> Result<abi::WinInfo, libbreenix::suite::CaseError> {
    listed(id).cite("17.1")?.ok_or_else(|| libbreenix::suite::CaseError::Fail(format!("window {id} is not listed ({CONTRACT} section 17.2)")))
}

fn events_overflow() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let first = burst(&mut k, w.id(), 64, 1).cite("24")?;
    let answer = k.ask(&format!("burst {} 10 {} 65", w.id(), kind::KEY_PRESS)).cite("24")?;
    ensure(answer.starts_with("ok 0 "), "19.2", format!("10 events written to a full ring answered `{answer}`; none can be queued"))?;
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    let codes: Vec<u32> = got.iter().take(64).map(|e| e.keycode).collect();
    ensure(codes == (1..=64).collect::<Vec<u32>>() && got[..64.min(got.len())].iter().all(|e| e.kind == kind::KEY_PRESS), "19.2",
        format!("the full ring delivered {} events, keycodes {:?}...; it keeps the 64 it holds", got.len(), codes.iter().take(8).collect::<Vec<_>>()))?;
    let over = got.get(64).copied();
    ensure(got.len() == 65 && over.is_some_and(|e| e.kind == kind::OVERFLOW && e.lost == 10), "19.2", format!(
        "after the 64 kept events the ring delivered {:?}; the contract gives Overflow with lost = 10", over.map(|e| (e.kind, e.lost))))?;
    let next = send(&mut k, w.id(), &[key(kind::KEY_PRESS, 99)]).cite("19.2")?;
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    ensure(got.len() == 1 && got[0].keycode == 99 && got[0].serial == next, "19.2", format!(
        "after the overflow was read, the next event arrived as {:?}", got.iter().map(|e| (e.kind, e.keycode)).collect::<Vec<_>>()))?;
    ensure(first >= 1, "19.3", "serials start at 1")?;
    ensure(Event::from_raw(&over.unwrap()) == Event::Overflow { lost: 10 }, "19.1", "Breengel does not decode Overflow with its count")
}

fn read_blocking(id: u64, timeout_ns: u64) -> i64 {
    let mut out = vec![abi::WindowEvent::ZERO; 4];
    let mut r = abi::ReadEvents::ZERO;
    r.id = id;
    r.ptr = out.as_mut_ptr() as u64;
    r.capacity = 4;
    r.flags = abi::READ_EVENTS_BLOCK;
    r.timeout_ns = timeout_ns;
    call(op::READ_EVENTS, &mut r)
}

fn events_wait_timeout() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let _ = read_events(w.id());
    for ms in [50u64, 200] {
        let start = now_ms();
        let r = read_blocking(w.id(), ms * 1_000_000);
        let took = since(start);
        want_errno(r, ETIMEDOUT, "17.1", &format!("a blocking READ_EVENTS with a {ms} ms timeout and nothing queued"))?;
        value(&format!("timeout-{ms}ms"), took as i64, "ms", Some((ms as i64, ms as i64 + 100)));
        ensure(took >= ms && took <= ms + 100, "17.5", format!("a {ms} ms wait returned after {took} ms; the contract gives {ms} to {} ms", ms + 100))?;
    }
    Ok(())
}

extern "C" fn on_usr1(_: i32) {}

fn events_wait_eintr() -> CaseResult {
    interface()?;
    let mut f = || {
        let action = signal::Sigaction::new(on_usr1);
        if signal::sigaction(signal::SIGUSR1, Some(&action), None).is_err() {
            return 2;
        }
        let mut c = abi::Create::ZERO;
        c.width = 8;
        c.height = 8;
        c.buffers = 2;
        if call(op::CREATE, &mut c) < 0 {
            return 3;
        }
        let _ = io::write(Fd::from_raw(1), b"blocking\n");
        let r = read_blocking(c.id, 5_000_000_000);
        let _ = io::write(Fd::from_raw(1), format!("returned {r}\n").as_bytes());
        0
    };
    let mut h = Helper::call(&mut f).cite("24")?;
    let line = h.line(3_000).cite("24")?;
    ensure(line == "blocking", "24", format!("the helper said `{line}`"))?;
    let _ = time::sleep_ms(100);
    signal::kill(h.pid, signal::SIGUSR1).map_err(|e| format!("kill SIGUSR1: {e}")).cite("17.1")?;
    let start = now_ms();
    let done = h.line(2_000).cite("17.1")?;
    let took = since(start);
    ensure(done == format!("returned {}", -(EINTR as i64)), "17.1", format!("a blocked READ_EVENTS interrupted by a handled SIGUSR1 {done}; the contract gives EINTR"))?;
    ensure(took <= 250, "17.5", format!("the interrupted READ_EVENTS returned {took} ms after the signal"))
}

fn events_wait_targeted() -> CaseResult {
    interface()?;
    let a = W::new(16, 16, 2)?;
    let b = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(a.id());
    let _ = read_events(b.id());
    let id_a = a.id();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut out = vec![abi::WindowEvent::ZERO; 4];
            let mut r = abi::ReadEvents::ZERO;
            r.id = id_a;
            r.ptr = out.as_mut_ptr() as u64;
            r.capacity = 4;
            r.flags = abi::READ_EVENTS_BLOCK;
            r.timeout_ns = 3_000_000_000;
            let n = call(op::READ_EVENTS, &mut r);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            (n, out)
        })
    };
    let _ = time::sleep_ms(50);
    send(&mut k, b.id(), &[key(kind::KEY_PRESS, 20)]).cite("19.2")?;
    let _ = time::sleep_ms(200);
    let early = done.load(std::sync::atomic::Ordering::SeqCst);
    send(&mut k, a.id(), &[key(kind::KEY_PRESS, 10)]).cite("19.2")?;
    let (n, out) = reader.join().map_err(|_| String::from("the reader thread panicked")).cite("19.2")?;
    ensure(!early, "19.2", "a reader blocked on window A returned when an event was written to window B")?;
    ensure(n == 1 && out[0].keycode == 10, "19.2", format!("the reader blocked on window A returned {n} event(s), keycode {}; window A's event has keycode 10", out[0].keycode))?;
    let left = read_events(b.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    ensure(left.len() == 1 && left[0].keycode == 20, "19.2", "window B's event is not waiting on window B")
}

fn events_pollable() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let fd = call_id(op::EVENT_FD, w.id());
    let fd = want_ok(fd, "19.2", "EVENT_FD")?;
    let ready = |ms: i32| -> Result<i16, String> {
        let mut fds = [io::PollFd { fd: fd as i32, events: io::poll_events::POLLIN, revents: 0 }];
        io::poll(&mut fds, ms).map_err(|e| format!("poll: {e}"))?;
        Ok(fds[0].revents)
    };
    let quiet = ready(0).cite("19.2")?;
    ensure(quiet & io::poll_events::POLLIN == 0, "19.2", format!("the event descriptor of a window with no event reports revents {quiet:#x}"))?;
    // A thread blocked in poll wakes when the event is queued.
    let waiter = std::thread::spawn(move || {
        let mut fds = [io::PollFd { fd: fd as i32, events: io::poll_events::POLLIN, revents: 0 }];
        let n = io::poll(&mut fds, 3_000).unwrap_or(0);
        (n, fds[0].revents)
    });
    let _ = time::sleep_ms(50);
    let start = now_ms();
    send(&mut k, w.id(), &[key(kind::KEY_PRESS, 30)]).cite("19.2")?;
    let (n, revents) = waiter.join().map_err(|_| String::from("the polling thread panicked")).cite("19.2")?;
    let woke = since(start);
    ensure(n == 1 && revents & io::poll_events::POLLIN != 0, "19.2", format!("a poll waiting on the event descriptor returned {n} with revents {revents:#x} when the event was queued"))?;
    let now = ready(0).cite("19.2")?;
    ensure(now & io::poll_events::POLLIN != 0, "19.2", format!("with an event queued, the event descriptor reports revents {now:#x}, without POLLIN"))?;
    ensure(woke < 1_000, "19.2", format!("a poll waiting on the event descriptor returned {woke} ms after the event was queued"))?;
    let _ = read_events(w.id());
    let drained = ready(0).cite("19.2")?;
    ensure(drained & io::poll_events::POLLIN == 0, "19.2", format!("after the event was read, the descriptor still reports revents {drained:#x}"))?;
    let _ = io::close(Fd::from_raw(fd as u64));
    step("17.4", child("EVENT_FD as an unrelated process", 3_000, Some(1000), || {
        let r = call_id(op::EVENT_FD, w.id());
        if r == -(EPERM as i64) { Ok(()) } else { Err(format!("an unrelated process's EVENT_FD {}; the contract gives EPERM", outcome(r))) }
    }))
}

fn events_per_window() -> CaseResult {
    interface()?;
    let a = W::new(16, 16, 2)?;
    let b = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(a.id());
    let _ = read_events(b.id());
    send(&mut k, a.id(), &[key(kind::KEY_PRESS, 10), key(kind::KEY_PRESS, 11)]).cite("19.2")?;
    send(&mut k, b.id(), &[key(kind::KEY_PRESS, 20)]).cite("19.2")?;
    let ga: Vec<u32> = read_events(a.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?.iter().map(|e| e.keycode).collect();
    let gb: Vec<u32> = read_events(b.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?.iter().map(|e| e.keycode).collect();
    ensure(ga == [10, 11] && gb == [20], "19.2", format!("window A received keycodes {ga:?} and window B {gb:?}; A was sent [10, 11] and B [20]"))
}

fn events_after_destroy() -> CaseResult {
    interface()?;
    let mut w = W::new(16, 16, 2)?;
    let id = w.id();
    let (mut k, _) = kiosk().cite("24")?;
    want_ok(w.destroy(), "17.2", "DESTROY")?;
    let answer = k.ask(&format!("event {id} {}", event_hex(&key(kind::KEY_PRESS, 30)))).cite("24")?;
    ensure(answer == "err 2", "19.2", format!("the compositor's WRITE_EVENTS to destroyed window {id} answered `{answer}`; the contract gives ENOENT (err 2)"))
}

fn put<const N: usize>(bytes: &mut [u8], at: usize, v: [u8; N]) {
    bytes[at..at + N].copy_from_slice(&v);
}

fn events_abi_layout() -> CaseResult {
    interface()?;
    ensure(core::mem::size_of::<abi::WindowEvent>() == 320, "19.1", format!("WindowEvent is {} bytes; the contract gives 320", core::mem::size_of::<abi::WindowEvent>()))?;
    // Every field written at its literal offset in raw bytes, sent and read back raw.
    let mut raw = [0u8; 320];
    put(&mut raw, 8, 0x1122_3344_5566_7788u64.to_le_bytes());
    put(&mut raw, 16, 0u64.to_le_bytes());
    put(&mut raw, 24, kind::KEY_PRESS.to_le_bytes());
    put(&mut raw, 26, abi::EVENT_REPEAT.to_le_bytes());
    put(&mut raw, 28, 0b1010u32.to_le_bytes());
    put(&mut raw, 32, 57u32.to_le_bytes());
    put(&mut raw, 36, 0x111u32.to_le_bytes());
    put(&mut raw, 40, (-4i32).to_le_bytes());
    put(&mut raw, 44, 1234i32.to_le_bytes());
    put(&mut raw, 48, (-7i32).to_le_bytes());
    put(&mut raw, 52, 8i32.to_le_bytes());
    put(&mut raw, 56, 640u32.to_le_bytes());
    put(&mut raw, 60, 480u32.to_le_bytes());
    put(&mut raw, 64, 1u32.to_le_bytes());
    put(&mut raw, 68, 3u32.to_le_bytes());
    put(&mut raw, 80, *b"abc");
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let answer = k.ask(&format!("event {} {hex}", w.id())).cite("24")?;
    let serial: u64 = answer.strip_prefix("ok 1 ").and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("kiosk_test's WRITE_EVENTS answered `{answer}`")).cite("19.2")?;
    let mut out = [0u8; 320];
    let mut r = abi::ReadEvents::ZERO;
    r.id = w.id();
    r.ptr = out.as_mut_ptr() as u64;
    r.capacity = 1;
    want_ok(call(op::READ_EVENTS, &mut r), "19.1", "READ_EVENTS")?;
    let mut want = raw;
    put(&mut want, 0, serial.to_le_bytes());
    for (at, len, name) in [(0, 8, "serial"), (8, 8, "time_ns"), (16, 8, "epoch"), (24, 2, "kind"), (26, 2, "flags"), (28, 4, "modifiers"),
        (32, 4, "keycode"), (36, 4, "button"), (40, 4, "x"), (44, 4, "y"), (48, 4, "scroll_x"), (52, 4, "scroll_y"), (56, 4, "width"),
        (60, 4, "height"), (64, 4, "pressed"), (68, 4, "text_len"), (72, 4, "lost"), (76, 4, "reserved"), (80, 240, "text")] {
        ensure(out[at..at + len] == want[at..at + len], "19.1", format!(
            "the event read back has {name} @{at} = {:02x?}; it was written as {:02x?}", &out[at..at + len.min(8)], &want[at..at + len.min(8)]))?;
    }
    // The ABI crate's structure reads the same bytes.
    // SAFETY: WindowEvent is plain data of exactly these 320 bytes.
    let e: abi::WindowEvent = unsafe { core::ptr::read_unaligned(out.as_ptr() as *const abi::WindowEvent) };
    ensure((e.serial, e.time_ns, e.kind, e.flags, e.modifiers, e.keycode, e.button) == (serial, 0x1122_3344_5566_7788, kind::KEY_PRESS, abi::EVENT_REPEAT, 0b1010, 57, 0x111)
        && (e.x, e.y, e.scroll_x, e.scroll_y, e.width, e.height, e.pressed, e.text_len) == (-4, 1234, -7, 8, 640, 480, 1, 3), "19.1",
        "breengel-abi's WindowEvent does not read the fields at the contract's offsets")
}

fn events_no_overloaded_fields() -> CaseResult {
    interface()?;
    // Kernel-made: Resized carries its size in width and height and nothing else.
    let w = W::new(16, 16, 2)?;
    let _ = read_events(w.id());
    let mut r = abi::Resize::ZERO;
    r.id = w.id();
    r.width = 24;
    r.height = 12;
    want_ok(call(op::RESIZE, &mut r), "17.1", "RESIZE")?;
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    let e = got.iter().find(|e| e.kind == kind::RESIZED).copied().ok_or("RESIZE delivered no Resized event (section 19.2)").cite("19.2")?;
    ensure((e.width, e.height) == (24, 12) && (e.keycode, e.button, e.x, e.y, e.pressed, e.text_len, e.scroll_x, e.scroll_y, e.modifiers, e.lost) == (0, 0, 0, 0, 0, 0, 0, 0, 0, 0), "19.1", format!(
        "Resized carries width {} height {} and keycode {} button {} x {} y {} pressed {} text_len {}; the size travels in width and height only",
        e.width, e.height, e.keycode, e.button, e.x, e.y, e.pressed, e.text_len))?;
    // Compositor-made: button state in pressed, text in text, through Breengel.
    let mut b = ev(kind::POINTER_BUTTON);
    b.button = abi::button::RIGHT;
    b.pressed = 1;
    b.x = 3;
    b.y = 4;
    let t = with_text(key(kind::KEY_PRESS, 16), b"q");
    let (mut k, mut bw) = setup(b"no-overloaded-fields")?;
    send(&mut k, bw.id(), &[b, t]).cite("19.2")?;
    let got = collect(&mut bw, 2, Handled::NoChange)?;
    ensure(got[0].event == Event::MouseButton { button: abi::button::RIGHT, pressed: true, x: 3, y: 4 }, "19.1", format!("a right-button press decodes as {:?}", got[0].event))?;
    ensure(matches!(&got[1].event, Event::KeyPress { keycode: 16, text, ascii: b'q', .. } if text == "q"), "19.1", format!("a key press with text `q` decodes as {:?}", got[1].event))
}

fn events_malformed_write() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let mut long = key(kind::KEY_PRESS, 30);
    long.text_len = 241;
    let mut reserved = key(kind::KEY_PRESS, 30);
    reserved.reserved = 1;
    let cases = [
        ("kind 0", ev(0)), ("kind 15", ev(15)), ("FontChanged, which Breengel makes", ev(kind::FONT_CHANGED)),
        ("Overflow, which the kernel makes", ev(kind::OVERFLOW)), ("Resized, which the kernel makes", ev(kind::RESIZED)),
        ("ResizeFailed, which the kernel makes", ev(kind::RESIZE_FAILED)), ("CompositorChanged, which the kernel makes", ev(kind::COMPOSITOR_CHANGED)),
        ("text_len 241", long), ("a nonzero reserved field", reserved),
    ];
    for (what, bad) in cases {
        let answer = k.ask(&format!("event {} {}", w.id(), event_hex(&bad))).cite("24")?;
        ensure(answer == "err 22", "19.2", format!("a compositor write with {what} answered `{answer}`; the contract gives EINVAL (err 22)"))?;
        // A valid event before the bad one in the same call is not delivered either.
        let pair = k.ask(&format!("event {} {} {}", w.id(), event_hex(&key(kind::KEY_PRESS, 31)), event_hex(&bad))).cite("24")?;
        ensure(pair == "err 22", "19.2", format!("a write of a valid event and one with {what} answered `{pair}`; the contract gives EINVAL"))?;
    }
    let got = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
    ensure(got.is_empty(), "19.2", format!("refused writes delivered {} event(s), keycodes {:?}", got.len(), got.iter().map(|e| e.keycode).collect::<Vec<_>>()))
}
