//! Content routing harvested from dfc2e318, with press edges from 135a8294.
//! No transport calls: the caller publishes focus before delivering these notices.
use super::stack::Stack;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct Content {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}
impl Content {
    fn contains(self, (x, y): (i32, i32)) -> bool {
        let (x, y) = (x as i64 - self.x as i64, y as i64 - self.y as i64);
        x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64
    }
    fn notice(self, kind: NoticeKind, (x, y): (i32, i32)) -> Notice {
        Notice {
            id: self.id,
            kind,
            x: x.saturating_sub(self.x),
            y: y.saturating_sub(self.y),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoticeKind {
    Move,
    Button { code: u16, pressed: bool },
    Wheel(i32),
    Enter,
    Leave,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Notice {
    pub id: u64,
    pub kind: NoticeKind,
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy)]
pub(super) enum Action {
    Move,
    Button { code: u16, value: i32 },
    Wheel(i32),
    Cancel,
}
pub(super) fn motion_action(_previous: (i32, i32), _current: (i32, i32)) -> Action { Action::Move }
pub(super) struct Routed {
    pub focus: Option<(Option<u64>, Option<u64>)>,
    pub notices: Vec<Notice>,
    pub press_edge: bool,
    pub release_edge: bool,
}
#[derive(Default)]
struct Device {
    held: u32,
    captured: u32,
    grab: Option<Content>,
    point: (i32, i32),
}
pub(super) struct Router {
    devices: BTreeMap<u32, Device>,
    hover: Option<(u32, Content)>,
    point: (i32, i32),
}
impl Router {
    pub(super) fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            hover: None,
            point: (0, 0),
        }
    }
    pub(super) fn grabbed(&self, device: u32) -> bool {
        self.devices.get(&device).is_some_and(|d| d.grab.is_some())
    }
    /// Cancel disappearing or minimized targets using their last known geometry.
    pub(super) fn reconcile(&mut self, stack: &Stack, contents: &[Content]) -> Vec<Notice> {
        let mut notices = Vec::new();
        for d in self.devices.values_mut() {
            let visible = |id| {
                stack
                    .state(id)
                    .filter(|(_, minimized, _)| !minimized)
                    .and_then(|_| contents.iter().find(|c| c.id == id))
                    .copied()
            };
            if let Some(grab) = d.grab {
                d.grab = visible(grab.id);
                if d.grab.is_none() {
                    notices.push(grab.notice(NoticeKind::Cancel, d.point));
                    d.captured = 0;
                }
            }
        }
        if let Some((device, hover)) = self.hover {
            self.hover = stack
                .state(hover.id)
                .filter(|(_, minimized, _)| !minimized)
                .and_then(|_| contents.iter().find(|c| c.id == hover.id))
                .copied()
                .map(|c| (device, c));
            if self.hover.is_none() {
                notices.push(hover.notice(NoticeKind::Leave, self.point));
            }
        }
        notices
    }
    /// `content_enabled` leaves chrome/appbar ownership with the existing caller.
    pub(super) fn route(
        &mut self,
        stack: &mut Stack,
        contents: &[Content],
        device: u32,
        point: (i32, i32),
        content_enabled: bool,
        action: Action,
    ) -> Routed {
        let mut out = Routed {
            focus: None,
            notices: self.reconcile(stack, contents),
            press_edge: false,
            release_edge: false,
        };
        if matches!(action, Action::Cancel) {
            if let Some(d) = self.devices.remove(&device) {
                if let Some(grab) = d.grab {
                    out.notices.push(grab.notice(NoticeKind::Cancel, point));
                }
            }
            if let Some((owner, hover)) = self.hover {
                if owner == device {
                    out.notices.push(hover.notice(NoticeKind::Leave, point));
                    self.hover = None;
                }
            }
            return out;
        }
        let hit = content_enabled
            .then(|| {
                stack.order().iter().rev().find_map(|&id| {
                    if stack.minimized(id) {
                        return None;
                    }
                    contents
                        .iter()
                        .copied()
                        .find(|c| c.id == id && c.contains(point))
                })
            })
            .flatten();
        let d = self.devices.entry(device).or_default();
        d.point = point;
        if self.hover.map(|(_, c)| c.id) != hit.map(|c| c.id) {
            if let Some((_, old)) = self.hover {
                out.notices.push(old.notice(NoticeKind::Leave, point));
            }
            if let Some(next) = hit {
                out.notices.push(next.notice(NoticeKind::Enter, point));
            }
        }
        self.hover = hit.map(|c| (device, c));
        self.point = point;
        match action {
            Action::Move => {
                if let Some(target) = d.grab.or(hit) {
                    out.notices.push(target.notice(NoticeKind::Move, point));
                }
            }
            Action::Wheel(delta) => {
                if let Some(target) = hit {
                    out.notices
                        .push(target.notice(NoticeKind::Wheel(delta), point));
                }
            }
            Action::Button { code, value } if (0x110..=0x112).contains(&code) => {
                let bit = 1 << (code - 0x110);
                if value == 0 {
                    out.release_edge = d.held & bit != 0;
                    d.held &= !bit;
                    if d.captured & bit != 0 {
                        if let Some(target) = d.grab {
                            out.notices.push(target.notice(
                                NoticeKind::Button {
                                    code,
                                    pressed: false,
                                },
                                point,
                            ));
                        }
                        d.captured &= !bit;
                        if d.captured == 0 {
                            d.grab = None;
                        }
                    }
                } else if value == 1 && d.held & bit == 0 {
                    d.held |= bit;
                    out.press_edge = true;
                    if let Some(target) = d.grab.or(hit) {
                        if d.grab.is_none() {
                            out.focus = Some(stack.raise(target.id));
                        }
                        d.grab = Some(target);
                        d.captured |= bit;
                        out.notices.push(target.notice(
                            NoticeKind::Button {
                                code,
                                pressed: true,
                            },
                            point,
                        ));
                    }
                }
            }
            Action::Button { .. } | Action::Cancel => {}
        }
        // Discard idle device state instead of retaining removed virtual devices.
        if d.held == 0 && d.grab.is_none() {
            self.devices.remove(&device);
        }
        out
    }
}

#[derive(Clone, Copy)]
pub(super) enum Axis {
    AbsoluteX(i32),
    AbsoluteY(i32),
    RelativeX(i32),
    RelativeY(i32),
    Boundary,
}
/// f48a2564: contiguous axes from one device share their final batch position.
/// Keep a slot per source record so admission still accounts for each serial.
pub(super) fn batch_positions(
    samples: &[(u32, Axis)],
    mut point: (i32, i32),
    (width, height): (usize, usize),
) -> Vec<Option<(i32, i32)>> {
    let mut positions = vec![None; samples.len()];
    let mut at = 0;
    while at < samples.len() {
        if matches!(samples[at].1, Axis::Boundary) {
            at += 1;
            continue;
        }
        let start = at;
        let device = samples[at].0;
        while at < samples.len() && samples[at].0 == device {
            match samples[at].1 {
                Axis::AbsoluteX(value) => point.0 = value,
                Axis::AbsoluteY(value) => point.1 = value,
                Axis::RelativeX(value) => point.0 = point.0.saturating_add(value),
                Axis::RelativeY(value) => point.1 = point.1.saturating_add(value),
                Axis::Boundary => break,
            }
            point.0 = point.0.clamp(0, width.saturating_sub(1) as i32);
            point.1 = point.1.clamp(0, height.saturating_sub(1) as i32);
            at += 1;
        }
        positions[start..at].fill(Some(point));
    }
    positions
}
