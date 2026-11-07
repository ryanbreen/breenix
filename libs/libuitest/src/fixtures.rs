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
fn checked(path: &Path, pin: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if digest(&bytes) != pin { return Err(format!("fixture pin mismatch: {}", path.display())); }
    Ok(())
}
/// The caller must supply a private session directory.
pub fn prepare(root: &Path) -> Result<(), String> {
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
            ("etc/app.conf", b"theme=default\n")] {
            let path = root.join(name); fs::write(&path, contents)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        }
        for &(name, contents) in FIXTURE_FILES {
            let path = root.join("fixtures").join(name); fs::write(&path, contents)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        }
        // A valid frozen BGCLK001 page; participant/step support is E22 22.6.
        let mut clock = [0u8; 64]; clock[..8].copy_from_slice(b"BGCLK001");
        clock[24..32].copy_from_slice(&WALL_BASE_NS.to_le_bytes());
        let path = root.join("ui-clock"); fs::write(&path, clock)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        Ok(())
    })();
    result.map_err(|e| format!("session fixture setup: {e}"))
}
pub fn verify(root: &Path) -> Result<(), String> {
    verify_at(root, Path::new("/usr/share/fonts"))
}
pub fn verify_at(root: &Path, fonts: &Path) -> Result<(), String> {
    checked(&root.join("etc/fonts.conf"), FONT_CONFIG_SHA256)?;
    checked(&root.join("etc/locale.conf"), LOCALE_SHA256)?;
    checked(&fonts.join("DejaVuSansMono.ttf"), MONO_FONT_SHA256)?;
    checked(&fonts.join("DejaVuSans.ttf"), DISPLAY_FONT_SHA256)?;
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
