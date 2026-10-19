//! The lab's question and the network test path (docs/metal/README.md, "The boot manager" and "The lab's answer").
//!
//! The manager asks over the network port the firmware's HTTP boot uses (the port in the device path the install's
//! stub was loaded from): DHCP through the DHCPv4 protocol (UEFI 2.10 section 29.2) for a station address, then an
//! HTTP `GET` of the lab's query URL (29.6). The test path starts the firmware's HTTP boot: either `LoadImage` of the
//! port's HTTP boot path with `BootPolicy` set, as a boot manager loads a load-file device (sections 3.1.2 and 24.7),
//! or `BootNext` set to the firmware's HTTP boot option and a reset, whichever `MANAGER.CFG` names; the firmware
//! fetches the boot URL itself either way.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::http;
use metal_efi::say;
use metal_efi::uefi::*;

use crate::clock::Clock;
use crate::options;
use crate::slots::TestPath;

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

/// The handle carrying `binding` on the network port `port` (a whole device path) names, connecting the port's
/// drivers first when the firmware has not.
unsafe fn port_handle(boot: &BootServices, port: &[u8], binding: &Guid) -> Result<Handle, Status> {
    if !is_whole_path(port) {
        return Err(INVALID_PARAMETER);
    }
    let mut remaining = port.as_ptr();
    let mut handle = null_mut();
    if (boot.locate_device_path)(binding, &mut remaining, &mut handle) == SUCCESS {
        return Ok(handle);
    }
    let mut remaining = port.as_ptr();
    let mut device = null_mut();
    ok((boot.locate_device_path)(&DEVICE_PATH, &mut remaining, &mut device))?;
    let status = (boot.connect_controller)(device, null(), null(), 1);
    say!("connected the network port's drivers ({:#x})", status);
    let mut remaining = port.as_ptr();
    ok((boot.locate_device_path)(binding, &mut remaining, &mut handle))?;
    Ok(handle)
}

