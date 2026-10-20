//! Current per-boot target metadata, consumed before RAM block-device registration (docs/metal/README.md, "Boot
//! intents and root selection").
//!
//! Format 1 is `BREENIX-BOOT 1`, then one target line. Format 2 is `BREENIX-BOOT 2`, then one item per line:
//! `target <line>` (required), `root <guid>` and `home <guid>` (unique partition GUIDs, in their text form) and
//! `tests <offset> <bytes>` (a BXTEST image later in the same RAM disk), ending at the payload's end or its first NUL.
//! A format-2 payload that does not parse is kept as a refusal, so it does not become a RAM-disk root.

const FORMAT_1: &[u8] = b"BREENIX-BOOT 1\n";
const FORMAT_2: &[u8] = b"BREENIX-BOOT 2\n";
const TARGET_MAX: usize = 256;
const FORMAT_2_MAX: usize = 1024;

#[derive(Clone, Copy)]
struct Captured {
    target: [u8; TARGET_MAX],
    target_len: usize,
    root: Option<[u8; 36]>,
    home: Option<[u8; 36]>,
    tests: Option<(usize, usize)>,
    userland: Option<[u8; 64]>,
    userland_len: usize,
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

fn parse_format_2(body: &[u8]) -> Result<Captured, &'static str> {
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    if end > FORMAT_2_MAX {
        return Err("the boot intent is longer than 1024 bytes");
    }
    let text = core::str::from_utf8(&body[..end]).map_err(|_| "the boot intent is not text")?;
    let mut captured =
        Captured { target: [0; TARGET_MAX], target_len: 0, root: None, home: None, tests: None, userland: None, userland_len: 0, refused: None };
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
            "root" if captured.root.is_none() => captured.root = Some(guid_text(value).ok_or("the root is not a GUID")?),
            "home" if captured.home.is_none() => captured.home = Some(guid_text(value).ok_or("the home is not a GUID")?),
            "userland" if captured.userland.is_none() => {
                if !valid_generation(value) { return Err("invalid userland generation"); }
                let mut id = [0; 64];
                id[..value.len()].copy_from_slice(value.as_bytes());
                captured.userland = Some(id);
                captured.userland_len = value.len();
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
    Ok(captured)
}

/// Retain the current-format payload before root discovery. Parsing of the target is deferred until PID 1
/// selection, after allocator initialization. Returns whether the payload was a boot intent (format 1 or 2), so it
/// is not a RAM-disk root.
pub fn capture(bytes: &[u8]) -> bool {
    if let Some(line) = bytes.strip_prefix(FORMAT_1) {
        if line.is_empty() || line.len() > TARGET_MAX { return false; }
        let mut captured =
            Captured { target: [0; TARGET_MAX], target_len: line.len(), root: None, home: None, tests: None, userland: None, userland_len: 0, refused: None };
        captured.target[..line.len()].copy_from_slice(line);
        BOOT_INTENT.call_once(|| captured);
        return true;
    }
    let Some(body) = bytes.strip_prefix(FORMAT_2) else { return false };
    let captured = parse_format_2(body).unwrap_or_else(|why| Captured {
        target: [0; TARGET_MAX],
        target_len: 0,
        root: None,
        home: None,
        tests: None,
        userland: None,
        userland_len: 0,
        refused: Some(why),
    });
    BOOT_INTENT.call_once(|| captured);
    true
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
