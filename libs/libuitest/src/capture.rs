//! Captures write into the case's evidence arena before any comparison runs.
use crate::{arena::{Arena, Layout, Record, Slot, SlotRead}, capture_marker::{self, Generation}, compare::{self, Format, Frame, Options}, FailureClass, ScenarioOutcome};
use libbreenix::{gpu, window::{self, abi, ObservationKind}};
use libgfx::surface::Surface;

fn byte_len(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 { return Err("empty capture".into()); }
    (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| "capture extent overflow".into())
}
/// One CAPTURE call asks for both destinations; the kernel holds its front's
/// pixel and tree pins through both copies. Do not fetch MAP_TREE afterwards.
pub fn window(arena: &mut Arena, check: &str, id: u64) -> Result<Record, String> {
    window_result(arena, check, id).map_err(|e| e.to_string())
}
/// Keep a device refusal typed so cases can distinguish ENOENT from other errors.
#[derive(Debug)]
pub struct WindowError { pub syscall: Option<libbreenix::error::Error>, pub detail: String }
impl WindowError {
    pub fn arena(detail: String) -> Self { Self { syscall: None, detail } }
}
impl core::fmt::Display for WindowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { f.write_str(&self.detail) }
}
pub fn window_result(arena: &mut Arena, check: &str, id: u64) -> Result<Record, WindowError> {
    let mut syscall = None;
    let result = arena.write_frame_tree(|pixels, tree| {
        let captured = window::capture(id, pixels, Some(tree)).map_err(|e| {
            let detail = format!("window capture {id}: {e}");
            syscall = Some(e);
            detail
        })?;
        let len = byte_len(captured.width, captured.height)?;
        if len > pixels.len() { return Err("captured extent exceeds arena".into()); }
        Ok((Record { check: check.into(), window: id, commit: captured.commit_id,
            width: captured.width, height: captured.height, ..Default::default() }, len))
    });
    result.map_err(|detail| WindowError { syscall, detail })
}
pub use libbreenix::window::readback_until;
/// Require the caller's observation kind, rather than accepting whatever kind
/// the device returned. The shared E21 validator judges backend completions.
pub fn screen(arena: &mut Arena, check: &str, kind: ObservationKind) -> Result<Record, String> {
    let deadline = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?
        .as_nanos() as u64 + crate::settle::BOUND_NS;
    let mode = gpu::current_mode().map_err(|e| format!("capture CURRENT_MODE: {e}"))?;
    let len = byte_len(mode.width, mode.height)?;
    let mut writer = arena.begin(Slot::Screen)?;
    let pixels = writer.bytes_mut();
    if len > pixels.len() { return Err("screen exceeds arena".into()); }
    let mut observation = [0; 1024];
    let mut rb = abi::Readback { dst: pixels.as_mut_ptr() as u64, dst_len: len as u64,
        record: observation.as_mut_ptr() as u64, record_len: 1024, ..Default::default() };
    readback_until(&mut rb, deadline).map_err(|e| format!("screen capture: {e}"))?;
    if !window::validate_scanout_observation(&observation, mode.width, mode.height, kind) {
        return Err("screen capture: invalid scanout observation".into());
    }
    use gpu::abi::record as off;
    let quad = |at| u64::from_le_bytes(observation[at..at+8].try_into().unwrap());
    if quad(off::SERIAL) != rb.serial { return Err("screen capture serial mismatch".into()); }
    let record = Record { check: check.into(), width: mode.width, height: mode.height,
        composite: quad(off::COMPOSITE_SERIAL), commit: quad(off::COMMIT),
        observation: observation.to_vec(), ..Default::default() };
    screen_windows(&record)?;
    writer.publish(record.clone(), len)?;
    Ok(record)
}
/// Take a temporary observation without consuming the case's evidence slot.
pub fn screen_fresh(path: &std::path::Path, layout: Layout, check: &str, kind: ObservationKind) -> Result<(Record, Vec<u8>), String> {
    let mut arena = Arena::create(path, layout)?;
    let result = (|| {
        let record = screen(&mut arena, check, kind)?;
        match arena.read(Slot::Screen)? {
            SlotRead::Final { bytes, .. } => Ok((record, bytes)),
            _ => Err("screen observation incomplete".into()),
        }
    })();
    drop(arena);
    let cleanup = std::fs::remove_file(path).map_err(|e| e.to_string());
    match (result, cleanup) { (Err(e), _) => Err(e), (Ok(_), Err(e)) => Err(e), (Ok(value), Ok(())) => Ok(value) }
}
/// The v2 observation has 16 rows of (window, commit, render fence, generation). Refuse malformed or
/// duplicate identities instead of truncating the list.
pub fn screen_windows(record: &Record) -> Result<Vec<(u64, u64)>, String> {
    use gpu::abi::record as off;
    let bytes: &[u8; 1024] = record.observation.as_slice().try_into().map_err(|_| "screen observation length")?;
    let count = u32::from_le_bytes(bytes[off::WINDOW_COUNT..off::WINDOW_COUNT+4].try_into().unwrap()) as usize;
    if count > 16 { return Err("screen observation window count".into()); }
    let mut windows = Vec::with_capacity(count);
    for i in 0..count {
        let at = off::WINDOWS + i*32;
        let id = u64::from_le_bytes(bytes[at..at+8].try_into().unwrap());
        let commit = u64::from_le_bytes(bytes[at+8..at+16].try_into().unwrap());
        if id == 0 || windows.iter().any(|&(seen, _)| seen == id) { return Err("screen observation window identity".into()); }
        windows.push((id, commit));
    }
    Ok(windows)
}
pub fn surface(arena: &mut Arena, check: &str, surface: &Surface) -> Result<Record, String> {
    if surface.format() != libgfx::surface::Format::BGRX { return Err("surface capture requires canonical BGRX".into()); }
    let width = u32::try_from(surface.width()).map_err(|_| "surface width overflow")?;
    let height = u32::try_from(surface.height()).map_err(|_| "surface height overflow")?;
    let len = byte_len(width, height)?;
    let mut writer = arena.begin(Slot::Frame)?;
    let pixels = writer.bytes_mut();
    if len > pixels.len() { return Err("surface exceeds arena".into()); }
    surface.read(&mut pixels[..len], width as usize*4).map_err(|e| format!("surface readback: {e}"))?;
    let record = Record { check: check.into(), width, height, ..Default::default() };
    writer.publish(record.clone(), len)?;
    Ok(record)
}
/// Authored CPU surfaces have no window commit or tree.
pub fn pixels(arena: &mut Arena, check: &str, width: u32, height: u32, pixels: &[u8]) -> Result<Record, String> {
    let len = byte_len(width, height)?;
    if pixels.len() != len { return Err("surface pixel length mismatch".into()); }
    let mut writer = arena.begin(Slot::Frame)?;
    if len > writer.bytes_mut().len() { return Err("surface exceeds arena".into()); }
    writer.bytes_mut()[..len].copy_from_slice(pixels);
    let record = Record { check: check.into(), width, height, ..Default::default() };
    writer.publish(record.clone(), len)?;
    Ok(record)
}
/// The marker is the root value of a NUL-terminated UTF-8 node, not a later
/// metadata read. This small node precedes 22.5's full widget-tree writer.
pub fn root(generation: Generation, width: u32, height: u32) -> Vec<u8> {
    let mut bytes = serde_json::json!({"key":"root","parent":null,"role":"window","name":"capture",
        "value":{"commit":generation.commit,"producer":generation.producer},
        "bounds":[0,0,width,height],"clip":[0,0,width,height],"flags":["enabled","visible"]}).to_string().into_bytes();
    bytes.push(0);
    bytes
}
pub fn root_marker(tree: &[u8]) -> Result<[u8; capture_marker::MARKER_BYTES], String> {
    let end = tree.iter().position(|&b| b == 0).ok_or("unterminated capture tree")?;
    let root: serde_json::Value = serde_json::from_slice(&tree[..end]).map_err(|e| format!("capture tree: {e}"))?;
    if root["key"] != "root" || root["role"] != "window" || !root["parent"].is_null() { return Err("capture tree lacks root window".into()); }
    let commit = root["value"]["commit"].as_u64().ok_or("capture root commit")?;
    let producer = root["value"]["producer"].as_u64().and_then(|v| u32::try_from(v).ok()).ok_or("capture root producer")?;
    Ok(capture_marker::encode(Generation { commit, producer }))
}
/// Convert the engine's actual verdict into an inner outcome, preserving the
/// check name, first coordinate and both colors across executor transport.
pub fn compare_outcome(check: &str, want: &[u8], got: &[u8], width: usize, height: usize) -> Result<ScenarioOutcome, String> {
    let frame = |bytes| Frame::new(bytes, width, height, width*4, Format::Bgrx).map_err(|e| format!("capture comparison: {e:?}"));
    let verdict = compare::compare(frame(want)?, frame(got)?, Options::default()).map_err(|e| format!("capture comparison: {e:?}"))?;
    match verdict.failure_class() {
        None => Ok(ScenarioOutcome::pass()),
        Some(FailureClass::PixelMismatch) => {
            let first = verdict.comparison.first.ok_or("mismatch lacks first pixel")?;
            Ok(ScenarioOutcome::fail(FailureClass::PixelMismatch, check,
                &crate::messages::pixel_details(first.x, first.y, first.actual, first.expected)))
        }
        Some(_) => Err("unexpected capture comparison class".into()),
    }
}