/// This image's own device path, as the firmware's HTTP boot loaded it, copied into `out`; its length.
///
/// # Safety
/// `image` must be this application's handle.
pub unsafe fn own_path(boot: &BootServices, image: Handle, out: &mut [u8]) -> Option<usize> {
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
    let len = path_length(path.cast())? + END_NODE.len();
    if len > out.len() {
        return None;
    }
    out[..len].copy_from_slice(core::slice::from_raw_parts(path.cast::<u8>(), len));
    Some(len)
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

/// Asks the lab whether a test boot is pending. `port` is the HTTP boot port's device path; `url` the query URL
/// (`http://host[:port]/path`, not printed: it is a site value). In probe mode the address may come from this
/// image's own HTTP boot when DHCP is refused.
///
/// # Safety
/// `port` must be a device path; `image` this application's handle.
pub unsafe fn ask(boot: &BootServices, image: Handle, port_path: &[u8], url: &[u8], clock: &Clock, probe: bool) -> Answer {
    let port = match port_handle(boot, port_path, &DHCP4_SERVICE_BINDING) {
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
    let nic = match port_handle(boot, port_path, &HTTP_SERVICE_BINDING) {
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

/// The node of `path` of `kind` and `subtype`, with its header.
fn node_of(path: &[u8], kind: u8, subtype: u8) -> Option<&[u8]> {
    Nodes::new(path).find(|(k, s, _)| *k == kind && *s == subtype).map(|(_, _, bytes)| bytes)
}

/// The device path to load the boot image by: the recorded path when a load-file handle covers it to its end (the
/// firmware's HTTP boot child as a finished HTTP boot leaves it), else that port's HTTP boot child (its MAC node and
/// an IPv4 node) with the recorded URI node after its own path, written into `out`.
unsafe fn load_path(boot: &BootServices, port: &[u8], out: &mut [u8; 1024]) -> Option<usize> {
    let mut remaining = port.as_ptr();
    let mut handle = null_mut();
    if (boot.locate_device_path)(&LOAD_FILE, &mut remaining, &mut handle) == SUCCESS
        && *remaining == END_NODE[0]
        && port.len() <= out.len()
    {
        say!("test loop: a load-file handle covers the recorded HTTP boot path");
        out[..port.len()].copy_from_slice(port);
        return Some(port.len());
    }
    let mac = node_of(port, node::MESSAGING, node::MAC)?;
    let uri = node_of(port, node::MESSAGING, node::URI)?;
    let mut count = 0;
    let mut handles = null_mut();
    if (boot.locate_handle_buffer)(BY_PROTOCOL, &LOAD_FILE, null(), &mut count, &mut handles) != SUCCESS {
        return None;
    }
    let mut found = None;
    for index in 0..count {
        let mut path: *mut c_void = null_mut();
        if (boot.handle_protocol)(*handles.add(index), &DEVICE_PATH, &mut path) != SUCCESS || path.is_null() {
            continue;
        }
        let Some(length) = path_length(path.cast()) else { continue };
        let bytes = core::slice::from_raw_parts(path.cast::<u8>(), length);
        if node_of(bytes, node::MESSAGING, node::MAC) == Some(mac) && node_of(bytes, node::MESSAGING, node::IPV4).is_some() {
            let has_uri = node_of(bytes, node::MESSAGING, node::URI).is_some();
            let total = length + if has_uri { 0 } else { uri.len() } + END_NODE.len();
            if total <= out.len() {
                out[..length].copy_from_slice(bytes);
                let mut at = length;
                if !has_uri {
                    out[at..at + uri.len()].copy_from_slice(uri);
                    at += uri.len();
                }
                out[at..at + END_NODE.len()].copy_from_slice(&END_NODE);
                found = Some(total);
                break;
            }
        }
    }
    (boot.free_pool)(handles.cast());
    if found.is_some() {
        say!("test loop: loading through the port's HTTP boot child with the recorded URI");
    }
    found
}

unsafe fn boot_next(runtime: &RuntimeServices, http_option: Option<u16>) -> Status {
    let Some(number) = http_option else { return NOT_FOUND };
    let name = Wide::<16>::new("BootNext");
    let attributes = VARIABLE_NON_VOLATILE | VARIABLE_BOOTSERVICE_ACCESS | VARIABLE_RUNTIME_ACCESS;
    let bytes = number.to_le_bytes();
    let status = (runtime.set_variable)(name.as_ptr(), &GLOBAL_VARIABLE, attributes, 2, bytes.as_ptr().cast());
    if status != SUCCESS {
        say!("could not set BootNext ({:#x})", status);
        return status;
    }
    let mut text = [0u8; 8];
    say!("BootNext is {}; restarting into the firmware's HTTP boot", options::option_name(number, &mut text));
    (runtime.reset_system)(RESET_WARM, SUCCESS, 0, core::ptr::null())
}

/// Starts the firmware's HTTP boot. With `load-file`, `LoadImage` of the HTTP boot port's path (`load_path`); when
/// the firmware has no load-file handle for it, or refuses the load, `BootNext` and a reset instead, which the
/// firmware carries out as its own HTTP boot option. With `boot-next`, that alone. Returns only when no image could
/// be started (no option, or the reset refused), or the started image returned.
///
/// # Safety
/// `port` must be the HTTP boot port's whole device path; `image` this application's handle.
pub unsafe fn start_test_loop(
    boot: &BootServices,
    runtime: &RuntimeServices,
    image: Handle,
    port: &[u8],
    http_option: Option<u16>,
    path: TestPath,
) -> Status {
    if path == TestPath::LoadFile && is_whole_path(port) {
        let mut load = [0u8; 1024];
        match load_path(boot, port, &mut load) {
            Some(_) => {
                let mut child = null_mut();
                let status = (boot.load_image)(1, image, load.as_ptr(), null(), 0, &mut child);
                if status == SUCCESS {
                    say!("starting the image the firmware's HTTP boot fetched");
                    let status = (boot.start_image)(child, null_mut(), null_mut());
                    say!("the test loop's image returned ({:#x})", status);
                    return if status == SUCCESS { LOAD_ERROR } else { status };
                }
                say!("test loop: the firmware did not load the boot image ({:#x}); using BootNext", status);
            }
            None => say!("test loop: no load-file handle for the HTTP boot port; using BootNext"),
        }
    }
    boot_next(runtime, http_option)
}

/// Probe: whether the HTTP boot port's path resolves to a load-file handle, and which part of it that handle covers.
///
/// # Safety
/// `port` must be a device path.
pub unsafe fn report_load_file(boot: &BootServices, port: &[u8]) {
    if !is_whole_path(port) {
        say!("load-file: no HTTP boot path to resolve");
        return;
    }
    let mut remaining = port.as_ptr();
    let mut handle = null_mut();
    let status = (boot.locate_device_path)(&LOAD_FILE, &mut remaining, &mut handle);
    let covered = remaining as usize - port.as_ptr() as usize;
    let mut kinds = [0u8; 96];
    let covered_len = options::kinds(&port[..covered], &mut kinds);
    let mut rest = [0u8; 96];
    let rest_len = options::kinds(&port[covered..], &mut rest);
    say!(
        "load-file on the HTTP boot path: {:#x}, handle covers {}, remaining {}",
        status,
        core::str::from_utf8(&kinds[..covered_len]).unwrap_or("?"),
        core::str::from_utf8(&rest[..rest_len]).unwrap_or("?")
    );
}
