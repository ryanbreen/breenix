//! Startup configuration. Paths use the same root rule as BreenGL's fonts.
use std::{fs, path::{Path, PathBuf}};

pub struct Paths { pub hotkeys: PathBuf, pub bwm: PathBuf }
impl Paths {
    pub fn from_root(root: Option<&Path>) -> Self {
        let root = root.unwrap_or(Path::new("/"));
        Self { hotkeys: root.join("etc/hotkeys.conf"), bwm: root.join("etc/bwm.conf") }
    }
    pub fn environment() -> Self {
        let root = std::env::var_os("BREENIX_CONFIG_ROOT");
        Self::from_root(root.as_deref().map(Path::new))
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Settings { pub font_size: f32, pub clock_visible: bool, pub display_mode: Option<(u32, u32)>, pub display_scale: Option<u32>, pub warnings: Vec<String> }
impl Default for Settings {
    fn default() -> Self { Self { font_size: 14.0, clock_visible: true, display_mode: None, display_scale: None, warnings: Vec::new() } }
}
impl Settings {
    /// Optional unique keys, key=value lines, # comments, final newline required.
    pub fn parse(text: &str) -> Result<Self, String> {
        if !text.is_empty() && !text.ends_with('\n') { return Err("incomplete bwm.conf line".into()); }
        let mut value = Self::default();
        let (mut font, mut clock, mut mode, mut scale) = (false, false, false, false);
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') { continue; }
            let (key, data) = line.split_once('=').ok_or("bwm.conf expects key=value")?;
            match key {
                "font.size" if !font => {
                    let n: f32 = data.parse().map_err(|_| "invalid font.size")?;
                    if data.trim() != data || !n.is_finite() || !(6.0..=72.0).contains(&n) { return Err("font.size outside 6..72".into()); }
                    value.font_size = n; font = true;
                }
                "clock.visible" if !clock => {
                    value.clock_visible = match data { "true" => true, "false" => false, _ => return Err("invalid clock.visible".into()) };
                    clock = true;
                }
                "display.mode" if !mode => {
                    mode = true;
                    value.display_mode = data.split_once('x').and_then(|(w, h)| {
                        if w.is_empty() || h.is_empty() || !w.bytes().all(|b| b.is_ascii_digit()) || !h.bytes().all(|b| b.is_ascii_digit()) { return None; }
                        let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
                        (w > 0 && h > 0).then_some((w, h))
                    });
                    if value.display_mode.is_none() { value.warnings.push(format!("invalid display.mode: {data}")); }
                }
                "display.scale" if !scale => {
                    scale = true;
                    value.display_scale = match data { "1" => Some(1), "2" => Some(2), _ => None };
                    if value.display_scale.is_none() { value.warnings.push(format!("invalid display.scale: {data}")); }
                }
                _ => return Err(format!("unknown or duplicate bwm.conf key: {key}")),
            }
        }
        Ok(value)
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// Replace display settings while preserving all other configuration bytes.
pub fn with_display(text: &str, mode: (u32, u32), scale: u32) -> String {
    let mut result = String::new();
    let (mut found_mode, mut found_scale) = (false, false);
    for line in text.split_inclusive('\n') {
        if line.starts_with("display.mode=") {
            result.push_str(&format!("display.mode={}x{}\n", mode.0, mode.1));
            found_mode = true;
        } else if line.starts_with("display.scale=") {
            result.push_str(&format!("display.scale={scale}\n"));
            found_scale = true;
        } else {
            result.push_str(line);
        }
    }
    if !found_mode || !found_scale {
        if !result.is_empty() && !result.ends_with('\n') { result.push('\n'); }
        if !found_mode { result.push_str(&format!("display.mode={}x{}\n", mode.0, mode.1)); }
        if !found_scale { result.push_str(&format!("display.scale={scale}\n")); }
    }
    result
}
