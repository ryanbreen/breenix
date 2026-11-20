//! Current per-boot target metadata, consumed before RAM block-device registration (docs/metal/README.md, "Boot
//! intents and root selection").
//!
//! Format 1 is `BREENIX-BOOT 1`, then one target line. Format 2 is `BREENIX-BOOT 2`, then one item per line:
//! `target <line>` (required), `root <guid>` and `home <guid>` (unique partition GUIDs, in their text form) and
//! `tests <offset> <bytes>` (a BXTEST image later in the same RAM disk), ending at the payload's end or its first NUL.
//! A format-2 payload that does not parse is kept as a refusal, so it does not become a RAM-disk root.

const FORMAT_1: &[u8] = b"BREENIX-BOOT 1\n";
const FORMAT_2: &[u8] = b"BREENIX-BOOT 2\n";
const FORMAT_3: &[u8] = b"BREENIX-BOOT 3\n";
const TARGET_MAX: usize = 256;
const FORMAT_2_MAX: usize = 1024;

#[derive(Clone, Copy)]
struct Mount {
    guid: [u8; 36],
    path: [u8; 64],
    path_len: usize,
    writable: bool,
}

#[derive(Clone, Copy)]
pub struct Captured {
    format_3: bool,
    #[cfg(any(target_arch = "aarch64", test))]
    format_2: bool,
    target: [u8; TARGET_MAX],
    target_len: usize,
    cmdline: [u8;256],
    cmdline_len: Option<usize>,
    root: Option<[u8; 36]>,
    home: Option<[u8; 36]>,
    tests: Option<(usize, usize)>,
    userland: Option<[u8; 64]>,
    userland_len: usize,
    previous: Option<[u8; 64]>,
    previous_len: usize,
    mount_entries: [Option<Mount>; 6],
    mount_count: usize,
    refused: Option<&'static str>,
}

static BOOT_INTENT: spin::Once<Captured> = spin::Once::new();

fn guid_text(text: &str) -> Option<[u8; 36]> {
    let bytes = text.as_bytes();
    let shape = bytes.len() == 36
        && bytes.iter().enumerate().all(|(at, &b)| if [8, 13, 18, 23].contains(&at) { b == b'-' } else { b.is_ascii_hexdigit() });
    shape.then(|| bytes.try_into().unwrap())
}

fn number(text: &str) -> Option<usize> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

