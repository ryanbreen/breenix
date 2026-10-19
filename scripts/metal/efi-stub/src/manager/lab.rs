//! The lab's question and the network test path (docs/metal/README.md, "The boot manager" and "The lab's answer").
//!
//! The manager asks over the network port the firmware's HTTP boot option names: DHCP through the DHCPv4 protocol
//! (UEFI 2.10 section 29.2) for a station address, then an HTTP `GET` of the lab's query URL (29.6). The test path
//! loads the HTTP boot option with `LoadImage` and `BootPolicy` set, as the firmware's own boot manager does
//! (sections 3.1.2 and 24.7), so the firmware fetches the boot URL itself.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::http;
use metal_efi::say;
use metal_efi::uefi::*;

use crate::clock::Clock;

const DHCP_DISCOVER_TIMEOUTS: [u32; 3] = [2, 3, 4];
const DHCP_REQUEST_TIMEOUTS: [u32; 2] = [2, 2];
const HTTP_TIMEOUT_MS: u32 = 4000;
pub const ID_MAX: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Current,
    Previous,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SlotAction {
    None,
    Commit,
    Rollback,
}

/// A boot id from the lab, printable ASCII only.
#[derive(Clone, Copy)]
pub struct BootId {
    pub bytes: [u8; ID_MAX],
    pub len: usize,
}

