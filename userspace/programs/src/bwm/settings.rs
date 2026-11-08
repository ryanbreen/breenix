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
pub struct Settings { pub font_size: f32, pub clock_visible: bool }
impl Default for Settings {
    fn default() -> Self { Self { font_size: 14.0, clock_visible: true } }
}
impl Settings {
    /// Optional unique keys, key=value lines, # comments, final newline required.
    pub fn parse(text: &str) -> Result<Self, String> {
        if !text.is_empty() && !text.ends_with('\n') { return Err("incomplete bwm.conf line".into()); }
        let mut value = Self::default();
        let (mut font, mut clock) = (false, false);
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
