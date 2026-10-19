//! Load options and the global variables around them (UEFI 2.10 sections 3.1.3, 3.3 and 8.5.4): `BootOrder`,
//! `Boot####`, `OsIndicationsSupported` and `OsIndications`.
use core::ffi::c_void;

use metal_efi::say;
use metal_efi::uefi::*;

pub const OPTION_MAX: usize = 1024;

/// One variable of the global namespace into `buffer`; its length.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn get(runtime: &RuntimeServices, name: &str, buffer: &mut [u8]) -> Result<usize, Status> {
    let wide = Wide::<16>::new(name);
    let mut size = buffer.len();
    let mut attributes = 0u32;
    ok((runtime.get_variable)(wide.as_ptr(), &GLOBAL_VARIABLE, &mut attributes, &mut size, buffer.as_mut_ptr().cast::<c_void>()))?;
    Ok(size)
}

/// The `Boot####` variable name for an option number.
pub fn option_name(number: u16, out: &mut [u8; 8]) -> &str {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out[..4].copy_from_slice(b"Boot");
    for index in 0..4 {
        out[4 + index] = HEX[((number >> (12 - 4 * index)) & 0xf) as usize];
    }
    core::str::from_utf8(&out[..]).unwrap_or("Boot????")
}

/// A load option's fields (section 3.1.3): its attributes, description (as ASCII, other characters shown as `?`)
/// and device path.
pub struct LoadOption<'a> {
    pub attributes: u32,
    pub description: [u8; 64],
    pub description_len: usize,
    pub path: &'a [u8],
}

pub fn parse(bytes: &[u8]) -> Option<LoadOption<'_>> {
    let attributes = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let path_len = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    let mut at = 6;
    let mut description = [0u8; 64];
    let mut description_len = 0;
    loop {
        let unit = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?);
        at += 2;
        if unit == 0 {
            break;
        }
        if description_len < description.len() {
            description[description_len] = if (0x20..0x7f).contains(&unit) { unit as u8 } else { b'?' };
            description_len += 1;
        }
    }
    let path = bytes.get(at..at + path_len)?;
    Some(LoadOption { attributes, description, description_len, path })
}

/// What a device path names, by the kinds of its nodes, without any address or identifier from them.
pub fn kinds(path: &[u8], out: &mut [u8; 96]) -> usize {
    let mut len = 0;
    for (kind, subtype, _) in Nodes::new(path) {
        if kind == node::END {
            break;
        }
        let name: &[u8] = match (kind, subtype) {
            (node::HARDWARE, 1) => b"pci",
            (node::ACPI, _) => b"acpi",
            (node::MESSAGING, node::MAC) => b"mac",
            (node::MESSAGING, node::IPV4) => b"ipv4",
            (node::MESSAGING, node::IPV6) => b"ipv6",
            (node::MESSAGING, node::URI) => b"uri",
            (node::MESSAGING, 18) => b"sata",
            (node::MESSAGING, 5) => b"usb",
            (node::MESSAGING, 23) => b"nvme",
            (node::MEDIA, node::HARD_DRIVE) => b"partition",
            (node::MEDIA, node::FILE_PATH) => b"file",
            (node::MEDIA, 6) => b"firmware-file",
            (node::MEDIA, 7) => b"firmware-volume",
            (node::MEDIA, 3) | (node::HARDWARE, 4) | (node::MESSAGING, 10) => b"vendor",
            (5, _) => b"bbs",
            _ => b"other",
        };
        if len > 0 && len < out.len() {
            out[len] = b'/';
            len += 1;
        }
        for &byte in name {
            if len < out.len() {
                out[len] = byte;
                len += 1;
            }
        }
    }
    len
}

/// The GUID of a path's first node when it is a vendor node (hardware, messaging or media), as text.
fn vendor_guid(path: &[u8], out: &mut [u8; 36]) -> Option<usize> {
    let (kind, subtype, node) = Nodes::new(path).next()?;
    let vendor = matches!((kind, subtype), (node::HARDWARE, 4) | (node::MESSAGING, 10) | (node::MEDIA, 3));
    if !vendor || node.len() < 20 {
        return None;
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = &node[4..20];
    // The GUID's text form (section 2.6.2): the first three fields little-endian, the rest in order.
    let order = [3usize, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut len = 0;
    for (index, &at) in order.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out[len] = b'-';
            len += 1;
        }
        out[len] = HEX[(bytes[at] >> 4) as usize];
        out[len + 1] = HEX[(bytes[at] & 0xf) as usize];
        len += 2;
    }
    Some(len)
}

