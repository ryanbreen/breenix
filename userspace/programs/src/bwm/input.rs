//! Source input, descriptor wake, and ordered client admission (E18 W1/W9/W29).
use super::*;
use abi::Plain;
use libbreenix::input::{
    self as core_input, axis, disposition as outcome, key, kind, reason, syn, Event,
};
use libbreenix::window::{self, abi};
use std::collections::{BTreeMap, BTreeSet};

static READER: std::sync::OnceLock<core_input::Reader> = std::sync::OnceLock::new();
static SOURCE: std::sync::Mutex<(Event, u32)> = std::sync::Mutex::new((
    Event {
        serial: 0,
        time_ns: 0,
        device: 0,
        kind: 0,
        code: 0,
        value: 0,
        flags: 0,
    },
    0,
));
static DERIVED: std::sync::Mutex<BTreeMap<u64, u32>> = std::sync::Mutex::new(BTreeMap::new());
static QUEUES: std::sync::Mutex<input_backlog::Backlog<Queued>> =
    std::sync::Mutex::new(input_backlog::Backlog::new());
#[derive(Clone, Copy)]
struct Queued {
    event: abi::WindowEvent,
    source: u64,
    derived: u32,
}

fn dispose(source: u64, derived: u32, window: u64, disposition: u32, reason: u32) {
    if source == 0 {
        return;
    }
    let d = core_input::Disposition {
        serial: source,
        window,
        disposition,
        reason,
        derived,
        reserved: 0,
    };
    match READER.get().expect("input reader").dispose(&d) {
        Ok(()) => {}
        // The core may already have terminated a row during pressure recovery.
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EALREADY)) => {}
        Err(e) => eprintln!("[bwm] input disposition {}: {}", source, e),
    }
}
fn context(event: Event) {
    *SOURCE.lock().unwrap() = (event, 0);
}
fn derived(mut event: abi::WindowEvent) -> Queued {
    let context = SOURCE.lock().unwrap();
    let source = context.0;
    let mut indices = DERIVED.lock().unwrap();
    let index = indices.entry(source.serial).or_default();
    *index += 1;
    let index = *index;
    event.serial = if source.serial == 0 {
        0
    } else {
        core_input::abi::derived_serial(source.serial, index).expect("derived identity capacity")
    };
    event.time_ns = source.time_ns;
    event.modifiers = source.flags;
    Queued {
        event,
        source: source.serial,
        derived: index,
    }
}
fn enqueue(id: u64, event: Queued) {
    let mut queues = QUEUES.lock().unwrap();
    if queues.push(id, event, |_, _| false) == input_backlog::Admission::Disabled {
        if event.event.kind == abi::event_kind::CLOSE_REQUESTED {
            queues.request_close(id, event);
        } else {
            dispose(
                event.source,
                event.derived,
                id,
                outcome::LOST,
                reason::RING_FULL,
            );
        }
    }
    drop(queues);
    flush_input();
}
pub(super) fn send_notice(id: u64, input: Input) {
    enqueue(id, derived(session::event(input)));
}
pub(super) fn send_to(windows: &[Window], index: usize, input: Input) {
    if let Some(w) = windows.get(index) {
        send_notice(w.window_id, input);
    }
}
fn losses(queues: &mut input_backlog::Backlog<Queued>) {
    use input_backlog::DropReason;
    for (id, q, why) in queues.take_discarded() {
        let (id, why) = match why {
            DropReason::Overflow => (id, reason::OVERFLOW),
            DropReason::Gone => (0, reason::TARGET_GONE),
            DropReason::Failed => (id, reason::RING_FULL),
        };
        dispose(q.source, q.derived, id, outcome::LOST, why);
    }
}
pub(super) fn flush_input() {
    use input_backlog::Delivery;
    let mut queues = QUEUES.lock().unwrap();
    losses(&mut queues);
    queues.flush(
        |id, q| match session::send(id, q.event) {
            Ok(1) => {
                dispose(q.source, q.derived, id, outcome::DELIVERED, reason::NONE);
                Delivery::Accepted
            }
            Ok(_) | Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {
                Delivery::Full
            }
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Delivery::Gone,
            Err(e) => {
                eprintln!("[bwm] event delivery {}: {}", id, e);
                Delivery::Failed
            }
        },
        |lost| Queued {
            event: session::event(Input::Overflow { lost }),
            source: 0,
            derived: 0,
        },
    );
    losses(&mut queues);
}
pub(super) fn retain_windows(windows: &[Window]) {
    let mut queues = QUEUES.lock().unwrap();
    queues.retain_windows(|id| windows.iter().any(|w| w.window_id == id));
    losses(&mut queues);
}
pub(super) fn handled_serial(seen: u64) -> u64 {
    QUEUES
        .lock()
        .unwrap()
        .pending()
        .filter(|e| e.source != 0 && e.derived == 0)
        .map(|e| e.source - 1)
        .min()
        .unwrap_or(seen)
        .min(seen)
}
pub(super) fn pending_input() -> bool {
    !QUEUES.lock().unwrap().is_empty()
}

