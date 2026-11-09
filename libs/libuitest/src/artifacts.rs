//! Local evidence publication and the boot-wide on-disk artifact budget.
use crate::arena::{Arena, Slot, SlotRead};
use libpixcheck::{sha256, Frame, Options};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
pub const CAP_BYTES: usize = 32 * 1024 * 1024;

pub const BUNDLE_LIMIT: usize = 2 * 1024 * 1024;

/// UTART001 stores each file's name, length, SHA-256 and exact bytes.
pub struct Bundle {
    pub bytes: Vec<u8>,
    pub screen_dropped: bool,
}
pub fn label(case: &str) -> String {
    let hex: String = sha256(case.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("uitest-{}", &hex[..16])
}
pub fn bundle(entries: &[(String, Vec<u8>)]) -> Result<Bundle, String> {
    let mut files: Vec<_> = entries.iter().collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.iter().any(|(name, _)| !component(name)) || files.windows(2).any(|p| p[0].0 == p[1].0)
    {
        return Err("invalid bundle name".into());
    }
    let headers: Vec<_> = files
        .iter()
        .map(|(name, data)| {
            let hash: String = sha256(data).iter().map(|b| format!("{b:02x}")).collect();
            format!("file={name} bytes={} sha256={hash}\n", data.len())
        })
        .collect();
    let size = |drop_screen: bool| {
        files
            .iter()
            .zip(&headers)
            .filter(|((name, _), _)| !drop_screen || name != "screen.png")
            .try_fold(9usize, |size, ((_, data), header)| {
                size.checked_add(header.len())?.checked_add(data.len())
            })
    };
    let screen_dropped = size(false).is_none_or(|n| n > BUNDLE_LIMIT)
        && files.iter().any(|(name, _)| name == "screen.png");
    let size = size(screen_dropped)
        .filter(|&n| n <= BUNDLE_LIMIT)
        .ok_or("bundle-bound")?;
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(b"UTART001\n");
    for ((name, data), header) in files.into_iter().zip(headers) {
        if screen_dropped && name == "screen.png" {
            continue;
        }
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(data);
    }
    Ok(Bundle {
        bytes,
        screen_dropped,
    })
}

/// Leave the runner's 1.5 s collection reserve and 250 ms for returning the result.
pub fn upload_budget_ms(case_ms_left: u64) -> u64 {
    case_ms_left.saturating_sub(1750).min(5000)
}

/// Filesystem claims survive forks; ambiguous attempts consume their slot.
pub fn reserve_upload(root: &Path, label: &str) -> Result<(), String> {
    if label.len() != 23
        || !label.starts_with("uitest-")
        || !label[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("label".into());
    }
    directory(root).map_err(|e| e.to_string())?;
    let claims = root.join(".uploads");
    directory(&claims).map_err(|e| e.to_string())?;
    match fs::create_dir(claims.join(label)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err("already-attempted".into())
        }
        Err(_) => return Err("slot-io".into()),
    }
    for slot in 0..16 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(claims.join(slot.to_string()))
        {
            Ok(mut file) => {
                file.write_all(label.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|_| "slot-io")?;
                File::open(&claims)
                    .and_then(|d| d.sync_all())
                    .map_err(|_| "slot-io")?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("slot-io".into()),
        }
    }
    Err("slots".into())
}

fn component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn directory(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            directory(parent)?;
        }
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {
            if let Some(p) = path.parent() {
                File::open(p)?.sync_all()?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(std::io::Error::other("artifact path is not a directory"));
    }
    Ok(())
}

