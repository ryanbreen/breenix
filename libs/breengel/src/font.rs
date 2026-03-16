//! System font configuration and hot-reload watcher.
//!
//! Reads `/etc/fonts.conf` for OS-level font defaults and detects changes
//! so apps can hot-swap fonts when the user changes them (e.g. via bfontpicker).
//!
//! # Usage
//!
//! ```rust,no_run
//! use breengel::FontWatcher;
//!
//! let mut watcher = FontWatcher::new();
//! let mut font = watcher.load_font().unwrap();
//! let mut size = watcher.mono_size();
//!
//! loop {
//!     if let Some(new_font) = watcher.poll() {
//!         font = new_font;
//!         size = watcher.mono_size();
//!     }
//!     // ... render with font ...
//! }
//! ```

use libfont::{Font, CachedFont};

const CONFIG_PATH: &str = "/etc/fonts.conf";
const DEFAULT_MONO_FONT: &str = "/usr/share/fonts/DejaVuSansMono.ttf";
const DEFAULT_MONO_SIZE: f32 = 10.0;
const DEFAULT_DISPLAY_FONT: &str = "/usr/share/fonts/DejaVuSans.ttf";
const DEFAULT_DISPLAY_SIZE: f32 = 14.0;

/// Polls the system font config for changes and provides loaded fonts.
pub struct FontWatcher {
    mono_path: String,
    mono_size: f32,
    poll_counter: u32,
    poll_interval: u32,
}

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

        // Final safety: ensure sizes are valid (catches NaN, 0.0, negative, sub-minimum)
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

impl FontWatcher {
    /// Create a new watcher. Reads the current config immediately.
    pub fn new() -> Self {
        let config = FontConfig::load();
        Self {
            mono_path: config.mono_path,
            mono_size: config.mono_size,
            poll_counter: 0,
            poll_interval: 20,
        }
    }

    /// Set how many calls to `poll()` between config file checks.
    /// Default is 20 (at 50ms sleep = ~1 second).
    pub fn set_poll_interval(&mut self, interval: u32) {
        self.poll_interval = interval.max(1);
    }

    /// The current configured font path.
    pub fn mono_path(&self) -> &str {
        &self.mono_path
    }

    /// The current configured font size.
    pub fn mono_size(&self) -> f32 {
        self.mono_size
    }

    /// Load the current font from config. Call once at startup.
    /// Returns `CachedFont` or `None` if the font file is missing.
    /// Use `mono_size()` to get the pixel size (returned separately to avoid
    /// f32-in-tuple ABI corruption on aarch64 with opt-level="z").
    pub fn load_font(&self) -> Option<CachedFont> {
        let data = std::fs::read(&self.mono_path).ok()?;
        let font = Font::parse(&data).ok()?;
        Some(CachedFont::new(font, 256))
    }

    /// Check if the font config has changed. Call once per frame.
    ///
    /// Returns `Some(new_font)` if the font changed, `None` otherwise.
    /// Use `mono_size()` to get the current pixel size (returned separately
    /// to avoid f32-in-tuple ABI corruption on aarch64 with opt-level="z").
    /// Internally rate-limited by `poll_interval` so re-reading the config file
    /// doesn't happen every frame.
    pub fn poll(&mut self) -> Option<CachedFont> {
        self.poll_counter += 1;
        if self.poll_counter < self.poll_interval {
            return None;
        }
        self.poll_counter = 0;

        let config = FontConfig::load();
        if config.mono_path == self.mono_path && config.mono_size == self.mono_size {
            return None;
        }

        self.mono_path = config.mono_path;
        self.mono_size = config.mono_size;

        let data = std::fs::read(&self.mono_path).ok()?;
        let font = Font::parse(&data).ok()?;
        Some(CachedFont::new(font, 256))
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