struct Held {
    event: Event,
    target: Option<u64>,
    text: Option<u8>,
    next: u64,
    consumed: bool,
}
#[derive(Clone, Copy)]
struct Click {
    target: u64,
    time: u64,
    x: i32,
    y: i32,
    count: u32,
}
pub(super) struct Stream {
    devices: BTreeMap<u32, (u64, Vec<core_input::Capability>)>,
    keys: BTreeMap<(u32, u16), Held>,
    captures: BTreeMap<(u32, u16), (u64, u32)>,
    clicks: BTreeMap<(u32, u16), Click>,
    hover: Option<u64>,
    gestures: BTreeMap<(u32, u64), u32>,
    pub(super) seen: u64,
    chrome_device: Option<u32>,
}
impl Stream {
    pub(super) fn new() -> Self {
        let reader = core_input::Reader::open(true).expect("open compositor input reader");
        reader.grab(&[]).expect("claim compositor input grab");
        assert!(READER.set(reader).is_ok(), "one input reader");
        Self {
            devices: BTreeMap::new(),
            keys: BTreeMap::new(),
            captures: BTreeMap::new(),
            clicks: BTreeMap::new(),
            hover: None,
            gestures: BTreeMap::new(),
            seen: 0,
            chrome_device: None,
        }
    }
    pub(super) fn fd(&self) -> i32 {
        READER.get().unwrap().fd().raw() as i32
    }
    pub(super) fn pointer_active(&self) -> bool {
        !self.captures.is_empty()
    }
    pub(super) fn deadline(&self) -> Option<u64> {
        self.keys
            .values()
            .filter(|k| !k.consumed && repeatable(k.event.code))
            .map(|k| k.next)
            .min()
    }
    fn remember(&mut self, device: u32) {
        if !self.devices.contains_key(&device) {
            let mut caps =
                vec![core_input::Capability::default(); core_input::abi::MAX_CAPABILITIES];
            let info = READER.get().unwrap().device_info(device, &mut caps);
            let Ok(info) = info else {
                return;
            };
            caps.truncate(info.count as usize);
            self.devices.insert(device, (info.generation, caps));
        }
    }
    fn normalize(&mut self, e: Event, extent: usize) -> Option<i32> {
        self.remember(e.device);
        let (_, caps) = self.devices.get(&e.device)?;
        let c = caps
            .iter()
            .find(|c| c.kind == kind::ABS && c.code == e.code)?;
        Some(
            ((e.value as i64 - c.minimum as i64) * (extent as i64 - 1)
                / (c.maximum as i64 - c.minimum as i64))
                .clamp(0, extent as i64 - 1) as i32,
        )
    }
}