/// Each successful write is durable before its name is advertised to a reader.
pub trait Publish {
    fn write_temp(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
    fn sync_temp(&mut self) -> Result<(), String>;
    fn rename(&mut self, name: &str) -> Result<(), String>;
    fn sync_directory(&mut self) -> Result<(), String>;
}
pub fn publish(writer: &mut dyn Publish, name: &str, bytes: &[u8]) -> Result<(), String> {
    writer.write_temp(name, bytes)?;
    writer.sync_temp()?;
    writer.rename(name)?;
    writer.sync_directory()
}
struct Disk<'a> {
    path: &'a Path,
    file: Option<File>,
    temp: PathBuf,
}
impl Publish for Disk<'_> {
    fn write_temp(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.temp = self.path.join(format!("{name}.tmp"));
        self.file = Some(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.temp)
                .map_err(|e| e.to_string())?,
        );
        self.file
            .as_mut()
            .unwrap()
            .write_all(bytes)
            .map_err(|e| e.to_string())
    }
    fn sync_temp(&mut self) -> Result<(), String> {
        self.file
            .as_ref()
            .unwrap()
            .sync_all()
            .map_err(|e| e.to_string())
    }
    fn rename(&mut self, name: &str) -> Result<(), String> {
        self.file.take();
        fs::rename(&self.temp, self.path.join(name)).map_err(|e| e.to_string())
    }
    fn sync_directory(&mut self) -> Result<(), String> {
        File::open(self.path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
}
impl Drop for Disk<'_> {
    fn drop(&mut self) {
        self.file.take();
        if !self.temp.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveError {
    Cap,
    Io,
}
impl SaveError {
    pub fn record(self) -> &'static str {
        match self {
            Self::Cap => "none:cap",
            Self::Io => "none:io",
        }
    }
}
pub trait CheckWriter {
    fn path(&self) -> &Path;
    fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), SaveError>;
    /// Freeze the first saved mismatch before its FRAME advertises omissions.
    fn prepare_retention(&mut self) -> Result<Option<bool>, SaveError> {
        Ok(None)
    }
}
// The runner reaps the case before starting its collector. Count the retained
// files rather than keeping a process-local counter that fork would duplicate.
fn usage(path: &Path) -> Result<u64, String> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.path().symlink_metadata().map_err(|e| e.to_string())?;
        let n = if meta.is_dir() {
            usage(&entry.path())?
        } else if meta.is_file() {
            meta.len()
        } else {
            return Err("unexpected artifact entry".into());
        };
        bytes = bytes.checked_add(n).ok_or("artifact usage overflow")?;
    }
    Ok(bytes)
}
pub struct Store {
    root: PathBuf,
    path: PathBuf,
}
impl Store {
    /// Start a fresh case directory. The collector uses open, preserving checks.
    pub fn new(root: &Path, suite: &str, case: &str) -> Result<Self, String> {
        let store = Self::open(root, suite, case)?;
        for entry in fs::read_dir(&store.path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("unexpected artifact entry".into());
            }
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
        File::open(&store.path)
            .and_then(|d| d.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(store)
    }
    pub fn open(root: &Path, suite: &str, case: &str) -> Result<Self, String> {
        let parts: Vec<_> = case.split('/').collect();
        if !component(suite) || parts.len() != 2 || !parts.iter().all(|s| component(s)) {
            return Err("invalid artifact case path".into());
        }
        directory(root).map_err(|e| e.to_string())?;
        let mut path = root.to_path_buf();
        for part in std::iter::once(suite).chain(parts.iter().copied()) {
            path.push(part);
            directory(&path).map_err(|e| e.to_string())?;
        }
        Ok(Self {
            root: root.into(),
            path,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn contains(&self, name: &str) -> bool {
        component(name) && self.path.join(name).is_file()
    }
    fn write_checked(&mut self, name: &str, bytes: &[u8]) -> Result<(), SaveError> {
        if !component(name) || name.ends_with(".tmp") {
            return Err(SaveError::Io);
        }
        let used = usage(&self.root).map_err(|_| SaveError::Io)?;
        if used
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > CAP_BYTES as u64)
        {
            return Err(SaveError::Cap);
        }
        let mut writer = Disk {
            path: &self.path,
            file: None,
            temp: PathBuf::new(),
        };
        publish(&mut writer, name, bytes).map_err(|_| SaveError::Io)?;
        Ok(())
    }
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.write_checked(name, bytes)
            .map_err(|e| e.record().into())
    }
    pub fn finish(&mut self, summary: &str) -> Result<(), String> {
        // Include check files published by the case before this collector opened.
        let mut entries = String::new();
        for entry in fs::read_dir(&self.path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "invalid artifact name")?;
            if name == "collection.txt" || name.ends_with(".tmp") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
            let hash: String = sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect();
            entries.push_str(&format!(
                "file={name} bytes={} sha256={hash}\n",
                bytes.len()
            ));
        }
        self.write(
            "collection.txt",
            format!("UTLOCAL1\n{summary}{entries}").as_bytes(),
        )
    }
}
impl CheckWriter for Store {
    fn path(&self) -> &Path {
        &self.path
    }
    fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), SaveError> {
        self.write_checked(name, bytes)
    }
}
/// Delay directory creation until a check actually needs to retain evidence.
pub struct Local {
    root: PathBuf,
    suite: String,
    case: String,
    path: PathBuf,
    store: Option<Store>,
    frozen: Option<bool>,
}
impl Local {
    pub fn new(root: &Path, suite: &str, case: &str) -> Self {
        Self {
            root: root.into(),
            suite: suite.into(),
            case: case.into(),
            path: root.join(suite).join(case),
            store: None,
            frozen: None,
        }
    }
}
impl CheckWriter for Local {
    fn path(&self) -> &Path {
        &self.path
    }
    fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), SaveError> {
        if self.store.is_none() {
            self.store =
                Some(Store::new(&self.root, &self.suite, &self.case).map_err(|_| SaveError::Io)?);
        }
        CheckWriter::write(self.store.as_mut().unwrap(), name, bytes)
    }
    fn prepare_retention(&mut self) -> Result<Option<bool>, SaveError> {
        if let Some(dropped) = self.frozen {
            return Ok(Some(dropped));
        }
        let store = self.store.as_mut().ok_or(SaveError::Io)?;
        let bundle = freeze(store).map_err(|_| SaveError::Io)?;
        self.frozen = Some(bundle.screen_dropped);
        Ok(self.frozen)
    }
}
/// Publish one immutable diagnostic snapshot. Later collection has its own manifest.
pub fn freeze(store: &mut Store) -> Result<Bundle, String> {
    if store.contains("retention.bundle") {
        return Err("already-frozen".into());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(store.path()).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "artifact name")?;
        if name.ends_with(".tmp") {
            continue;
        }
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("artifact type".into());
        }
        entries.push((name, fs::read(entry.path()).map_err(|e| e.to_string())?));
    }
    let snapshot = bundle(&entries)?;
    store.write("retention.bundle", &snapshot.bytes)?;
    Ok(snapshot)
}

