//! System font configuration reader.
//!
//! Reads `/etc/fonts.conf` for OS-level font defaults. Apps use this to
//! load the user's preferred fonts without hardcoding paths.
//!
//! Config format (key=value, # comments):
//! ```text
//! mono.font=/usr/share/fonts/DejaVuSansMono.ttf
//! mono.size=14
//! ```

const CONFIG_PATH: &str = "/etc/fonts.conf";

/// Default monospace font path (fallback when config is missing).
const DEFAULT_MONO_FONT: &str = "/usr/share/fonts/DejaVuSansMono.ttf";

/// Default monospace font size.
const DEFAULT_MONO_SIZE: f32 = 10.0;

/// Parsed system font configuration.
pub struct FontConfig {
    pub mono_path: String,
    pub mono_size: f32,
}

impl FontConfig {
    /// Read system font config, falling back to defaults for missing values.
    pub fn load() -> Self {
        let mut config = Self {
            mono_path: String::from(DEFAULT_MONO_FONT),
            mono_size: DEFAULT_MONO_SIZE,
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
                        _ => {}
                    }
                }
            }
        }

        // Safety: ensure size is valid (catches NaN, 0.0, sub-minimum)
        if !(config.mono_size >= 6.0) { config.mono_size = DEFAULT_MONO_SIZE; }

        config
    }

    /// Load the configured monospace font bytes and size.
    pub fn load_mono(&self) -> Option<(Vec<u8>, f32)> {
        let data = std::fs::read(&self.mono_path).ok()?;
        Some((data, self.mono_size))
    }
}

/// Parse a float from a string without pulling in std float parsing
/// (works with simple decimal numbers like "14.0" or "16").
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
