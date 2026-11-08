//! Live relative-pointer backend; no architecture-specific input path.
use super::{click_target, exposed_region, home_then_move, Backend, Delivery, Resolved, Target};
use crate::{
    tree::{find, Query, Tree},
    Session,
};
use libbreenix::{
    errno::Errno,
    error::Error,
    input::{self, axis, key, kind, Event, Reader},
    memory,
    window::{self, abi, abi::op},
};
use libbui::tree::Flag;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn status() -> Result<abi::CompositorStatus, String> {
    let mut s = abi::CompositorStatus::default();
    window::call(op::COMPOSITOR_STATUS, &mut s).map_err(|e| e.to_string())?;
    Ok(s)
}
fn pause() -> Result<(), String> {
    libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())
}
fn outcome(serial: u64) -> Result<Option<(String, u64, String)>, String> {
    let ledger =
        std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
    Ok(super::ledger_entry(&ledger, serial)?.map(|row|
        (row.disposition.into(), row.window, row.reason.into())))
}

/// Wait for the addressed client's commit, including commit_no_change.
pub fn delivered(serial: u64) -> Result<Delivery, String> {
    wait_delivery(serial, None, false)
}

fn wait_delivery(serial: u64, wanted: Option<u64>, allow_wm: bool) -> Result<Delivery, String> {
    let end = Instant::now() + Duration::from_secs(2);
    let mut target = wanted;
    loop {
        if let Some((name, window, reason)) = outcome(serial)? {
            if name == "delivered" && window != 0 && wanted.is_none_or(|id| id == window) {
                target = Some(window);
                let (commit, meta) = window::read_meta(window)
                    .map_err(|e| format!("input serial={serial} window={window} metadata: {e}"))?;
                if commit != 0 && super::client_processed(meta.processed_serial, serial) {
                    return Ok(Delivery {serial, window});
                }
            } else if allow_wm && wanted.is_none() && name == "wm-handled" {
                return Ok(Delivery {serial, window: 0});
            } else {
                return Err(format!("input serial={serial} disposition={name} window={window} expected={wanted:?} reason={reason}"));
            }
        }
        if Instant::now() >= end {
            return Err(format!("input unprocessed serial={serial} window={target:?}"));
        }
        pause()?;
    }
}

/// Check the requested source set, including gaps hidden by later terminal rows.
pub fn dispositions(serials: &[u64]) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let ledger = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
        let Some(serial) = super::first_missing_disposition(&ledger, serials)? else { return Ok(()); };
        if Instant::now() >= end { return Err(format!("missing-disposition serial={serial}")); }
        pause()?;
    }
}
/// Wait for a known source serial's terminal disposition. Client delivery also
/// needs its processed commit watermark, checked separately below.
pub fn wait_pointer_idle(serial: u64) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some((name, _, reason)) = outcome(serial)? {
            if name != "delivered" && name != "wm-handled" {
                return Err(format!("input serial={serial} {name} reason={reason}"));
            }
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!(
                "input timeout serial={serial} waiting=source-disposition"
            ));
        }
        pause()?;
    }
}
struct Mapping(abi::MapTree);
impl Mapping {
    fn new(id: u64) -> Result<Self, String> {
        let mut m = abi::MapTree {
            id,
            ..Default::default()
        };
        window::call(op::MAP_TREE, &mut m).map_err(|e| e.to_string())?;
        Ok(Self(m))
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = memory::munmap(self.0.addr as *mut u8, self.0.len as usize);
    }
}

