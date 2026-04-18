//! System font configuration and hot-reload watcher.
//!
//! Reads `/etc/fonts.conf` for OS-level font defaults and detects changes
//! so apps can hot-swap fonts when the user changes them (e.g. via bfontpicker).
//!
//! Font management is handled automatically by [`Window`](crate::Window).
//! Apps receive [`Event::FontChanged`](crate::Event::FontChanged) when the
//! system font config changes, and access fonts via `win.take_mono_font()`.

use libfont::{Font, CachedFont};

const CONFIG_PATH: &str = "/etc/fonts.conf";
const DEFAULT_MONO_FONT: &str = "/usr/share/fonts/DejaVuSansMono.ttf";
const DEFAULT_MONO_SIZE: f32 = 10.0;
const DEFAULT_DISPLAY_FONT: &str = "/usr/share/fonts/DejaVuSans.ttf";
const DEFAULT_DISPLAY_SIZE: f32 = 14.0;

/// Parsed system font configuration.
pub struct FontConfig {
    pub mono_path: String,
    pub mono_size: f32,
    pub display_path: String,
    pub display_size: f32,
}

impl FontConfig {
    /// Read system font config, falling back to defaults for missing values.
    pub fn load() -> Self {
        let mut config = Self {
            mono_path: String::from(DEFAULT_MONO_FONT),
            mono_size: DEFAULT_MONO_SIZE,
            display_path: String::from(DEFAULT_DISPLAY_FONT),
            display_size: DEFAULT_DISPLAY_SIZE,
        };

        if let Ok(contents) = std::fs::read_to_string(CONFIG_PATH) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "mono.font" => config.mono_path = String::from(value),
                        "mono.size" => {
                            if let Ok(size) = parse_f32(value) {
                                if size >= 6.0 && size <= 72.0 {
                                    config.mono_size = size;
                                }
                            }
                        }
                        "display.font" => config.display_path = String::from(value),
                        "display.size" => {
                            if let Ok(size) = parse_f32(value) {
                                if size >= 6.0 && size <= 72.0 {
                                    config.display_size = size;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if !(config.mono_size >= 6.0) { config.mono_size = DEFAULT_MONO_SIZE; }
        if !(config.display_size >= 6.0) { config.display_size = DEFAULT_DISPLAY_SIZE; }

        config
    }

    /// Load the configured monospace font bytes.
    pub fn load_mono(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.mono_path).ok()
    }

    /// Load the configured display font bytes.
    pub fn load_display(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.display_path).ok()
    }
}

/// Internal font watcher — polls `/etc/fonts.conf` for changes and
/// owns the loaded `CachedFont` instances. Used by `Window` internally.
pub(crate) struct FontWatcher {
    mono_path: String,
    mono_size: f32,
    display_path: String,
    display_size: f32,
    mono_font: Option<CachedFont>,
    display_font: Option<CachedFont>,
    poll_counter: u32,
    poll_interval: u32,
}

fn load_cached_font(path: &str) -> Option<CachedFont> {
    let data = std::fs::read(path).ok()?;
    let font = Font::parse(&data).ok()?;
    Some(CachedFont::new(font, 256))
}

impl FontWatcher {
    pub(crate) fn new() -> Self {
        let config = FontConfig::load();
        let mono_font = load_cached_font(&config.mono_path);
        let display_font = load_cached_font(&config.display_path);
        Self {
            mono_path: config.mono_path,
            mono_size: config.mono_size,
            display_path: config.display_path,
            display_size: config.display_size,
            mono_font,
            display_font,
            poll_counter: 0,
            poll_interval: 20,
        }
    }

    pub(crate) fn disabled() -> Self {
        Self {
            mono_path: String::from(DEFAULT_MONO_FONT),
            mono_size: DEFAULT_MONO_SIZE,
            display_path: String::from(DEFAULT_DISPLAY_FONT),
            display_size: DEFAULT_DISPLAY_SIZE,
            mono_font: None,
            display_font: None,
            poll_counter: 0,
            poll_interval: 0,
        }
    }

    pub(crate) fn set_poll_interval(&mut self, interval: u32) {
        self.poll_interval = interval.max(1);
    }

    pub(crate) fn disable_polling(&mut self) {
        self.poll_interval = 0;
    }

    pub(crate) fn mono_path(&self) -> &str {
        &self.mono_path
    }

    pub(crate) fn mono_size(&self) -> f32 {
        self.mono_size
    }

    pub(crate) fn display_path(&self) -> &str {
        &self.display_path
    }

    pub(crate) fn display_size(&self) -> f32 {
        self.display_size
    }

    /// Take the mono font out of the watcher, leaving None.
    pub(crate) fn take_mono_font(&mut self) -> Option<CachedFont> {
        self.mono_font.take()
    }

    /// Return a mono font to the watcher.
    pub(crate) fn put_mono_font(&mut self, font: Option<CachedFont>) {
        self.mono_font = font;
    }

    /// Take the display font out of the watcher, leaving None.
    pub(crate) fn take_display_font(&mut self) -> Option<CachedFont> {
        self.display_font.take()
    }

    /// Return a display font to the watcher.
    pub(crate) fn put_display_font(&mut self, font: Option<CachedFont>) {
        self.display_font = font;
    }

    /// Poll the config file for changes. Returns true if fonts changed.
    /// Internally rate-limited by `poll_interval`.
    pub(crate) fn poll(&mut self) -> bool {
        if self.poll_interval == 0 {
            return false;
        }

        self.poll_counter += 1;
        if self.poll_counter < self.poll_interval {
            return false;
        }
        self.poll_counter = 0;

        let config = FontConfig::load();
        let mono_changed = config.mono_path != self.mono_path
            || config.mono_size != self.mono_size;
        let display_changed = config.display_path != self.display_path
            || config.display_size != self.display_size;

        if !mono_changed && !display_changed {
            return false;
        }

        if mono_changed {
            self.mono_path = config.mono_path;
            self.mono_size = config.mono_size;
            self.mono_font = load_cached_font(&self.mono_path);
        }
        if display_changed {
            self.display_path = config.display_path;
            self.display_size = config.display_size;
            self.display_font = load_cached_font(&self.display_path);
        }
        true
    }
}

fn parse_f32(s: &str) -> Result<f32, ()> {
    let mut result: f32 = 0.0;
    let mut decimal = false;
    let mut decimal_place: f32 = 0.1;

    for c in s.bytes() {
        if c == b'.' {
            if decimal { return Err(()); }
            decimal = true;
        } else if c >= b'0' && c <= b'9' {
            let digit = (c - b'0') as f32;
            if decimal {
                result += digit * decimal_place;
                decimal_place *= 0.1;
            } else {
                result = result * 10.0 + digit;
            }
        } else {
            return Err(());
        }
    }

    Ok(result)
}
