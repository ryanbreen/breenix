//! An HTTP client over the firmware's HTTP protocol (UEFI 2.10 section 29.6), bound to one network port with an
//! explicit IPv4 station address: HTTP boot need not leave a default mapping for a newly created TCP child, and the
//! boot manager configures its address by DHCP itself.
use core::ffi::c_void;
use core::ptr::{self, null_mut};

use crate::uefi::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Access {
    pub default: u8,
    pub address: [u8; 4],
    pub subnet: [u8; 4],
    pub port: u16,
}

#[repr(C)]
struct Config {
    version: u32,
    timeout_ms: u32,
    ipv6: u8,
    access: *mut Access,
}

#[repr(C)]
struct Request {
    method: u32,
    url: *const u16,
}

#[repr(C)]
pub struct Header {
    pub name: *const u8,
    pub value: *const u8,
}

#[repr(C)]
struct Message {
    data: *mut c_void,
    count: usize,
    headers: *mut Header,
    len: usize,
    body: *mut c_void,
}

#[repr(C)]
struct Token {
    event: Event,
    status: Status,
    message: *mut Message,
}

#[repr(C)]
struct Protocol {
    _mode: usize,
    configure: unsafe extern "efiapi" fn(*mut Protocol, *const Config) -> Status,
    request: unsafe extern "efiapi" fn(*mut Protocol, *mut Token) -> Status,
    cancel: unsafe extern "efiapi" fn(*mut Protocol, *mut Token) -> Status,
    response: unsafe extern "efiapi" fn(*mut Protocol, *mut Token) -> Status,
    poll: unsafe extern "efiapi" fn(*mut Protocol) -> Status,
}

/// `HttpMethodGet` and `HttpMethodPut`.
pub const GET: u32 = 0;
pub const PUT: u32 = 6;
/// `HTTP_STATUS_200_OK`: the status enumeration's value, not the wire integer 200.
pub const STATUS_200_OK: u32 = 3;

unsafe extern "efiapi" fn completed(_event: Event, context: *mut c_void) {
    ptr::write_volatile(context.cast::<bool>(), true);
}

/// One HTTP child of a network port's HTTP service binding.
pub struct Client<'a> {
    boot: &'a BootServices,
    binding: *mut ServiceBinding,
    child: Handle,
    http: *mut Protocol,
    access: Access,
    /// How long one exchange may take before it is cancelled.
    pub limit_ms: usize,
}

impl<'a> Client<'a> {
    /// A configured client on `nic`, the handle carrying the HTTP service binding.
    ///
    /// # Safety
    /// `nic` must be a handle of this firmware.
    pub unsafe fn new(boot: &'a BootServices, nic: Handle, access: Access, timeout_ms: u32) -> Result<Self, Status> {
        let mut binding = null_mut();
        ok((boot.handle_protocol)(nic, &HTTP_SERVICE_BINDING, &mut binding))?;
        let binding = binding.cast::<ServiceBinding>();
        let mut child = null_mut();
        ok(((*binding).create_child)(binding, &mut child))?;
        let mut protocol = null_mut();
        if let Err(status) = ok((boot.handle_protocol)(child, &HTTP, &mut protocol)) {
            ((*binding).destroy_child)(binding, child);
            return Err(status);
        }
        let mut client = Self { boot, binding, child, http: protocol.cast(), access, limit_ms: timeout_ms as usize };
        let config = Config { version: 1, timeout_ms, ipv6: 0, access: &mut client.access };
        let status = ((*client.http).configure)(client.http, &config);
        if status != SUCCESS {
            return Err(status);
        }
        Ok(client)
    }