/// US key text uses the physical key code; modifiers remain independent bits.
fn key_text(code: u16, mods: u32) -> Option<u8> {
    if mods & (core_input::modifier::CTRL | core_input::modifier::ALT | core_input::modifier::SUPER)
        != 0
    {
        return None;
    }
    let shift = mods & core_input::modifier::SHIFT != 0;
    let letter = match code {
        16..=25 => Some(b"qwertyuiop"[(code - 16) as usize]),
        30..=38 => Some(b"asdfghjkl"[(code - 30) as usize]),
        44..=50 => Some(b"zxcvbnm"[(code - 44) as usize]),
        _ => None,
    };
    if let Some(c) = letter {
        return Some(if shift { c.to_ascii_uppercase() } else { c });
    }
    match code {
        2..=11 => Some(if shift {
            b"!@#$%^&*()"[(code - 2) as usize]
        } else {
            b"1234567890"[(code - 2) as usize]
        }),
        12 => Some(if shift { b'_' } else { b'-' }),
        13 => Some(if shift { b'+' } else { b'=' }),
        26 => Some(if shift { b'{' } else { b'[' }),
        27 => Some(if shift { b'}' } else { b']' }),
        39 => Some(if shift { b':' } else { b';' }),
        40 => Some(if shift { b'"' } else { b'\'' }),
        41 => Some(if shift { b'~' } else { b'`' }),
        43 => Some(if shift { b'|' } else { b'\\' }),
        51 => Some(if shift { b'<' } else { b',' }),
        52 => Some(if shift { b'>' } else { b'.' }),
        53 => Some(if shift { b'?' } else { b'/' }),
        57 => Some(b' '),
        _ => None,
    }
}
fn repeatable(code: u16) -> bool {
    !matches!(code, 1 | 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126)
}
fn motion(e: Event) -> bool {
    e.kind == kind::ABS || (e.kind == kind::REL && matches!(e.code, axis::X | axis::Y))
}
fn chord_key(code: u16) -> u8 {
    match code {
        key::ENTER => b'\r',
        key::TAB => b'\t',
        _ => key_text(code, 0).unwrap_or(0),
    }
}
impl State {
    fn target(&self) -> Option<u64> {
        self.windows
            .get(self.focused_win)
            .filter(|w| !w.minimized)
            .map(|w| w.window_id)
    }
    fn raw(&mut self, id: u64, source: Event, mut event: abi::WindowEvent) {
        event.serial = source.serial;
        event.time_ns = source.time_ns;
        event.modifiers = source.flags;
        enqueue(
            id,
            Queued {
                event,
                source: source.serial,
                derived: 0,
            },
        );
    }
    fn notice(&mut self, id: u64, event: abi::WindowEvent) {
        enqueue(id, derived(event));
    }
    fn rejected(&self, e: Event, why: u32) {
        dispose(e.serial, 0, 0, outcome::REJECTED, why);
    }
    fn handled(&self, e: Event) {
        dispose(e.serial, 0, 0, outcome::WM_HANDLED, reason::NONE);
    }
    pub(super) fn input(&mut self, ready: bool, damage: &mut Damage) {
        context(Event::default());
        if ready {
            let mut bytes = vec![0; core_input::abi::READER_EVENTS * (32 + 256)];
            loop {
                let records = match READER.get().unwrap().read(&mut bytes) {
                    Ok(records) => records
                        .map(|r| r.map(|r| (r.event, r.payload.to_vec())))
                        .collect::<Result<Vec<_>, _>>()
                        .expect("valid core records"),
                    Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => break,
                    Err(e) => panic!("input stream: {}", e),
                };
                if records.is_empty() {
                    break;
                }
                let mut scene = None;
                let mut motion_end = 0;
                let mut position = None;
                for (at, (event, payload)) in records.iter().cloned().enumerate() {
                    // Axes in one contiguous part of a SYN batch describe one
                    // position. Route their source serials at that position,
                    // rather than requesting an intermediate one-axis resize.
                    if motion(event) && at >= motion_end {
                        let mut point = (self.mouse_x, self.mouse_y);
                        motion_end = at;
                        position = Some(point);
                        for &(axis, _) in &records[at..] {
                            if axis.device != event.device || !motion(axis) {
                                break;
                            }
                            motion_end += 1;
                            match self.axis_position(axis, point) {
                                Some(next) => point = next,
                                None => {
                                    position = None;
                                    break;
                                }
                            }
                            position = Some(point);
                        }
                    }
                    self.stream.seen = self.stream.seen.max(event.serial);
                    context(event);
                    self.stream.remember(event.device);
                    match event.kind {
                        kind::SYN => {
                            if event.code == syn::SCENE {
                                scene = Some(core_input::SceneTag {
                                    scene_version: u64::from_le_bytes(
                                        payload[..8].try_into().unwrap(),
                                    ),
                                    window: u64::from_le_bytes(payload[8..16].try_into().unwrap()),
                                    layout_version: u64::from_le_bytes(
                                        payload[16..24].try_into().unwrap(),
                                    ),
                                    gesture: u64::from_le_bytes(
                                        payload[24..32].try_into().unwrap(),
                                    ),
                                });
                            } else if event.code == syn::DROPPED {
                                let held: Vec<_> = self
                                    .stream
                                    .keys
                                    .values()
                                    .map(|held| (held.event, held.target))
                                    .collect();
                                for (source, target) in held {
                                    context(source);
                                    if let Some(id) = target {
                                        self.notice(
                                            id,
                                            session::event(Input::KeyRelease {
                                                keycode: source.code as u32,
                                                modifiers: 0,
                                            }),
                                        );
                                    }
                                }
                                context(event);
                                let targets: BTreeSet<_> =
                                    self.stream.captures.values().map(|(id, _)| *id).collect();
                                for id in targets {
                                    let mut cancel = abi::WindowEvent::ZERO;
                                    cancel.kind = abi::event_kind::POINTER_CANCEL;
                                    self.notice(id, cancel);
                                }
                                self.stream.keys.clear();
                                self.stream.captures.clear();
                                self.stream.chrome_device = None;
                                self.dragging = None;
                                self.resizing = None;
                            } else if event.code == syn::REPORT {
                                if event.value == syn::DEVICE_REMOVED {
                                    self.stream.devices.remove(&event.device);
                                    self.stream.clicks.retain(|(d, _), _| *d != event.device);
                                }
                                scene = None;
                            }
                            self.handled(event);
                        }
                        kind::KEY if event.code < key::BUTTON_LEFT => self.key_event(event),
                        kind::TEXT => self.text_event(event, &payload),
                        kind::ABS | kind::REL | kind::KEY => self.pointer_event(
                            event,
                            scene,
                            damage,
                            motion(event).then_some(position).flatten(),
                        ),
                        _ => self.rejected(event, reason::NO_FOCUS),
                    }
                    if !self
                        .stream
                        .keys
                        .values()
                        .any(|k| k.event.serial == event.serial)
                    {
                        DERIVED.lock().unwrap().remove(&event.serial);
                    }
                }
            }
        }
        self.repeat();
        flush_input();
        // A registry change can move an edge away from a stationary pointer.
        // Shape follows the current scene even without another motion event.
        self.cursor(damage);
        context(Event::default());
    }
    fn text_event(&mut self, event: Event, bytes: &[u8]) {
        let Some(id) = self.target() else {
            self.rejected(event, reason::NO_FOCUS);
            return;
        };
        let mut at = 0;
        while at < bytes.len() {
            let mut end = (at + 240).min(bytes.len());
            while std::str::from_utf8(&bytes[at..end]).is_err() {
                end -= 1;
            }
            let mut text = [0; 240];
            text[..end - at].copy_from_slice(&bytes[at..end]);
            let raw = session::event(Input::Text {
                bytes: text,
                len: (end - at) as u32,
            });
            if at == 0 {
                self.raw(id, event, raw);
            } else {
                self.notice(id, raw);
            }
            at = end;
        }
    }
    fn key_event(&mut self, e: Event) {
        let slot = (e.device, e.code);
        if e.value == 0 {
            if let Some(held) = self.stream.keys.remove(&slot) {
                DERIVED.lock().unwrap().remove(&held.event.serial);
                if held.consumed {
                    self.handled(e);
                } else if let Some(id) = held.target.filter(|id| {
                    self.windows
                        .iter()
                        .any(|w| w.window_id == *id && !w.minimized)
                }) {
                    self.raw(
                        id,
                        e,
                        session::event(Input::KeyRelease {
                            keycode: e.code as u32,
                            modifiers: e.flags,
                        }),
                    );
                } else {
                    self.rejected(e, reason::TARGET_GONE);
                }
            } else {
                self.rejected(e, reason::NO_FOCUS);
            }
            if matches!(e.code, key::LEFT_SUPER | key::RIGHT_SUPER) {
                if !self.hotkey_mgr.super_combo_used {
                    if self
                        .hotkey_mgr
                        .tap_release_ns
                        .is_some_and(|last| e.time_ns.saturating_sub(last) <= 400_000_000)
                    {
                        let action = self
                            .hotkey_mgr
                            .bindings
                            .iter()
                            .find(|b| b.key == 0 && b.modifiers == modifier::SUPER && b.taps == 2)
                            .map(|b| b.action.clone());
                        self.hotkey_mgr.tap_release_ns = None;
                        if let Some(action) = action {
                            self.action(&action);
                        }
                    } else {
                        self.hotkey_mgr.tap_release_ns = Some(e.time_ns);
                    }
                } else {
                    self.hotkey_mgr.tap_release_ns = None;
                }
                self.hotkey_mgr.super_combo_used = false;
            }
            return;
        }
        if e.value != 1 || self.stream.keys.contains_key(&slot) {
            self.handled(e);
            return;
        }
        let is_modifier = matches!(e.code, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126);
        if !is_modifier && e.flags & core_input::modifier::SUPER != 0 {
            self.hotkey_mgr.super_combo_used = true;
            self.hotkey_mgr.tap_release_ns = None;
        }
        let mut consumed = false;
        if (key::F1..key::F1 + 5).contains(&e.code) && e.flags == 0 {
            let (order, count) = sorted_by_creation(&self.windows);
            let n = (e.code - key::F1) as usize;
            if n < count && !self.windows[order[n]].minimized {
                self.focus(Some(self.windows[order[n]].window_id), false);
            }
            consumed = true;
        } else {
            let chord = chord_key(e.code);
            let action = self
                .hotkey_mgr
                .bindings
                .iter()
                .filter(|b| {
                    b.key != 0 && b.key == chord && b.modifiers as u32 == e.flags && b.taps == 1
                })
                .map(|b| b.action.clone())
                .next();
            if let Some(action) = action {
                self.action(&action);
                consumed = true;
            }
        }
        let target = self.target();
        let text = key_text(e.code, e.flags);
        if consumed {
            self.handled(e);
        } else if let Some(id) = target {
            let mut raw = abi::WindowEvent::ZERO;
            raw.kind = abi::event_kind::KEY_PRESS;
            raw.keycode = e.code as u32;
            if let Some(c) = text {
                raw.text[0] = c;
                raw.text_len = 1;
            }
            self.raw(id, e, raw);
            if let Some(c) = text {
                let mut bytes = [0; 240];
                bytes[0] = c;
                self.notice(id, session::event(Input::Text { bytes, len: 1 }));
            }
        } else {
            self.rejected(e, reason::NO_FOCUS);
        }
        self.stream.keys.insert(
            slot,
            Held {
                event: e,
                target,
                text,
                next: libbreenix::ui_clock::now_ns().saturating_add(500_000_000),
                consumed,
            },
        );
    }
    fn repeat(&mut self) {
        let now = libbreenix::ui_clock::now_ns();
        let mut due = Vec::new();
        for held in self
            .stream
            .keys
            .values_mut()
            .filter(|held| !held.consumed && repeatable(held.event.code))
        {
            while held.next <= now {
                due.push((held.event, held.target, held.text, held.next));
                held.next = held.next.saturating_add(50_000_000);
            }
        }
        for (source, target, text, time) in due {
            context(source);
            if let Some(id) = target.filter(|id| self.target() == Some(*id)) {
                let mut event = abi::WindowEvent::ZERO;
                event.kind = abi::event_kind::KEY_PRESS;
                event.flags = abi::EVENT_REPEAT;
                event.keycode = source.code as u32;
                if let Some(text) = text {
                    event.text[0] = text;
                    event.text_len = 1;
                }
                let mut q = derived(event);
                q.event.time_ns = time;
                enqueue(id, q);
            }
        }
    }
    fn action(&mut self, action: &HotkeyAction) {
        match action {
            HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
                let mut eligible: Vec<_> = self.windows.iter().filter(|w| !w.minimized).collect();
                eligible.sort_by_key(|w| w.creation_order);
                if !eligible.is_empty() {
                    let pos = eligible
                        .iter()
                        .position(|w| Some(w.window_id) == self.target());
                    let next = match (action, pos) {
                        (HotkeyAction::FocusPrev, Some(p)) => {
                            (p + eligible.len() - 1) % eligible.len()
                        }
                        (_, Some(p)) => (p + 1) % eligible.len(),
                        _ => 0,
                    };
                    self.focus(Some(eligible[next].window_id), false);
                }
            }
            _ => execute_hotkey_action(action, &mut self.windows, self.focused_win),
        }
    }
}
impl State {
    fn axis_position(&mut self, e: Event, (mut x, mut y): (i32, i32)) -> Option<(i32, i32)> {
        let extent = if e.code == axis::X {
            self.screen_w
        } else {
            self.screen_h
        };
        let value = if e.kind == kind::ABS {
            self.stream.normalize(e, extent)?
        } else {
            (if e.code == axis::X { x } else { y })
                .saturating_add(e.value)
                .clamp(0, extent as i32 - 1)
        };
        if e.code == axis::X {
            x = value;
        } else {
            y = value;
        }
        Some((x, y))
    }
    fn hit(&self) -> Option<usize> {
        self.windows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, w)| !w.minimized && w.hit_any(self.mouse_x, self.mouse_y))
            .map(|(i, _)| i)
    }
    fn content_hit(&self) -> Option<u64> {
        self.hit()
            .filter(|&i| self.windows[i].hit_content(self.mouse_x, self.mouse_y))
            .map(|i| self.windows[i].window_id)
    }
    fn local(&self, id: u64) -> Option<(i32, i32)> {
        self.windows
            .iter()
            .find(|w| w.window_id == id && !w.minimized)
            .map(|w| (self.mouse_x - w.content_x(), self.mouse_y - w.content_y()))
    }
    fn hover(&mut self) {
        let hit = self.content_hit();
        if hit == self.stream.hover {
            return;
        }
        if let Some(id) = self.stream.hover {
            if let Some((x, y)) = self.local(id) {
                let mut e = abi::WindowEvent::ZERO;
                e.kind = abi::event_kind::POINTER_LEAVE;
                e.x = x;
                e.y = y;
                self.notice(id, e);
            }
        }
        self.stream.hover = hit;
        if let Some(id) = hit {
            if let Some((x, y)) = self.local(id) {
                let mut e = abi::WindowEvent::ZERO;
                e.kind = abi::event_kind::POINTER_ENTER;
                e.x = x;
                e.y = y;
                self.notice(id, e);
            }
        }
    }
    fn pointer_event(
        &mut self,
        e: Event,
        scene: Option<core_input::SceneTag>,
        damage: &mut Damage,
        position: Option<(i32, i32)>,
    ) {
        if let Some(tag) = scene {
            let slot = (e.device, tag.gesture);
            if !self.stream.gestures.contains_key(&slot) {
                let mut status = abi::CompositorStatus::ZERO;
                let current = window::call(abi::op::COMPOSITOR_STATUS, &mut status)
                    .map(|_| status.scene_version)
                    .unwrap_or(0);
                let why = if current != tag.scene_version {
                    reason::STALE_SCENE
                } else if self.content_hit().is_some_and(|id| id != tag.window)
                    && e.kind == kind::KEY
                {
                    reason::STALE_SCENE
                } else {
                    0
                };
                self.stream.gestures.insert(slot, why);
                while self.stream.gestures.len() > 1024 {
                    self.stream.gestures.pop_first();
                }
            }
            let why = self.stream.gestures[&slot];
            if why != 0 {
                self.rejected(e, why);
                return;
            }
        }
        if motion(e) {
            let Some(point) =
                position.or_else(|| self.axis_position(e, (self.mouse_x, self.mouse_y)))
            else {
                self.rejected(e, reason::TARGET_GONE);
                return;
            };
            let old = (self.mouse_x, self.mouse_y);
            (self.mouse_x, self.mouse_y) = point;
            let moved = old != (self.mouse_x, self.mouse_y);
            damage.mouse_moved_this_frame |= moved;
            damage.cursor_dirty |= moved;
            if (self.dragging.is_some() || self.resizing.is_some())
                && self.stream.chrome_device == Some(e.device)
            {
                self.drag_resize(damage);
                self.handled(e);
                return;
            }
            self.hover();
            let targets: BTreeSet<_> = self
                .stream
                .captures
                .iter()
                .filter(|((d, _), _)| *d == e.device)
                .map(|(_, (id, _))| *id)
                .collect();
            let target = targets
                .iter()
                .next()
                .copied()
                .or_else(|| self.content_hit());
            if let Some((id, (x, y))) = target.and_then(|id| self.local(id).map(|xy| (id, xy))) {
                let mut raw = abi::WindowEvent::ZERO;
                raw.kind = abi::event_kind::POINTER_MOVE;
                raw.x = x;
                raw.y = y;
                self.raw(id, e, raw);
            } else {
                self.handled(e);
            }
            self.cursor(damage);
            return;
        }
        if e.kind == kind::REL {
            if let Some((id, (x, y))) = self
                .content_hit()
                .and_then(|id| self.local(id).map(|xy| (id, xy)))
            {
                let mut raw = abi::WindowEvent::ZERO;
                raw.kind = abi::event_kind::SCROLL;
                raw.x = x;
                raw.y = y;
                if e.code == axis::HWHEEL {
                    raw.scroll_x = e.value;
                } else {
                    raw.scroll_y = e.value;
                }
                self.raw(id, e, raw);
            } else {
                self.handled(e);
            }
            return;
        }
        let slot = (e.device, e.code);
        if e.value == 0 {
            if e.code == key::BUTTON_LEFT && self.stream.chrome_device == Some(e.device) {
                self.stream.chrome_device = None;
                if let Some((id, _, _)) = self.dragging.take() {
                    if let Some(w) = self.windows.iter().find(|w| w.window_id == id) {
                        save_window_defaults(w, &mut self.win_defaults);
                    }
                    self.handled(e);
                    self.cursor(damage);
                    return;
                }
                if let Some((id, _, _, _, _, _, _, _)) = self.resizing.take() {
                    if let Some(w) = self.windows.iter().find(|w| w.window_id == id) {
                        save_window_defaults(w, &mut self.win_defaults);
                    }
                    self.handled(e);
                    self.cursor(damage);
                    return;
                }
            }
            if let Some((id, clicks)) = self.stream.captures.remove(&slot) {
                if let Some((x, y)) = self.local(id) {
                    let mut raw = abi::WindowEvent::ZERO;
                    raw.kind = abi::event_kind::POINTER_BUTTON;
                    raw.button = e.code as u32;
                    raw.x = x;
                    raw.y = y;
                    raw.clicks = clicks;
                    self.raw(id, e, raw);
                } else {
                    self.rejected(e, reason::TARGET_GONE);
                }
            } else {
                self.handled(e);
            }
            return;
        }
        if e.value != 1 {
            self.handled(e);
            return;
        }
        // A chromeless owner dismisses on an outside press without activating
        // the obscured normal window.
        if let Some(i) = self
            .windows
            .iter()
            .rposition(|w| w.chromeless && !w.minimized)
        {
            if !self.windows[i].hit_content(self.mouse_x, self.mouse_y) {
                send_to(&self.windows, i, Input::Close);
                self.handled(e);
                return;
            }
        }
        if self.mouse_y < px(TASKBAR_HEIGHT) as i32 {
            self.handled(e);
            return;
        }
        if self.mouse_y >= (self.screen_h - px(APPBAR_HEIGHT)) as i32
            && !self.hit().is_some_and(|i| self.windows[i].chromeless)
        {
            if e.code == key::BUTTON_LEFT {
                if let Some(i) = appbar_hit_test(
                    &self.windows,
                    self.screen_w,
                    self.screen_h,
                    self.mouse_x,
                    self.mouse_y,
                ) {
                    let id = self.windows[i].window_id;
                    if self.target() == Some(id) {
                        self.minimize(id);
                    } else {
                        self.windows[i].minimized = false;
                        self.focus(Some(id), true);
                    }
                }
            }
            self.handled(e);
            return;
        }
        // Resize grabs extend beyond the frame. A higher window obscures a
        // lower window's grab just as it obscures its content.
        let hit = self
            .windows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, w)| {
                !w.minimized
                    && (w.hit_any(self.mouse_x, self.mouse_y)
                        || w.hit_resize_edge(self.mouse_x, self.mouse_y).is_some())
            })
            .map(|(i, _)| i);
        let Some(i) = hit else {
            self.handled(e);
            return;
        };
        let id = self.windows[i].window_id;
        self.focus(Some(id), true);
        let top = self.windows.iter().position(|w| w.window_id == id).unwrap();
        if e.code == key::BUTTON_LEFT {
            if let Some(edge) = self.windows[top].hit_resize_edge(self.mouse_x, self.mouse_y) {
                let w = &mut self.windows[top];
                w.resize_right = matches!(
                    edge,
                    ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
                )
                .then_some(w.x + w.width as i32);
                w.resize_bottom = matches!(
                    edge,
                    ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
                )
                .then_some(w.y + w.height as i32);
                self.stream.chrome_device = Some(e.device);
                self.resizing = Some((
                    id,
                    edge,
                    self.mouse_x,
                    self.mouse_y,
                    w.x,
                    w.y,
                    w.width,
                    w.height,
                ));
                self.handled(e);
                self.cursor(damage);
                return;
            }
            if self.windows[top].hit_title(self.mouse_x, self.mouse_y) {
                if self.windows[top].hit_close_button(self.mouse_x, self.mouse_y) {
                    self.close_dispatch(top);
                } else if self.windows[top].hit_minimize_button(self.mouse_x, self.mouse_y) {
                    self.minimize(id);
                } else {
                    self.stream.chrome_device = Some(e.device);
                    self.dragging = Some((
                        id,
                        self.mouse_x - self.windows[top].x,
                        self.mouse_y - self.windows[top].y,
                    ));
                }
                self.handled(e);
                return;
            }
        }
        if let Some((x, y)) = self
            .local(id)
            .filter(|_| self.windows[top].hit_content(self.mouse_x, self.mouse_y))
        {
            let previous = self.stream.clicks.get(&slot);
            let clicks = if previous.is_some_and(|p| {
                p.target == id
                    && e.time_ns.saturating_sub(p.time) <= 400_000_000
                    && (self.mouse_x - p.x).abs() <= 4
                    && (self.mouse_y - p.y).abs() <= 4
                    && p.count == 1
            }) {
                2
            } else {
                1
            };
            self.stream.clicks.insert(
                slot,
                Click {
                    target: id,
                    time: e.time_ns,
                    x: self.mouse_x,
                    y: self.mouse_y,
                    count: clicks,
                },
            );
            self.stream.captures.insert(slot, (id, clicks));
            let mut raw = abi::WindowEvent::ZERO;
            raw.kind = abi::event_kind::POINTER_BUTTON;
            raw.button = e.code as u32;
            raw.pressed = 1;
            raw.x = x;
            raw.y = y;
            raw.clicks = clicks;
            self.raw(id, e, raw);
        } else {
            self.handled(e);
        }
    }
    fn cursor(&mut self, damage: &mut Damage) {
        let edge = self
            .resizing
            .map(|(_, edge, _, _, _, _, _, _)| edge)
            .or_else(|| {
                self.hit()
                    .and_then(|i| self.windows[i].hit_resize_edge(self.mouse_x, self.mouse_y))
            });
        let shape = match edge {
            Some(ResizeEdge::Top | ResizeEdge::Bottom) => cursor_shape::RESIZE_NS,
            Some(ResizeEdge::Left | ResizeEdge::Right) => cursor_shape::RESIZE_EW,
            Some(ResizeEdge::TopLeft | ResizeEdge::BottomRight) => cursor_shape::RESIZE_NWSE,
            Some(ResizeEdge::TopRight | ResizeEdge::BottomLeft) => cursor_shape::RESIZE_NESW,
            None => cursor_shape::ARROW,
        };
        damage.cursor_dirty |= self.active_cursor_shape != shape;
        self.active_cursor_shape = shape;
    }
}
