//! Atomic font configuration reloads, paced by the UI clock (BreenGL §9).
use libgfx::ttf_font::{CachedFont, Font};
use std::io::Read;
use std::os::unix::fs::MetadataExt;

const DEFAULT_MONO_FONT: &str = "/usr/share/fonts/DejaVuSansMono.ttf";
const DEFAULT_DISPLAY_FONT: &str = "/usr/share/fonts/DejaVuSans.ttf";
const CHECK_NS: u64 = 250_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    pub mono_path: String,
    pub mono_size: f32,
    pub display_path: String,
    pub display_size: f32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontWatchError {
    Config,
    Font(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontChange {
    pub mono: bool,
    pub display: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            mono_path: DEFAULT_MONO_FONT.into(),
            mono_size: 10.0,
            display_path: DEFAULT_DISPLAY_FONT.into(),
            display_size: 14.0,
        }
    }
}
fn config_path() -> std::path::PathBuf {
    std::env::var_os("BREENIX_CONFIG_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/".into())
        .join("etc/fonts.conf")
}
impl FontConfig {
    /// Strict syntax: four unique known keys and a complete final line.
    pub fn parse(contents: &str) -> Result<Self, FontWatchError> {
        if !contents.ends_with('\n') {
            return Err(FontWatchError::Config);
        }
        let mut values: [Option<&str>; 4] = [None; 4];
        for line in contents.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(FontWatchError::Config)?;
            if value.trim() != value { return Err(FontWatchError::Config); }
            let index = match key {
                "mono.font" => 0,
                "mono.size" => 1,
                "display.font" => 2,
                "display.size" => 3,
                _ => return Err(FontWatchError::Config),
            };
            if values[index].is_some() || value.is_empty() || value.contains('\0') {
                return Err(FontWatchError::Config);
            }
            values[index] = Some(value);
        }
        let [Some(mono), Some(ms), Some(display), Some(ds)] = values else {
            return Err(FontWatchError::Config);
        };
        let size = |s: &str| -> Result<f32, FontWatchError> {
            let mut parts = s.split('.');
            let digits = |p: &str| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit());
            if !digits(parts.next().unwrap_or(""))
                || parts.next().is_some_and(|p| !digits(p)) || parts.next().is_some() {
                return Err(FontWatchError::Config);
            }
            let n = s.parse::<f32>().map_err(|_| FontWatchError::Config)?;
            if !n.is_finite() || !(6.0..=72.0).contains(&n) {
                return Err(FontWatchError::Config);
            }
            Ok(n)
        };
        Ok(Self {
            mono_path: mono.into(),
            mono_size: size(ms)?,
            display_path: display.into(),
            display_size: size(ds)?,
        })
    }
    pub fn try_load() -> Result<Self, FontWatchError> {
        load_config().map(|(config, _)| config)
    }

    /// Startup compatibility for callers without a watcher. Reloads use strict errors.
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }
    pub fn load_mono(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.mono_path).ok()
    }
    pub fn load_display(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.display_path).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    inode: u64,
    size: u64,
    seconds: i64,
    nanos: i64,
}
impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            inode: meta.ino(),
            size: meta.len(),
            seconds: meta.mtime(),
            nanos: meta.mtime_nsec(),
        }
    }
    fn read(path: &str) -> Result<Self, FontWatchError> {
        std::fs::metadata(path)
            .map(|m| Self::of(&m))
            .map_err(|_| FontWatchError::Font(path.into()))
    }
}
fn load_config() -> Result<(FontConfig, Stamp), FontWatchError> {
    let mut file = std::fs::File::open(config_path()).map_err(|_| FontWatchError::Config)?;
    let stamp = Stamp::of(&file.metadata().map_err(|_| FontWatchError::Config)?);
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|_| FontWatchError::Config)?;
    let after = Stamp::of(&file.metadata().map_err(|_| FontWatchError::Config)?);
    if stamp != after || contents.len() as u64 != stamp.size {
        return Err(FontWatchError::Config);
    }
    Ok((FontConfig::parse(&contents)?, stamp))
}
fn load_font(path: &str) -> Result<(CachedFont, Stamp), FontWatchError> {
    // Read bytes and identity through one descriptor so rename cannot mix two files.
    let error = || FontWatchError::Font(path.into());
    let mut file = std::fs::File::open(path).map_err(|_| error())?;
    let stamp = Stamp::of(&file.metadata().map_err(|_| error())?);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| error())?;
    if bytes.len() as u64 != stamp.size {
        return Err(error());
    }
    if Stamp::of(&file.metadata().map_err(|_| error())?) != stamp {
        return Err(error());
    }
    let font = Font::parse(&bytes).map_err(|_| error())?;
    Ok((CachedFont::new(font, 256), stamp))
}