    unsafe fn exchange(&self, message: &mut Message, response: bool) -> Result<(), Status> {
        let mut done = false;
        let mut event = null_mut();
        ok((self.boot.create_event)(EVT_NOTIFY_SIGNAL, TPL_CALLBACK, Some(completed), ptr::addr_of_mut!(done).cast(), &mut event))?;
        let mut token = Token { event, status: NOT_READY, message };
        let status = if response {
            ((*self.http).response)(self.http, &mut token)
        } else {
            ((*self.http).request)(self.http, &mut token)
        };
        if status == SUCCESS {
            for _ in 0..self.limit_ms {
                ((*self.http).poll)(self.http);
                if ptr::read_volatile(&done) {
                    break;
                }
                (self.boot.stall)(1000);
            }
            if !ptr::read_volatile(&done) {
                ((*self.http).cancel)(self.http, &mut token);
                token.status = TIMEOUT;
            }
        }
        (self.boot.close_event)(event);
        ok(status)?;
        ok(token.status)
    }

    /// Sends a request line and headers (a body, if any, follows through `send_body`).
    ///
    /// # Safety
    /// `url` must be NUL-terminated UCS-2; each header's name and value NUL-terminated ASCII.
    pub unsafe fn request(&self, method: u32, url: *const u16, headers: &mut [Header]) -> Result<(), Status> {
        let mut request = Request { method, url };
        let mut message = Message {
            data: ptr::addr_of_mut!(request).cast(),
            count: headers.len(),
            headers: headers.as_mut_ptr(),
            len: 0,
            body: null_mut(),
        };
        self.exchange(&mut message, false)
    }

    /// Sends part of a request's body.
    ///
    /// # Safety
    /// `body..body+len` must be readable.
    pub unsafe fn send_body(&self, body: *const u8, len: usize) -> Result<(), Status> {
        let mut message = Message { data: null_mut(), count: 0, headers: null_mut(), len, body: body as *mut c_void };
        self.exchange(&mut message, false)
    }

    /// Receives the response's status and as much of its body as `body` holds. Returns the status enumeration value
    /// and the body's length.
    pub fn response(&self, body: &mut [u8]) -> Result<(u32, usize), Status> {
        let mut status_code = 0u32;
        let mut message = Message {
            data: ptr::addr_of_mut!(status_code).cast(),
            count: 0,
            headers: null_mut(),
            len: body.len(),
            body: if body.is_empty() { null_mut() } else { body.as_mut_ptr().cast() },
        };
        // SAFETY: the message's buffers are this frame's and `body`.
        let result = unsafe { self.exchange(&mut message, true) };
        if !message.headers.is_null() {
            // The driver allocated the headers; the caller frees each name and value and the array (29.6).
            for index in 0..message.count {
                // SAFETY: `count` headers the driver returned.
                unsafe {
                    let header = &*message.headers.add(index);
                    (self.boot.free_pool)(header.name as *mut c_void);
                    (self.boot.free_pool)(header.value as *mut c_void);
                }
            }
            // SAFETY: the driver's header array.
            unsafe { (self.boot.free_pool)(message.headers.cast()) };
        }
        result?;
        Ok((status_code, if body.is_empty() { 0 } else { message.len }))
    }
}

impl Drop for Client<'_> {
    fn drop(&mut self) {
        // SAFETY: the child this client created; configuring with no data resets it before it is destroyed.
        unsafe {
            ((*self.http).configure)(self.http, core::ptr::null());
            ((*self.binding).destroy_child)(self.binding, self.child);
        }
    }
}

/// The request's `Host` value from an `http://host[:port]/path` URL, NUL-terminated.
pub fn host_header(url: &[u8], out: &mut [u8; 256]) -> Result<(), Status> {
    let rest = url.strip_prefix(b"http://").ok_or(INVALID_PARAMETER)?;
    let authority = rest.split(|&b| b == b'/').next().ok_or(INVALID_PARAMETER)?;
    if authority.is_empty() || authority.len() >= out.len() {
        return Err(INVALID_PARAMETER);
    }
    out.fill(0);
    out[..authority.len()].copy_from_slice(authority);
    Ok(())
}