pub fn save_all() -> bool {
    fs::read_to_string("/etc/breenix/uitest-save").is_ok_and(|s| s.trim() == "all")
}
pub fn png(frame: Frame<'_>) -> Result<Vec<u8>, String> {
    libimg_encode::encode(&libimg::png::Image {
        width: frame.width as u32,
        height: frame.height as u32,
        pixels: frame.canonical(),
    })
    .map_err(|e| format!("artifact PNG: {e:?}"))
}
/// Magenta only where the comparison counted a difference; dim the reference elsewhere.
pub fn diff(
    want: Frame<'_>,
    got: Frame<'_>,
    options: Options<'_>,
) -> Result<libimg::png::Image, String> {
    libpixcheck::compare(want, got, options).map_err(|e| format!("artifact diff: {e:?}"))?;
    let encloses = |r: libpixcheck::Rect, m: libpixcheck::Rect| {
        m.x >= r.x && m.y >= r.y && m.x + m.w <= r.x + r.w && m.y + m.h <= r.y + r.h
    };
    let mut pixels = Vec::with_capacity(want.width * want.height * 4);
    for y in 0..want.height {
        for x in 0..want.width {
            let domain = if options.regions.is_empty() {
                !options.masks.iter().any(|m| m.rect.contains(x, y))
            } else {
                options.regions.iter().any(|r| {
                    r.contains(x, y)
                        && !options
                            .masks
                            .iter()
                            .any(|m| encloses(*r, m.rect) && m.rect.contains(x, y))
                })
            };
            let a = want.pixel(x, y).unwrap();
            let b = got.pixel(x, y).unwrap();
            let differs = domain && (0..3).any(|c| a[c].abs_diff(b[c]) > options.tolerance.channel);
            pixels.extend_from_slice(&if differs {
                [255, 0, 255, 255]
            } else {
                [a[0] / 4, a[1] / 4, a[2] / 4, 255]
            });
        }
    }
    Ok(libimg::png::Image {
        width: want.width as u32,
        height: want.height as u32,
        pixels,
    })
}
/// Read finalized, hash-verified copies from the arena at the compared commit.
pub fn save_check(
    writer: &mut dyn CheckWriter,
    arena: &Arena,
    check: &str,
    commit: u64,
    want: Frame<'_>,
    got: Frame<'_>,
    options: Options<'_>,
    input_log: &[u8],
) -> Result<(), SaveError> {
    let read = |slot| match arena.read(slot) {
        Ok(SlotRead::Final { record, bytes }) if !record.oversize && record.check == check => {
            Ok((record, bytes))
        }
        _ => Err(SaveError::Io),
    };
    let (record, pixels) = read(Slot::Frame)?;
    if record.commit != commit
        || record.width as usize != got.width
        || record.height as usize != got.height
        || pixels != got.canonical().as_slice()
    {
        return Err(SaveError::Io);
    }
    let tree = if record.window != 0 {
        let (tree_record, tree) = read(Slot::Tree)?;
        if tree_record.window != record.window || tree_record.commit != commit {
            return Err(SaveError::Io);
        }
        let end = tree.iter().position(|&b| b == 0).unwrap_or(tree.len());
        let text = std::str::from_utf8(&tree[..end]).map_err(|_| SaveError::Io)?;
        Some((
            format!("tree-{}.txt", record.window),
            text.as_bytes().to_vec(),
        ))
    } else {
        None
    };
    let (screen_record, screen) = read(Slot::Screen)?;
    if screen_record.observation.len() != 1024
        || screen.len() != screen_record.width as usize * screen_record.height as usize * 4
    {
        return Err(SaveError::Io);
    }
    // Encode before publishing any file; codec errors leave the primary verdict intact.
    let got_png = png(got).map_err(|_| SaveError::Io)?;
    let diff_png = libimg_encode::encode(&diff(want, got, options).map_err(|_| SaveError::Io)?)
        .map_err(|_| SaveError::Io)?;
    let screen = Frame::new(
        &screen,
        screen_record.width as usize,
        screen_record.height as usize,
        screen_record.width as usize * 4,
        libpixcheck::Format::Bgrx,
    )
    .map_err(|_| SaveError::Io)?;
    let screen_png = png(screen).map_err(|_| SaveError::Io)?;
    writer.write(&format!("{check}.got.png"), &got_png)?;
    writer.write(&format!("{check}.diff.png"), &diff_png)?;
    if let Some((name, tree)) = tree {
        writer.write(&name, &tree)?;
    }
    writer.write("input.log", input_log)?;
    writer.write("screen.png", &screen_png)?;
    writer.write("screen.record", &screen_record.observation)
}