fn parse_format_2(body: &[u8], format_3: bool) -> Result<Captured, &'static str> {
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    if end > FORMAT_2_MAX || (format_3 && end + FORMAT_3.len() > FORMAT_2_MAX) {
        return Err("the boot intent is longer than 1024 bytes");
    }
    let text = core::str::from_utf8(&body[..end]).map_err(|_| "the boot intent is not text")?;
    let mut captured =
        Captured { format_3, #[cfg(any(target_arch = "aarch64", test))] format_2: true, target: [0; TARGET_MAX], target_len: 0, cmdline:[0;256], cmdline_len:None, root: None, home: None, tests: None, userland: None, userland_len: 0, previous: None, previous_len: 0, mount_entries: [None; 6], mount_count: 0, refused: None };
    let mut target = false;
    for line in text.split('\n').filter(|line| !line.is_empty()) {
        let (item, value) = line.split_once(' ').ok_or("a boot intent line has no value")?;
        match item {
            "target" if !target && value.len() < TARGET_MAX => {
                captured.target[..value.len()].copy_from_slice(value.as_bytes());
                captured.target[value.len()] = b'\n';
                captured.target_len = value.len() + 1;
                target = true;
            }
            "cmdline" if captured.cmdline_len.is_none() && value.len()<256
                && value.bytes().all(|b| (32..=126).contains(&b)) => {
                captured.cmdline[..value.len()].copy_from_slice(value.as_bytes());
                captured.cmdline_len=Some(value.len());
            }
            "root" if !format_3 && captured.root.is_none() => captured.root = Some(guid_text(value).ok_or("the root is not a GUID")?),
            "home" if !format_3 && captured.home.is_none() => captured.home = Some(guid_text(value).ok_or("the home is not a GUID")?),
            "userland" if captured.userland.is_none() => {
                if !valid_generation(value) { return Err("invalid userland generation"); }
                let mut id = [0; 64];
                id[..value.len()].copy_from_slice(value.as_bytes());
                captured.userland = Some(id);
                captured.userland_len = value.len();
            }
            "previous-userland" if captured.previous.is_none() => {
                if !valid_generation(value) { return Err("invalid previous userland generation"); }
                let mut id = [0; 64];
                id[..value.len()].copy_from_slice(value.as_bytes());
                captured.previous = Some(id);
                captured.previous_len = value.len();
            }
            "mount" if format_3 => {
                let mut parts = value.split(' ');
                let guid = guid_text(parts.next().ok_or("mount needs a GUID")?).ok_or("the mount is not a GUID")?;
                let path = parts.next().ok_or("mount needs a path")?;
                let writable = match parts.next() { Some("ro") => false, Some("rw") => true, _ => return Err("invalid mount mode") };
                if parts.next().is_some() || !valid_mount_path(path) || captured.mount_count == 6 {
                    return Err("invalid mount path or count");
                }
                if captured.mounts().any(|(_, existing, _)| existing == path)
                    || (writable && captured.mounts().filter(|(_, _, rw)| *rw).count() == 2) {
                    return Err("duplicate mount path or too many writable mounts");
                }
                let mut stored = [0; 64];
                stored[..path.len()].copy_from_slice(path.as_bytes());
                captured.mount_entries[captured.mount_count] = Some(Mount { guid, path: stored, path_len: path.len(), writable });
                captured.mount_count += 1;
            }
            "tests" if captured.tests.is_none() => {
                let (offset, bytes) = value.split_once(' ').ok_or("tests needs an offset and a length")?;
                captured.tests = Some((number(offset).ok_or("bad tests offset")?, number(bytes).ok_or("bad tests length")?));
            }
            _ => return Err("the boot intent has an unknown or repeated item"),
        }
    }
    if !target {
        return Err("the boot intent names no target");
    }
    if format_3 && !captured.mounts().any(|(_, path, _)| path == "/") {
        return Err("the boot intent names no root mount");
    }
    Ok(captured)
}

fn valid_mount_path(path: &str) -> bool {
    path.len() <= 64 && path.starts_with('/')
        && path.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/._-".contains(&b))
        && path.split('/').all(|part| part != "." && part != "..")
}

impl Captured {
    /// Distinguish a refused format-3 intent from a legacy boot.
    pub fn explicit_mounts(&self) -> Result<bool, &'static str> {
        if self.format_3 {
            if let Some(reason) = self.refused {
                return Err(reason);
            }
        }
        Ok(self.format_3)
    }

    /// Mounts in the order supplied by the intent.
    pub fn mounts(&self) -> impl Iterator<Item = (&str, &str, bool)> {
        self.mount_entries[..self.mount_count].iter().flatten().map(|mount| {
            (core::str::from_utf8(&mount.guid).unwrap(), core::str::from_utf8(&mount.path[..mount.path_len]).unwrap(), mount.writable)
        })
    }
}

/// Mounts from the captured boot intent, for the boot mount consumer.
#[allow(dead_code)]
pub fn mounts() -> impl Iterator<Item = (&'static str, &'static str, bool)> {
    BOOT_INTENT.get().into_iter().flat_map(|captured| captured.mounts())
}

/// The boot payload retained before block discovery.
pub fn captured() -> Option<&'static Captured> {
    BOOT_INTENT.get()
}

pub fn explicit_mounts_requested() -> bool {
    captured().is_some_and(|intent| intent.format_3)
}

