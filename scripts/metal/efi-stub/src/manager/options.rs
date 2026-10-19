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

pub fn has_uri(path: &[u8]) -> bool {
    Nodes::new(path).any(|(kind, subtype, _)| kind == node::MESSAGING && subtype == node::URI)
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

/// The firmware's HTTP boot option: the first option in the boot order whose device path has a URI node. Its
/// device path is copied into `path`.
///
/// # Safety
/// `runtime` must be the firmware's runtime services table.
pub unsafe fn http_boot_option(runtime: &RuntimeServices, path: &mut [u8; OPTION_MAX]) -> Option<(u16, usize)> {
    let mut order = [0u16; 64];
    let count = boot_order(runtime, &mut order).ok()?;
    for &number in &order[..count] {
        let mut name = [0u8; 8];
        let mut bytes = [0u8; OPTION_MAX];
        let Ok(len) = get(runtime, option_name(number, &mut name), &mut bytes) else { continue };
        let Some(option) = parse(&bytes[..len]) else { continue };
        if option.attributes & LOAD_OPTION_ACTIVE != 0 && has_uri(option.path) && option.path.len() <= path.len() {
            path[..option.path.len()].copy_from_slice(option.path);
            return Some((number, option.path.len()));
        }
    }
    None
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
                let breenix = option.description[..option.description_len].starts_with(b"Breenix");
                say!(
                    "boot order {}: {} {}, path {}{}",
                    index + 1,
                    option_name,
                    if option.attributes & LOAD_OPTION_ACTIVE != 0 { "active" } else { "inactive" },
                    core::str::from_utf8(&kinds_text[..kinds_len]).unwrap_or("?"),
                    if breenix { ", the Breenix boot manager" } else { "" }
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