pub struct LivePointer<'a> {
    session: &'a mut Session,
    reader: Reader,
    pending: Vec<Event>,
    desktop: Mapping,
    watched: BTreeMap<u64, Mapping>,
    last: Option<Target>,
    serials: Vec<u64>,
}
impl<'a> LivePointer<'a> {
    pub fn new(session: &'a mut Session) -> Result<Self, String> {
        let desktop = Mapping::new(0)?;
        let reader = Reader::open(true).map_err(|e| e.to_string())?;
        Ok(Self {
            session,
            reader,
            pending: Vec::new(),
            desktop,
            watched: BTreeMap::new(),
            last: None,
            serials: Vec::new(),
        })
    }
    pub fn session(&mut self) -> &mut Session {
        self.session
    }
    pub fn serials(&self) -> &[u64] { &self.serials }
    /// Snapshot the compositor's committed desktop projection at one scene.
    pub fn desktop_tree(&self) -> Result<Tree, String> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let before = status()?;
            let tree = unsafe { breengel::tree_page::Page::from_mapping(
                self.desktop.0.addr as *mut u8, self.desktop.0.len as usize)? }.snapshot()?;
            let after = status()?;
            if before.claim_epoch == after.claim_epoch && before.scene_version == after.scene_version {
                if let Some(tree) = tree {
                    if tree.root.value["scene_version"].as_u64() == Some(after.scene_version) { return Ok(tree); }
                }
            }
            if Instant::now() >= end { return Err("input waiting=desktop-tree-and-scene".into()); }
            pause()?;
        }
    }
    /// Queue a physical key and return its kernel-authored source event.
    /// The caller chooses when to wait for delivered() or a WM disposition.
    pub fn send_key(&mut self, code: u16, pressed: bool) -> Result<Event, String> {
        if pressed { self.session.raw_press(code)?; } else { self.session.raw_release(code)?; }
        self.source_event(self.session.input_devices()[0], kind::KEY, code, i32::from(pressed))
    }
    pub fn wm_key(&mut self, code: u16, pressed: bool) -> Result<u64, String> {
        let event = self.send_key(code, pressed)?;
        wait_pointer_idle(event.serial)?;
        Ok(event.serial)
    }
    pub fn info(&self, id: u64) -> Result<abi::WinInfo, String> {
        let mut infos = vec![abi::WinInfo::default(); 64];
        let n = window::list(&mut infos).map_err(|e| e.to_string())?;
        infos
            .get(..n)
            .ok_or("window registry overflow")?
            .iter()
            .find(|w| w.id == id)
            .copied()
            .ok_or_else(|| format!("input target window={id} missing"))
    }
    /// Attach before requesting a later app commit; attachment alone is passive.
    pub fn watch(&mut self, id: u64) -> Result<(), String> {
        if self.watched.contains_key(&id) {
            return Ok(());
        }
        self.watched.insert(id, Mapping::new(id)?);
        let info = self.info(id)?;
        self.move_to((info.x + 10, info.y + 30))?;
        let end = Instant::now() + Duration::from_secs(2);
        while window::read_meta(id)
            .map_err(|e| e.to_string())?
            .1
            .tree_serializations
            == 0
        {
            if Instant::now() >= end {
                return Err(format!("input tree window={id} not committed after attach"));
            }
            pause()?;
        }
        Ok(())
    }
    fn capture(&self, id: u64) -> Result<(Tree, u64), String> {
        let info = self.info(id)?;
        let mut pixels = vec![
            0;
            (info.width as usize)
                .checked_mul(info.height as usize)
                .and_then(|n| n.checked_mul(4))
                .ok_or("input capture overflow")?
        ];
        let mut bytes = vec![0; abi::limits::MAX_TREE_KIB as usize * 1024];
        let c = window::capture(id, &mut pixels, Some(&mut bytes))
            .map_err(|e| format!("input capture window={id}: {e}"))?;
        Ok((Tree::parse(&bytes)?, c.commit_id))
    }
    pub fn tree(&self, id: u64) -> Result<Tree, String> {
        self.capture(id).map(|(tree, _)| tree)
    }
    fn source(&mut self, device: u32, kind: u16, code: u16, value: i32) -> Result<u64, String> {
        self.source_event(device,kind,code,value).map(|e|e.serial)
    }
    fn source_event(&mut self, device: u32, kind: u16, code: u16, value: i32) -> Result<Event, String> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(index) = self.pending.iter().position(|e| {
                e.device == device && e.kind == kind && e.code == code && e.value == value
            }) {
                return Ok(self.pending.remove(index));
            }
            let mut bytes = [0; 32768];
            match self.reader.read(&mut bytes) {
                Ok(records) => {
                    for record in records {
                        let e = record.map_err(|e| e.to_string())?.event;
                        if self.session.input_devices().contains(&e.device) {
                            self.serials.push(e.serial);
                            if e.kind != input::kind::SYN { self.pending.push(e); }
                        }
                    }
                }
                Err(Error::Os(Errno::EAGAIN)) => {}
                Err(e) => return Err(e.to_string()),
            }
            if Instant::now() >= end {
                return Err(format!(
                    "input source timeout device={device} kind={kind} code={code} value={value}"
                ));
            }
            pause()?;
        }
    }
    fn processed(&self, serial: u64, wanted: Option<u64>) -> Result<Option<Delivery>, String> {
        let done = wait_delivery(serial, wanted, true)?;
        Ok((done.window != 0).then_some(done))
    }
    fn move_batch(
        &mut self,
        (x, y): (i32, i32),
        wanted: Option<u64>,
    ) -> Result<Option<Delivery>, String> {
        self.session.raw_motion(x, y)?;
        let sx = self.source(self.session.input_devices()[1], kind::REL, axis::X, x)?;
        let sy = self.source(self.session.input_devices()[1], kind::REL, axis::Y, y)?;
        // bwm normalizes contiguous X/Y to one position. One source moves it;
        // the stationary companion receives WM_HANDLED rather than a second move.
        let dx = self.processed(sx, None)?;
        let dy = self.processed(sy, None)?;
        let delivery = dy.or(dx);
        if let Some(id) = wanted {
            if delivery.is_none_or(|d| d.window != id) {
                return Err(format!(
                    "input motion serials={sx},{sy} delivery={delivery:?} expected window={id}"
                ));
            }
        }
        Ok(delivery)
    }
}
impl Backend for LivePointer<'_> {
    fn resolve(&mut self, target: &Target) -> Result<Resolved, String> {
        self.watch(target.window)?;
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let before = status()?;
            let desktop = unsafe {
                breengel::tree_page::Page::from_mapping(
                    self.desktop.0.addr as *mut u8,
                    self.desktop.0.len as usize,
                )?
            }
            .snapshot()?;
            if let Some(desktop) = desktop {
                let info = self.info(target.window)?;
                let (tree, commit) = self.capture(target.window)?;
                let newest = window::read_meta(target.window)
                    .map_err(|e| e.to_string())?
                    .0;
                let after = status()?;
                if before.claim_epoch == after.claim_epoch
                    && before.scene_version == after.scene_version
                    && desktop.root.value["scene_version"].as_u64() == Some(after.scene_version)
                    && newest == commit
                {
                    let win = desktop
                        .children
                        .iter()
                        .find(|n| n.value["id"].as_u64() == Some(target.window))
                        .ok_or("target absent from desktop tree")?;
                    let z = win.value["z"].as_u64().ok_or("desktop window z missing")?;
                    let covers: Vec<_> = desktop
                        .children
                        .iter()
                        .filter(|n| {
                            n.flags.contains(&Flag::Visible)
                                && n.value["z"].as_u64().is_some_and(|other| other > z)
                        })
                        .map(|n| n.bounds)
                        .collect();
                    let visible = if win.flags.contains(&Flag::Visible) {
                        exposed_region(
                            [
                                i64::from(info.x),
                                i64::from(info.y),
                                i64::from(info.width),
                                i64::from(info.height),
                            ],
                            desktop.root.bounds,
                            &covers,
                        )
                    } else {
                        Vec::new()
                    };
                    let node = find(
                        &tree,
                        &Query {
                            key: Some(&target.key),
                            ..Default::default()
                        },
                    )?;
                    let point = click_target(node, (info.x, info.y), &visible)?;
                    self.last = Some(target.clone());
                    return Ok(Resolved {
                        window: target.window,
                        point,
                        scene: after.scene_version,
                        commit,
                    });
                }
            }
            if Instant::now() >= end {
                return Err(format!(
                    "input target {} window={} waiting=newest-tree-and-scene",
                    target.key, target.window
                ));
            }
            pause()?;
        }
    }
    fn move_to(&mut self, point: (i32, i32)) -> Result<(), String> {
        let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
        for delta in home_then_move((mode.width as usize, mode.height as usize), point)? {
            self.move_batch(delta, None)?;
        }
        Ok(())
    }
    fn confirm(&mut self, resolved: &Resolved) -> Result<(), String> {
        let target = self.last.clone().ok_or("no resolved target")?;
        let current = self.resolve(&target)?;
        let mut hit = abi::HitTest {
            x: resolved.point.0,
            y: resolved.point.1,
            ..Default::default()
        };
        window::call(op::HIT_TEST, &mut hit).map_err(|e| e.to_string())?;
        if hit.id != resolved.window
            || hit.scene_version != resolved.scene
            || current.scene != resolved.scene
            || current.point != resolved.point
            || current.commit < resolved.commit
        {
            return Err(format!(
                "input target {} hit-test window={} scene={} expected window={} scene={}",
                target.key, hit.id, hit.scene_version, resolved.window, resolved.scene
            ));
        }
        Ok(())
    }
    fn button(&mut self, pressed: bool, window: u64) -> Result<Delivery, String> {
        if pressed {
            self.session.raw_press(key::BUTTON_LEFT)?;
        } else {
            self.session.raw_release(key::BUTTON_LEFT)?;
        }
        let serial = self.source(
            self.session.input_devices()[1],
            kind::KEY,
            key::BUTTON_LEFT,
            i32::from(pressed),
        )?;
        self.processed(serial, Some(window))?
            .ok_or("button not delivered".into())
    }
    fn motion(&mut self, delta: (i32, i32), window: u64) -> Result<Delivery, String> {
        self.move_batch(delta, Some(window))?
            .ok_or("drag step did not move pointer".into())
    }
    fn wheel(&mut self, dy: i32, window: u64) -> Result<Delivery, String> {
        self.session.raw_wheel(dy)?;
        let serial = self.source(self.session.input_devices()[1], kind::REL, axis::WHEEL, dy)?;
        self.processed(serial, Some(window))?
            .ok_or("wheel not delivered".into())
    }
}