pub struct FontWatcher {
    config: FontConfig,
    mono_font: Option<CachedFont>,
    display_font: Option<CachedFont>,
    mono_stamp: Option<Stamp>,
    config_stamp: Option<Stamp>,
    display_stamp: Option<Stamp>,
    last_check: Option<u64>,
    interval_ns: u64,
    enabled: bool,
}
impl Default for FontWatcher {
    fn default() -> Self {
        Self::new()
    }
}
impl FontWatcher {
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|_| {
            let mut watcher = Self::disabled();
            watcher.enabled = true;
            watcher.last_check = Some(crate::clock::now_ns());
            if let Ok((font, stamp)) = load_font(DEFAULT_MONO_FONT) {
                watcher.mono_font = Some(font);
                watcher.mono_stamp = Some(stamp);
            }
            if let Ok((font, stamp)) = load_font(DEFAULT_DISPLAY_FONT) {
                watcher.display_font = Some(font);
                watcher.display_stamp = Some(stamp);
            }
            watcher
        })
    }
    pub fn try_new() -> Result<Self, FontWatchError> {
        let (config, config_stamp) = load_config()?;
        let (mono, mono_stamp) = load_font(&config.mono_path)?;
        let (display, display_stamp) = load_font(&config.display_path)?;
        Ok(Self {
            config,
            mono_font: Some(mono),
            display_font: Some(display),
            mono_stamp: Some(mono_stamp),
            display_stamp: Some(display_stamp),
            config_stamp: Some(config_stamp),
            last_check: Some(crate::clock::now_ns()),
            interval_ns: CHECK_NS,
            enabled: true,
        })
    }
    pub fn disabled() -> Self {
        Self {
            config: FontConfig::default(),
            mono_font: None,
            display_font: None,
            mono_stamp: None,
            display_stamp: None,
            config_stamp: None,
            last_check: None,
            interval_ns: CHECK_NS,
            enabled: false,
        }
    }
    /// Legacy poll-count hints cannot weaken the 250 ms minimum.
    pub fn set_poll_interval(&mut self, interval_ms: u32) {
        self.interval_ns = u64::from(interval_ms).max(250) * 1_000_000;
        self.enabled = true;
    }
    pub fn disable_polling(&mut self) {
        self.enabled = false;
    }
    pub fn mono_path(&self) -> &str {
        &self.config.mono_path
    }
    pub fn mono_size(&self) -> f32 {
        self.config.mono_size
    }
    pub fn display_path(&self) -> &str {
        &self.config.display_path
    }
    pub fn display_size(&self) -> f32 {
        self.config.display_size
    }
    pub fn mono_font(&mut self) -> Option<&mut CachedFont> {
        self.mono_font.as_mut()
    }
    pub fn display_font(&mut self) -> Option<&mut CachedFont> {
        self.display_font.as_mut()
    }
    pub fn take_mono_font(&mut self) -> Option<CachedFont> {
        self.mono_font.take()
    }
    /// Return a taken font unless a reload has already filled its slot.
    pub fn put_mono_font(&mut self, font: Option<CachedFont>) {
        if self.mono_font.is_none() {
            self.mono_font = font;
        }
    }
    pub fn take_display_font(&mut self) -> Option<CachedFont> {
        self.display_font.take()
    }
    /// A client returning an older font cannot overwrite a published reload.
    pub fn put_display_font(&mut self, font: Option<CachedFont>) {
        if self.display_font.is_none() {
            self.display_font = font;
        }
    }
    pub fn poll(&mut self) -> Result<Option<FontChange>, FontWatchError> {
        if !self.enabled {
            return Ok(None);
        }
        let now = crate::clock::now_ns();
        if self
            .last_check
            .is_some_and(|last| now.saturating_sub(last) < self.interval_ns)
        {
            return Ok(None);
        }
        self.last_check = Some(now);
        let stamp = std::fs::metadata(config_path())
            .map(|m| Stamp::of(&m))
            .map_err(|_| FontWatchError::Config)?;
        let (config, config_stamp) = if self.config_stamp == Some(stamp) {
            (self.config.clone(), stamp)
        } else {
            load_config()?
        };
        let ms = Stamp::read(&config.mono_path)?;
        let ds = Stamp::read(&config.display_path)?;
        let mono = config.mono_path != self.config.mono_path
            || config.mono_size != self.config.mono_size
            || self.mono_stamp != Some(ms);
        let display = config.display_path != self.config.display_path
            || config.display_size != self.config.display_size
            || self.display_stamp != Some(ds);
        if !mono && !display {
            self.config_stamp = Some(config_stamp);
            return Ok(None);
        }
        // Validate every changed font before publishing any change. A bad second
        // font cannot leave the first half of a configuration installed.
        let new_mono = if mono {
            Some(load_font(&config.mono_path)?)
        } else {
            None
        };
        let new_display = if display {
            Some(load_font(&config.display_path)?)
        } else {
            None
        };
        if let Some((font, stamp)) = new_mono {
            self.mono_font = Some(font);
            self.mono_stamp = Some(stamp);
        }
        if let Some((font, stamp)) = new_display {
            self.display_font = Some(font);
            self.display_stamp = Some(stamp);
        }
        self.config = config;
        self.config_stamp = Some(config_stamp);
        Ok(Some(FontChange { mono, display }))
    }
}
