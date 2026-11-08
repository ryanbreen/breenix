//! Session fixture v1. Tree hashes use sorted relative UTF-8 names, u32 LE
//! name lengths, u64 LE content lengths and file bytes. Locale has no external
//! archive on Breenix: its fixture records the built-in C.UTF-8/UTC contract.
use std::{fs, path::Path, os::unix::fs::PermissionsExt};
use libpixcheck::{sha256, Sha256};

pub const FIXTURE_TREE_SHA256: &str = "cdd437833380ffe4286c061fa3fa6938b6b6641fc60e19abf5f3b69118716819";
pub const FONT_CONFIG_SHA256: &str = "f387118b27245cda30913433d6c1eb546c325f9bceb825cc69cafc25b705dea3";
pub const LOCALE_SHA256: &str = "63edab9e0965194ee8dcc684be9b6819eaff875cea6bceb958d45fd85e5ce1fc";
pub const MONO_FONT_SHA256: &str = "b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582";
pub const DISPLAY_FONT_SHA256: &str = "7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954";

pub const FONT_CONFIG: &[u8] = b"mono.font=/usr/share/fonts/DejaVuSansMono.ttf\nmono.size=10\ndisplay.font=/usr/share/fonts/DejaVuSans.ttf\ndisplay.size=14\n";
pub const HOTKEY_CONFIG: &[u8] = b"Super+Super = exec /usr/bin/blauncher\nSuper+Return = exec /usr/bin/bterm\nAlt+q = close\nAlt+Tab = focus_next\nAlt+Shift+Tab = focus_prev\n";
pub const LOCALE: &[u8] = b"LANG=C.UTF-8\nTZ=UTC\ncodeset=UTF-8\ncollation=byte\n";
pub const FIXTURE_FILES: &[(&str, &[u8])] = &[
    ("names.txt", b"Alpha\nZebra\nalpha\n\xc3\xa9clair\n"),
    ("sample.txt", b"Breenix UI session fixture v1\n"),
];
pub const WALL_BASE_NS: u64 = 1_704_067_200_000_000_000;

pub fn digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    dev: u64, inode: u64, size: u64,
    mtime: i64, mtime_nsec: i64, ctime: i64, ctime_nsec: i64,
}
impl FileIdentity {
    fn of(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self { dev: meta.dev(), inode: meta.ino(), size: meta.len(),
            mtime: meta.mtime(), mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(), ctime_nsec: meta.ctime_nsec() }
    }
    // ext2 timestamps have whole-second resolution. Seal a digest only after
    // that second has ended, so a same-second content write cannot reuse it.
    fn stable_before(self, verified: i64) -> bool {
        self.mtime < verified && self.ctime < verified
    }
}
struct VerifiedPin { path: std::path::PathBuf, identity: FileIdentity, digest: String }
static VERIFIED_PINS: std::sync::Mutex<Vec<VerifiedPin>> = std::sync::Mutex::new(Vec::new());
fn pin_now_seconds() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}
fn checked(path: &Path, pin: &str) -> Result<(), String> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() { return Err(format!("fixture pin is not a file: {}", path.display())); }
    let identity = FileIdentity::of(&meta);
    let mut cache = VERIFIED_PINS.lock().map_err(|_| "fixture pin cache poisoned")?;
    if let Some(verified) = cache.iter().find(|v| v.path == path && v.identity == identity) {
        return if verified.digest == pin { Ok(()) }
            else { Err(format!("fixture pin mismatch: {}", path.display())) };
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let after = FileIdentity::of(&file.metadata().map_err(|e| e.to_string())?);
    if after != identity { return Err(format!("fixture changed during verification: {}", path.display())); }
    let got = digest(&bytes);
    if got != pin { return Err(format!("fixture pin mismatch: {}", path.display())); }
    if identity.stable_before(pin_now_seconds()) {
        // The runner primes immutable inputs before forking session supervisors.
        // Fresh per-case files remain independently read and checked.
        if cache.len() == 16 { cache.remove(0); }
        cache.push(VerifiedPin { path: path.to_owned(), identity, digest: got });
    }
    Ok(())
}
/// Prime immutable font pins in the boot's runner before session forks. A
/// different identity still reads and verifies the file against its fixed pin.
pub fn verify_fonts(fonts: &Path) -> Result<(), String> {
    checked(&fonts.join("DejaVuSansMono.ttf"), MONO_FONT_SHA256)?;
    checked(&fonts.join("DejaVuSans.ttf"), DISPLAY_FONT_SHA256)
}
/// The caller must supply a private session directory.
pub fn prepare(root: &Path) -> Result<(), String> {
    // The supervisor prepares this tree before launching session children.
    // Create regular files at their final mode instead of writing then chmodding.
    unsafe extern "C" { fn umask(mask: u32) -> u32; }
    struct RestoreMask(u32);
    impl Drop for RestoreMask { fn drop(&mut self) { unsafe { umask(self.0); } } }
    let mask = RestoreMask(unsafe { umask(0o022) });
    let result = (|| -> std::io::Result<()> {
        match fs::symlink_metadata(root) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(root)?,
            Ok(_) => fs::remove_file(root)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }
        for dir in ["", "etc", "home", "var", "var/log", "fixtures"] {
            let path = root.join(dir);
            fs::create_dir_all(&path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        }
        for (name, contents) in [("etc/fonts.conf", FONT_CONFIG), ("etc/locale.conf", LOCALE),
            ("etc/hotkeys.conf", HOTKEY_CONFIG),
            ("etc/bwm.conf", b"font.size=14\nclock.visible=true\n"),
            ("etc/uitest-cursor.conf", b"visible=false\n"),
            ("etc/app.conf", b"theme=default\n")] {
            let path = root.join(name); fs::write(&path, contents)?;
        }
        for &(name, contents) in FIXTURE_FILES {
            let path = root.join("fixtures").join(name); fs::write(&path, contents)?;
        }
        // A valid frozen BGCLK001 page; participant/step support is E22 22.6.
        let mut clock = [0u8; 64]; clock[..8].copy_from_slice(b"BGCLK001");
        clock[24..32].copy_from_slice(&WALL_BASE_NS.to_le_bytes());
        let path = root.join("ui-clock"); fs::write(&path, clock)?;
        Ok(())
    })();
    drop(mask);
    result.map_err(|e| format!("session fixture setup: {e}"))
}
pub fn verify(root: &Path) -> Result<(), String> {
    verify_at(root, Path::new("/usr/share/fonts"))
}
pub fn verify_at(root: &Path, fonts: &Path) -> Result<(), String> {
    checked(&root.join("etc/fonts.conf"), FONT_CONFIG_SHA256)?;
    checked(&root.join("etc/locale.conf"), LOCALE_SHA256)?;
    verify_fonts(fonts)?;
    let mut entries = fs::read_dir(root.join("fixtures")).map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    let mut tree = Sha256::new();
    for entry in entries {
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() { return Err("non-file session fixture".into()); }
        let name = entry.file_name().into_string().map_err(|_| "non-UTF-8 fixture name")?;
        let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
        tree.update(&(name.len() as u32).to_le_bytes()); tree.update(name.as_bytes());
        tree.update(&(bytes.len() as u64).to_le_bytes()); tree.update(&bytes);
    }
    let got: String = tree.finish().iter().map(|b| format!("{b:02x}")).collect();
    if got != FIXTURE_TREE_SHA256 { return Err("session fixture tree pin mismatch".into()); }
    Ok(())
}