/// A load option description up to its first colon, letters, digits and spaces only: the firmware puts the
/// device's model and location after the colon.
fn description_class(option: &LoadOption) -> ([u8; 40], usize) {
    let mut out = [0u8; 40];
    let mut len = 0;
    for &byte in option.description[..option.description_len].iter().take_while(|&&b| b != b':') {
        if len < out.len() && (byte.is_ascii_alphanumeric() || byte == b' ') {
            out[len] = byte;
            len += 1;
        }
    }
    (out, len)
}

/// A 16-bit variable such as `BootCurrent`.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn get_u16(runtime: &RuntimeServices, name: &str) -> Option<u16> {
    let mut bytes = [0u8; 2];
    (get(runtime, name, &mut bytes) == Ok(2)).then(|| u16::from_le_bytes(bytes))
}

/// The boot order's option numbers into `out`; how many.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn boot_order(runtime: &RuntimeServices, out: &mut [u16; 64]) -> Result<usize, Status> {
    let mut bytes = [0u8; 128];
    let len = get(runtime, "BootOrder", &mut bytes)?;
    let count = (len / 2).min(out.len());
    for (index, slot) in out.iter_mut().take(count).enumerate() {
        *slot = u16::from_le_bytes([bytes[2 * index], bytes[2 * index + 1]]);
    }
    Ok(count)
}

/// Writes each load option of the boot order to COM1 by number, state and node kinds, not by its description
/// (which can name hardware) or any address its path carries.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn report(runtime: &RuntimeServices) {
    let mut order = [0u16; 64];
    let count = match boot_order(runtime, &mut order) {
        Ok(count) => count,
        Err(status) => {
            say!("boot order unreadable ({:#x})", status);
            return;
        }
    };
    say!("boot order: {} options", count);
    for (index, &number) in order[..count].iter().enumerate() {
        let mut name = [0u8; 8];
        let mut bytes = [0u8; OPTION_MAX];
        let option_name = option_name(number, &mut name);
        match get(runtime, option_name, &mut bytes).ok().and_then(|len| parse(&bytes[..len])) {
            Some(option) => {
                let mut kinds_text = [0u8; 96];
                let kinds_len = kinds(option.path, &mut kinds_text);
                let (class, class_len) = description_class(&option);
                let mut guid = [0u8; 36];
                let guid_len = vendor_guid(option.path, &mut guid).unwrap_or(0);
                say!(
                    "boot order {}: {} {}, \"{}\", path {}{}{}",
                    index + 1,
                    option_name,
                    if option.attributes & LOAD_OPTION_ACTIVE != 0 { "active" } else { "inactive" },
                    core::str::from_utf8(&class[..class_len]).unwrap_or("?"),
                    core::str::from_utf8(&kinds_text[..kinds_len]).unwrap_or("?"),
                    if guid_len > 0 { " vendor " } else { "" },
                    core::str::from_utf8(&guid[..guid_len]).unwrap_or("?")
                );
            }
            None => say!("boot order {}: {} unreadable", index + 1, option_name),
        }
    }
}

/// Firmware setup through `OsIndications` and a reset; returns only when the firmware does not offer it.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn firmware_setup(runtime: &RuntimeServices) -> Status {
    let mut supported = [0u8; 8];
    if get(runtime, "OsIndicationsSupported", &mut supported) != Ok(8)
        || u64::from_le_bytes(supported) & OS_INDICATIONS_BOOT_TO_FW_UI == 0
    {
        say!("the firmware does not offer firmware setup through OsIndications");
        return UNSUPPORTED;
    }
    let mut current = [0u8; 8];
    let value = match get(runtime, "OsIndications", &mut current) {
        Ok(8) => u64::from_le_bytes(current),
        _ => 0,
    } | OS_INDICATIONS_BOOT_TO_FW_UI;
    let name = Wide::<16>::new("OsIndications");
    let attributes = VARIABLE_NON_VOLATILE | VARIABLE_BOOTSERVICE_ACCESS | VARIABLE_RUNTIME_ACCESS;
    let bytes = value.to_le_bytes();
    let status = (runtime.set_variable)(name.as_ptr(), &GLOBAL_VARIABLE, attributes, bytes.len(), bytes.as_ptr().cast());
    if status != SUCCESS {
        say!("could not set OsIndications ({:#x})", status);
        return status;
    }
    say!("restarting into firmware setup");
    (runtime.reset_system)(RESET_COLD, SUCCESS, 0, core::ptr::null())
}