impl super::KeyboardBackend for LivePointer<'_> {
    fn key(&mut self, code: u16, pressed: bool, window: u64) -> Result<Delivery, String> {
        if pressed {
            self.session.raw_press(code)?;
        } else {
            self.session.raw_release(code)?;
        }
        let serial = self.source(
            self.session.input_devices()[0],
            kind::KEY,
            code,
            i32::from(pressed),
        )?;
        self.processed(serial, Some(window))?
            .ok_or("key not delivered".into())
    }
    fn text(&mut self, text: &str, window: u64) -> Result<Delivery, String> {
        self.session.raw_text(text)?;
        let serial = self.source(
            self.session.input_devices()[0],
            kind::TEXT,
            0,
            text.len() as i32,
        )?;
        self.processed(serial, Some(window))?
            .ok_or("text not delivered".into())
    }
    fn focus(&mut self, target: &Target) -> Result<(), String> {
        let resolved = self.resolve(target)?;
        let tree = self.tree(target.window)?;
        let query = Query {
            key: Some(&target.key),
            ..Default::default()
        };
        if !find(&tree, &query)?.flags.contains(&Flag::Enabled) {
            return Err(format!(
                "input-focus: {} cannot take focus (disabled)",
                target.key
            ));
        }
        self.move_to(resolved.point)?;
        self.confirm(&resolved)?;
        let press = self.button(true, target.window);
        let release = self.button(false, target.window);
        press?;
        release?;
        if !find(&self.tree(target.window)?, &query)?
            .flags
            .contains(&Flag::Focused)
        {
            return Err(format!("input-focus: {} cannot take focus", target.key));
        }
        Ok(())
    }
}
