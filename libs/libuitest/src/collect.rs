//! Supervisor-owned local collection. No network operation belongs to this phase.
use std::{collections::{BTreeMap, BTreeSet}, path::Path};
use libbreenix::{window::{self, abi}, gpu, error::Error, errno::Errno};
use breenix_suite::suite::collection::Trigger;
use crate::{arena::{Arena, Layout, Slot, SlotRead, TREE_BYTES}, artifacts::Store};

pub struct Evidence {
    layout: Layout,
    pub arena: Option<Arena>,
    held: BTreeMap<u64, abi::WinInfo>,
}
fn registry() -> Result<Vec<abi::WinInfo>, String> {
    let mut rows = vec![abi::WinInfo::default(); 64];
    loop {
        let total = window::list(&mut rows).map_err(|e| e.to_string())?;
        if total <= rows.len() { rows.truncate(total); return Ok(rows); }
        if total > 4096 { return Err("window registry exceeds collection bound".into()); }
        rows.resize(total, abi::WinInfo::default());
    }
}
fn session_owner(pid: u64, supervisor: u64, launched: &BTreeSet<u64>) -> bool {
    let mut next = pid;
    for _ in 0..256 {
        if launched.contains(&next) { return true; }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{next}/stat")) else { return false };
        let Some(parent) = stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1)).and_then(|s| s.parse::<u64>().ok()) else { return false };
        if parent == supervisor { return true; }
        if parent <= 1 || parent == next { return false; }
        next = parent;
    }
    false
}
impl Evidence {
    pub fn new() -> Result<Self, String> {
        let mode = gpu::current_mode().map_err(|e| format!("evidence CURRENT_MODE: {e}"))?;
        Ok(Self { layout: Layout::for_mode(mode.width, mode.height)?, arena: None, held: BTreeMap::new() })
    }
    pub fn configure(&mut self, root: &Path) -> Result<(), String> {
        self.arena = Some(Arena::create(&root.join("evidence.arena"), self.layout)?); Ok(())
    }
    pub fn observe(&mut self, supervisor: u64, launched: &BTreeSet<u64>) -> Result<(), String> {
        for row in registry()? {
            if !session_owner(u64::from(row.owner), supervisor, launched) { continue; }
            if !self.held.contains_key(&row.id) {
                match window::call(abi::op::HOLD_EVIDENCE, &mut abi::WinId { id: row.id }) {
                    Ok(_) => {},
                    // The owner may disappear between LIST and HOLD. Do not claim a hold.
                    Err(Error::Os(Errno::ENOENT)) => continue,
                    Err(e) => return Err(format!("hold window {}: {e}", row.id)),
                }
            }
            self.held.insert(row.id, row);
        }
        Ok(())
    }
    pub fn holds(&self, id: u64) -> bool { self.held.contains_key(&id) }
    pub fn collect(&mut self, suite: &str, case: &str, trigger: Trigger, stderr: &[(u64, u64, Vec<u8>)]) -> String {
        let mut screen = "none:unavailable";
        let mut windows = 0;
        let mut remnants = 0;
        let mut notes = format!("trigger={}\n", trigger.name());
        let result = (|| -> Result<(), String> {
            let mut store = Store::new(Path::new("/var/uitest"), suite, case)?;
            let slots = self.arena.as_ref().map(|arena| [Slot::Screen, Slot::Frame, Slot::Tree].map(|slot| arena.read(slot)));
            let failure_screen = slots.as_ref().is_some_and(|s| matches!(&s[0], Ok(SlotRead::Final { record, bytes }) if valid_screen(record, bytes)));
            // Screen readback is taken before terminating any session participant.
            let mode = gpu::current_mode().map_err(|e| e.to_string())?;
            let layout = Layout::for_mode(mode.width, mode.height)?;
            let mut pixels = vec![0; layout.capacity(Slot::Screen)];
            let mut observation = [0; 1024];
            let mut rb = abi::Readback { dst: pixels.as_mut_ptr() as u64, dst_len: pixels.len() as u64,
                record: observation.as_mut_ptr() as u64, record_len: observation.len() as u32, ..abi::Readback::default() };
            let rc = window::call(abi::op::SCANOUT_READBACK, &mut rb);
            let kind = if u32::from_le_bytes(observation[24..28].try_into().unwrap()) == 3 { window::ObservationKind::Composed } else { window::ObservationKind::Plain };
            if rc.is_ok() && window::validate_scanout_observation(&observation, mode.width, mode.height, kind) {
                save_png(&mut store, if failure_screen { "screen-at-collection.png" } else { "screen.png" }, mode.width, mode.height, pixels)?;
                store.write(if failure_screen { "screen-at-collection.record" } else { "screen.record" }, &observation)?;
                screen = "at-collection";
            } else { notes.push_str(&format!("screen=readback-error:{rc:?}\n")); }
            for (name, source) in [("input.log", "/proc/breenix/input-ledger"), ("composites.log", "/proc/breenix/composites")] {
                store.write(name, &std::fs::read(source).map_err(|e| format!("{source}: {e}"))?)?;
            }
            if let Some(slots) = slots {
                for ((slot, name), snapshot) in [(Slot::Screen, "screen"), (Slot::Frame, "frame"), (Slot::Tree, "tree")].into_iter().zip(slots) {
                    match snapshot {
                        Ok(SlotRead::Free) => {},
                        Ok(SlotRead::Partial(reason)) => notes.push_str(&format!("{name}=partial:{reason}\n")),
                        Err(e) => notes.push_str(&format!("{name}=partial:invalid {e}\n")),
                        Ok(SlotRead::Final { record, bytes }) => {
                            if matches!(slot, Slot::Screen) && !record.oversize && !valid_screen(&record, &bytes) {
                                notes.push_str("screen-slot=partial:invalid-observation\n"); continue;
                            }
                            let meta = format!("check={} window={} commit={} epoch={} composite={} width={} height={} bytes={} oversize={}\n",
                                record.check, record.window, record.commit, record.epoch, record.composite, record.width, record.height, bytes.len(), record.oversize);
                            store.write(&format!("{name}-at-failure.record"), meta.as_bytes())?;
                            if matches!(slot, Slot::Tree) { store.write("tree-at-failure.bin", &bytes)?; }
                            else if record.oversize {
                                store.write(&format!("{name}-at-failure.partial.bgrx"), &bytes)?;
                                notes.push_str(&format!("{name}=partial:oversize\n"));
                            } else {
                                let file = if matches!(slot, Slot::Screen) { "screen.png".into() } else { format!("{name}-at-failure.png") };
                                save_png(&mut store, &file, record.width, record.height, bytes)?;
                                if matches!(slot, Slot::Screen) {
                                    store.write("screen.record", &record.observation)?;
                                    screen = "at-failure";
                                }
                            }
                        }
                    }
                }
            }
            let live: BTreeSet<_> = registry()?.into_iter().map(|w| w.id).collect();
            for (&id, info) in &self.held {
                let dead = !live.contains(&id);
                // Allow a resize between LIST and CAPTURE without truncating a commit.
                let mut pixels = vec![0; 2560 * 1600 * 4];
                let mut tree = vec![0; TREE_BYTES];
                let mut capture = abi::Capture { id, dst: pixels.as_mut_ptr() as u64, dst_len: pixels.len() as u64,
                    tree_dst: tree.as_mut_ptr() as u64, tree_len: tree.len() as u64, ..abi::Capture::default() };
                let mut remnant = dead;
                let mut rc = window::call(if dead { abi::op::READ_REMNANT } else { abi::op::CAPTURE }, &mut capture);
                if matches!(rc, Err(Error::Os(Errno::ENOENT))) && !dead {
                    remnant = true; rc = window::call(abi::op::READ_REMNANT, &mut capture);
                }
                if let Err(e) = rc { notes.push_str(&format!("window-{id}=partial:capture {e}\n")); continue; }
                let bytes = capture.width as usize * capture.height as usize * 4;
                if bytes > pixels.len() { return Err("invalid captured window extent".into()); }
                pixels.truncate(bytes);
                let prefix = if remnant { "remnant" } else { "window" };
                let stem = format!("{prefix}-{id}-{}", capture.commit_id);
                save_png(&mut store, &format!("{stem}.png"), capture.width, capture.height, pixels)?;
                // Keep the page capacity, including the initialized tail beyond the copied tree.
                store.write(&format!("{stem}.tree"), &tree)?;
                store.write(&format!("{stem}.record"), format!("window={id} owner={} commit={} width={} height={}\n", info.owner, capture.commit_id, capture.width, capture.height).as_bytes())?;
                if remnant { remnants += 1; } else { windows += 1; }
            }
            for (pid, dropped, bytes) in stderr {
                store.write(&format!("stderr-{pid}.log"), bytes)?;
                notes.push_str(&format!("stderr-{pid}-dropped={dropped}\n"));
            }
            notes.push_str(&format!("screen={screen}\nwindows={windows}\nremnants={remnants}\n"));
            store.finish(&notes)
        })();
        let artifacts = match result { Ok(()) => "complete", Err(_) => "none:io" };
        // A write failure is an evidence outcome; the case's primary failure stands.
        format!("SUITE {suite} COLLECT {case} trigger={} screen={screen} windows={windows} remnants={remnants} artifacts={artifacts}\n", trigger.name())
    }
    pub fn release_remnants(&mut self) {
        for &id in self.held.keys() { let _ = window::call(abi::op::RELEASE_REMNANT, &mut abi::WinId { id }); }
        self.held.clear();
    }
}
fn save_png(store: &mut Store, name: &str, width: u32, height: u32, mut pixels: Vec<u8>) -> Result<(), String> {
    if pixels.len() != width as usize * height as usize * 4 { return Err("artifact pixel length mismatch".into()); }
    for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
    let png = libimg_encode::encode(&libimg::png::Image { width, height, pixels }).map_err(|e| format!("artifact PNG: {e:?}"))?;
    store.write(name, &png)
}

fn valid_screen(record: &crate::arena::Record, pixels: &[u8]) -> bool {
    let Ok(observation) = <&[u8; 1024]>::try_from(record.observation.as_slice()) else { return false };
    let kind = if u32::from_le_bytes(observation[24..28].try_into().unwrap()) == 3 { window::ObservationKind::Composed } else { window::ObservationKind::Plain };
    !record.oversize && Layout::for_mode(record.width, record.height).is_ok()
        && pixels.len() == record.width as usize * record.height as usize * 4
        && window::validate_scanout_observation(observation, record.width, record.height, kind)
}