impl BootId {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

#[derive(Clone, Copy)]
pub enum Answer {
    Test(BootId),
    Persistent(BootId, Entry, SlotAction),
    Busy(BootId),
    None(&'static str),
}

/// One answer line: `test <boot>`, `persistent <boot> <current|previous> [commit|rollback]` or `busy <boot>`.
pub fn parse_answer(body: &[u8]) -> Answer {
    let Ok(text) = core::str::from_utf8(body) else { return Answer::None("the answer is not text") };
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.contains('\n') {
        return Answer::None("the answer has more than one line");
    }
    let mut words = line.split(' ');
    let kind = words.next().unwrap_or("");
    let Some(id) = words.next().and_then(boot_id) else { return Answer::None("the answer names no boot") };
    let rest: [Option<&str>; 3] = [words.next(), words.next(), words.next()];
    match (kind, rest) {
        ("test", [None, None, None]) => Answer::Test(id),
        ("busy", [None, None, None]) => Answer::Busy(id),
        ("persistent", [entry, action, None]) => {
            let entry = match entry {
                None | Some("current") => Entry::Current,
                Some("previous") => Entry::Previous,
                Some(_) => return Answer::None("the answer names an unknown entry"),
            };
            let action = match action {
                None => SlotAction::None,
                Some("commit") => SlotAction::Commit,
                Some("rollback") => SlotAction::Rollback,
                Some(_) => return Answer::None("the answer names an unknown slot action"),
            };
            Answer::Persistent(id, entry, action)
        }
        _ => Answer::None("the answer is not one the manager knows"),
    }
}

fn boot_id(word: &str) -> Option<BootId> {
    if word.is_empty() || word.len() > ID_MAX || !word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    let mut bytes = [0u8; ID_MAX];
    bytes[..word.len()].copy_from_slice(word.as_bytes());
    Some(BootId { bytes, len: word.len() })
}

/// The handle carrying `binding` on the network port the HTTP boot option's device path names, connecting the
/// port's drivers first when the firmware has not.
unsafe fn port_handle(boot: &BootServices, option: &[u8], binding: &Guid) -> Result<Handle, Status> {
    let mut remaining = option.as_ptr();
    let mut handle = null_mut();
    if (boot.locate_device_path)(binding, &mut remaining, &mut handle) == SUCCESS {
        return Ok(handle);
    }
    let mut remaining = option.as_ptr();
    let mut device = null_mut();
    ok((boot.locate_device_path)(&DEVICE_PATH, &mut remaining, &mut device))?;
    let status = (boot.connect_controller)(device, null(), null(), 1);
    say!("connected the network port's drivers ({:#x})", status);
    let mut remaining = option.as_ptr();
    ok((boot.locate_device_path)(binding, &mut remaining, &mut handle))?;
    Ok(handle)
}

/// A station address by DHCP on the port, released (stopped) again before it returns.
unsafe fn dhcp(boot: &BootServices, port: Handle, clock: &Clock) -> Result<http::Access, Status> {
    let mut binding = null_mut();
    ok((boot.handle_protocol)(port, &DHCP4_SERVICE_BINDING, &mut binding))?;
    let binding = binding.cast::<ServiceBinding>();
    let mut child = null_mut();
    ok(((*binding).create_child)(binding, &mut child))?;
    let result = (|| {
        let mut protocol = null_mut();
        ok((boot.handle_protocol)(child, &DHCP4, &mut protocol))?;
        let dhcp = protocol.cast::<Dhcp4>();
        let mut discover = DHCP_DISCOVER_TIMEOUTS;
        let mut request = DHCP_REQUEST_TIMEOUTS;
        // Parameter request list: subnet mask, router, DNS servers.
        let mut parameters = Dhcp4PacketOption { op_code: 55, length: 3, data: [1, 3, 6] };
        let mut options = [&mut parameters as *mut Dhcp4PacketOption];
        let config = Dhcp4ConfigData {
            discover_try_count: discover.len() as u32,
            discover_timeout: discover.as_mut_ptr(),
            request_try_count: request.len() as u32,
            request_timeout: request.as_mut_ptr(),
            client_address: [0; 4],
            dhcp4_callback: 0,
            callback_context: null_mut(),
            option_count: options.len() as u32,
            option_list: options.as_mut_ptr(),
        };
        let started = clock.ms();
        ok(((*dhcp).configure)(dhcp, &config))?;
        let status = ((*dhcp).start)(dhcp, null_mut());
        let mut mode: Dhcp4ModeData = core::mem::zeroed();
        let mode_status = ((*dhcp).get_mode_data)(dhcp, &mut mode);
        ((*dhcp).stop)(dhcp);
        ((*dhcp).configure)(dhcp, null());
        ok(status)?;
        ok(mode_status)?;
        if mode.state != DHCP4_BOUND || mode.client_address == [0; 4] || mode.subnet_mask == [0; 4] {
            return Err(NOT_READY);
        }
        say!("dhcp: bound in {} ms", clock.ms() - started);
        Ok(http::Access { default: 0, address: mode.client_address, subnet: mode.subnet_mask, port: 0 })
    })();
    ((*binding).destroy_child)(binding, child);
    result
}

/// The station address the firmware's HTTP boot used for this image, when it was loaded over the network.
unsafe fn http_boot_address(boot: &BootServices, image: Handle) -> Option<http::Access> {
    let mut loaded = null_mut();
    (boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded);
    if loaded.is_null() {
        return None;
    }
    let mut path: *mut c_void = null_mut();
    (boot.handle_protocol)((*loaded.cast::<LoadedImage>()).device_handle, &DEVICE_PATH, &mut path);
    if path.is_null() {
        return None;
    }
    let bytes = core::slice::from_raw_parts(path.cast::<u8>(), path_length(path.cast())?);
    Nodes::new(bytes).find_map(|(kind, subtype, node)| {
        let address: [u8; 4] = node.get(4..8)?.try_into().ok()?;
        let subnet: [u8; 4] = node.get(23..27)?.try_into().ok()?;
        (kind == node::MESSAGING && subtype == node::IPV4 && address != [0; 4] && subnet != [0; 4])
            .then_some(http::Access { default: 0, address, subnet, port: 0 })
    })
}

/// Asks the lab whether a test boot is pending. `option` is the HTTP boot option's device path; `url` the query URL
/// (`http://host[:port]/path`, not printed: it is a site value). In probe mode the address may come from this
/// image's own HTTP boot when DHCP is refused.
///
/// # Safety
/// `option` must be a device path; `image` this application's handle.
pub unsafe fn ask(boot: &BootServices, image: Handle, option: &[u8], url: &[u8], clock: &Clock, probe: bool) -> Answer {
    let port = match port_handle(boot, option, &DHCP4_SERVICE_BINDING) {
        Ok(port) => port,
        Err(status) => {
            say!("the network port has no DHCP service ({:#x})", status);
            return Answer::None("the network port has no DHCP service");
        }
    };
    let access = match dhcp(boot, port, clock) {
        Ok(access) => access,
        Err(status) => {
            say!("dhcp: no address ({:#x})", status);
            match (probe, http_boot_address(boot, image)) {
                (true, Some(access)) => {
                    say!("probe: asking with the address of this image's HTTP boot");
                    access
                }
                _ => return Answer::None("DHCP gave no address"),
            }
        }
    };
    let nic = match port_handle(boot, option, &HTTP_SERVICE_BINDING) {
        Ok(nic) => nic,
        Err(status) => {
            say!("the network port has no HTTP service ({:#x})", status);
            return Answer::None("the network port has no HTTP service");
        }
    };
    let started = clock.ms();
    let client = match http::Client::new(boot, nic, access, HTTP_TIMEOUT_MS) {
        Ok(client) => client,
        Err(status) => {
            say!("lab: HTTP configure failed ({:#x})", status);
            return Answer::None("HTTP could not be configured");
        }
    };
    let Ok(text) = core::str::from_utf8(url) else { return Answer::None("the query URL is not text") };
    let wide = Wide::<512>::new(text);
    let mut host = [0u8; 256];
    if http::host_header(url, &mut host).is_err() {
        return Answer::None("the query URL has no host");
    }
    let mut headers = [http::Header { name: b"Host\0".as_ptr(), value: host.as_ptr() }];
    if let Err(status) = client.request(http::GET, wide.as_ptr(), &mut headers) {
        say!("lab: the request failed ({:#x})", status);
        return Answer::None("the lab did not take the request");
    }
    let mut body = [0u8; 256];
    let (code, len) = match client.response(&mut body) {
        Ok(reply) => reply,
        Err(status) => {
            say!("lab: no response ({:#x})", status);
            return Answer::None("the lab did not respond");
        }
    };
    say!("lab: response in {} ms, status code {}, {} bytes", clock.ms() - started, code, len);
    if code != http::STATUS_200_OK {
        return Answer::None("the lab answered with an error status");
    }
    parse_answer(&body[..len])
}

/// Starts the firmware's HTTP boot option. Returns only when it could not be loaded, or returned.
///
/// # Safety
/// `option` must be the HTTP boot option's device path; `image` this application's handle.
pub unsafe fn start_test_loop(boot: &BootServices, image: Handle, option: &[u8]) -> Status {
    let mut child = null_mut();
    let status = (boot.load_image)(1, image, option.as_ptr(), null(), 0, &mut child);
    if status != SUCCESS {
        say!("the HTTP boot option could not be loaded ({:#x})", status);
        return status;
    }
    say!("starting the image the HTTP boot option fetched");
    let status = (boot.start_image)(child, null_mut(), null_mut());
    say!("the test loop's image returned ({:#x})", status);
    if status == SUCCESS { LOAD_ERROR } else { status }
}

/// Whether the HTTP boot option's port offers the load-file protocol the test path uses.
///
/// # Safety
/// `option` must be a device path.
pub unsafe fn has_load_file(boot: &BootServices, option: &[u8]) -> bool {
    let mut remaining = option.as_ptr();
    let mut handle = null_mut();
    (boot.locate_device_path)(&LOAD_FILE, &mut remaining, &mut handle) == SUCCESS
}