/// Parse an intent without changing the captured boot state.
pub fn parse(bytes: &[u8]) -> Option<Captured> {
    if let Some(line) = bytes.strip_prefix(FORMAT_1) {
        if line.is_empty() || line.len() > TARGET_MAX { return None; }
        let mut captured =
            Captured { format_3: false, #[cfg(any(target_arch = "aarch64", test))] format_2: false, target: [0; TARGET_MAX], target_len: line.len(), cmdline:[0;256], cmdline_len:None, root: None, home: None, tests: None, userland: None, userland_len: 0, previous: None, previous_len: 0, mount_entries: [None; 6], mount_count: 0, refused: None };
        captured.target[..line.len()].copy_from_slice(line);
        return Some(captured);
    }
    let (body, format_3) = if let Some(body) = bytes.strip_prefix(FORMAT_2) { (body, false) }
        else { (bytes.strip_prefix(FORMAT_3)?, true) };
    let captured = parse_format_2(body, format_3).unwrap_or_else(|why| Captured {
        format_3,
        #[cfg(any(target_arch = "aarch64", test))]
        format_2: true,
        target: [0; TARGET_MAX],
        target_len: 0,
        cmdline:[0;256],
        cmdline_len:None,
        root: None,
        home: None,
        tests: None,
        userland: None,
        userland_len: 0,
        previous: None,
        previous_len: 0,
        mount_entries: [None; 6],
        mount_count: 0,
        refused: Some(why),
    });
    Some(captured)
}

/// Retain a boot intent before root discovery, returning whether it was recognized.
pub fn capture(bytes: &[u8]) -> bool {
    let Some(captured) = parse(bytes) else { return false };
    BOOT_INTENT.call_once(|| captured);
    true
}

/// Select a root generation only for a valid persistent boot.
#[cfg(any(target_arch = "aarch64", test))]
pub fn root_userland(captured: Option<&Captured>) -> Result<Option<&str>, &'static str> {
    let Some(captured) = captured else { return Ok(None) };
    if let Some(reason) = captured.refused { return Err(reason); }
    if !captured.format_2 || &captured.target[..captured.target_len] != b"persistent\n" { return Ok(None); }
    Ok(captured.userland.as_ref().and_then(|id| core::str::from_utf8(&id[..captured.userland_len]).ok()))
}

#[cfg(target_arch = "aarch64")]
pub fn root_generation() -> Result<Option<&'static str>, &'static str> {
    let captured = BOOT_INTENT.get();
    let id = root_userland(captured)?;
    if id.is_none() && captured.is_some_and(|intent| intent.format_2 && &intent.target[..intent.target_len] == b"persistent\n") {
        return Err("persistent intent names no userland generation");
    }
    Ok(id)
}

/// The selected target line, without the format header; absent for a refused intent.
pub fn get() -> Option<&'static [u8]> {
    BOOT_INTENT.get().filter(|captured| captured.refused.is_none()).map(|captured| &captured.target[..captured.target_len])
}

/// The unique GUID of the partition the intent names as root, in text form.
pub fn root_partition() -> Option<&'static str> {
    BOOT_INTENT.get()?.root.as_ref().and_then(|guid| core::str::from_utf8(guid).ok())
}

/// The unique GUID of the partition the intent names as home, in text form.
pub fn home_partition() -> Option<&'static str> {
    BOOT_INTENT.get()?.home.as_ref().and_then(|guid| core::str::from_utf8(guid).ok())
}

/// Where the BXTEST image lies in the RAM disk that carried the intent: offset and length in bytes.
pub fn tests() -> Option<(usize, usize)> {
    BOOT_INTENT.get()?.tests
}

/// Why a format-2 intent was refused.
pub fn refused() -> Option<&'static str> {
    BOOT_INTENT.get()?.refused
}

/// Immutable install identity embedded in the kernel slot and passed to the installer.
pub fn userland() -> Option<&'static str> {
    let captured = BOOT_INTENT.get()?;
    core::str::from_utf8(&captured.userland.as_ref()?[..captured.userland_len]).ok()
}

/// Directory-component validation excludes relative paths and staging names.
pub fn valid_generation(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The manager's current slot at install request time; only the installer consumes it.
pub fn previous_userland() -> Option<&'static str> {
    let captured = BOOT_INTENT.get()?;
    core::str::from_utf8(&captured.previous.as_ref()?[..captured.previous_len]).ok()
}

/// Kernel command line captured with the boot intent, before root discovery.
pub fn command_line()->Option<&'static str> {
    let captured=BOOT_INTENT.get().filter(|c|c.refused.is_none())?;
    core::str::from_utf8(&captured.cmdline[..captured.cmdline_len?]).ok()
}
