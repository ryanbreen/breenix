//! Pure semantic nodes and the restored integer HSV picker mapping.
use libbui::tree::{Flag, Node, Role};
use serde_json::{json, Value};

fn node(key: &str, role: Role, bounds: [i64; 4], value: Value) -> Node {
    Node { key: key.into(), parent: Some("root".into()), role, name: key.into(), value,
        bounds, clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] }
}

pub fn canvas(bounds: [i64; 4], width: usize, height: usize, digest: [u8; 32]) -> Node {
    let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    node("canvas", Role::Canvas, bounds, json!({"width":width,"height":height,"sha256":sha256}))
}

pub struct Status<'a> {
    pub tool: &'a str, pub size: &'a str, pub color: [u8; 3],
    pub undo: usize, pub redo: usize, pub dirty: bool,
    pub file: Option<&'a str>, pub message: &'a str,
}
pub fn status(bounds: [i64; 4], s: &Status<'_>) -> Node {
    node("status", Role::Status, bounds, json!({"tool":s.tool,"size":s.size,"color":s.color,
        "undo":s.undo,"redo":s.redo,"dirty":s.dirty,"file":s.file,"message":s.message}))
}
pub fn tools(bounds: [[i64; 4]; 7], selected: &str) -> Vec<Node> {
    ["pencil", "brush", "line", "rectangle", "circle", "fill", "eraser"].iter().enumerate().map(|(i, name)| {
        let mut n = node(&format!("tool-{name}"), Role::Button, bounds[i], Value::Null);
        if *name == selected { n.flags.push(Flag::Selected); }
        n
    }).collect()
}
pub fn hue(bounds: [i64; 4]) -> Node { node("hue", Role::Canvas, bounds, Value::Null) }
pub fn sv(bounds: [i64; 4]) -> Node { node("sv", Role::Canvas, bounds, Value::Null) }

/// For a point inside the hue bar, hue is floor(x * 360 / width).
pub fn hue_at(x: usize, width: usize) -> u16 { (x * 360 / width) as u16 }
/// For a point inside the square, saturation rises left to right and value
/// falls top to bottom: floor(x*255/size), floor((size-1-y)*255/size).
pub fn sv_at(x: usize, y: usize, size: usize) -> (u8, u8) {
    ((x * 255 / size) as u8, ((size - 1 - y) * 255 / size) as u8)
}
/// Convert degrees and 0..255 saturation/value with the original integer rounding.
pub fn hsv_to_rgb(hue: u16, sat: u8, val: u8) -> [u8; 3] {
    if sat == 0 { return [val; 3]; }
    let (h, s, v) = ((hue % 360) as u32, sat as u32, val as u32);
    let sector = h * 6 / 360;
    let f = h * 6 - sector * 360;
    let p = v * (255 - s) / 255;
    let q = v * (255 - s * f / 360) / 255;
    let t = v * (255 - s * (360 - f) / 360) / 255;
    let (r, g, b) = match sector { 0 => (v,t,p), 1 => (q,v,p), 2 => (p,v,t),
        3 => (p,q,v), 4 => (t,p,v), _ => (v,p,q) };
    [r as u8, g as u8, b as u8]
}

/// Semantic collaboration state supplied by the session model.
pub trait Collaboration {
    fn collab_value(&self) -> Value;
}
pub fn collab(bounds: [i64; 4], model: &impl Collaboration) -> Node {
    node("collab", Role::Status, bounds, model.collab_value())
}
