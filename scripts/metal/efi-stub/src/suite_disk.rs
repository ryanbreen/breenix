//! Cold disk readback and suite fixture delivery, from UEFI 2.10 §§13.4, 13.5 and 29.6.
//! A request names the previous storage record exactly; selection requires exactly one matching disk.
use super::*;

const MAGIC: &[u8; 16] = b"BREENIX-SUITE-1\0";
const HEADER: usize = 32 + 2 * SECTOR;
const CHUNK: usize = 64 * 1024;
const HTTP_BINDING: Guid = Guid(
    0xbdc8_e6af,
    0xd9bc,
    0x4379,
    [0xa7, 0x2a, 0xe0, 0xc4, 0xe7, 0x5d, 0xae, 0x1c],
);
const HTTP: Guid = Guid(
    0x7a59_b29b,
    0x910b,
    0x4171,
    [0x82, 0x42, 0xa8, 0x5a, 0x0d, 0xf2, 0x5b, 0x5b],
);

#[repr(C)]
struct File {
    _revision: u64,
    open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    close: unsafe extern "efiapi" fn(*mut File) -> Status,
    _delete: usize,
    read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut c_void) -> Status,
}
#[repr(C)]
struct FileSystem {
    _revision: u64,
    open_volume: unsafe extern "efiapi" fn(*mut FileSystem, *mut *mut File) -> Status,
}

fn ok(status: Status) -> Result<(), Status> {
    if status == SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

/// Pool memory aligned for firmware Block I/O. Owned by a Drop guard.
struct Buffer<'a> {
    boot: &'a BootServices,
    allocation: *mut c_void,
    data: *mut u8,
}
impl<'a> Buffer<'a> {
    unsafe fn new(boot: &'a BootServices, size: usize, alignment: usize) -> Result<Self, Status> {
        let alignment = alignment.max(8);
        if !alignment.is_power_of_two() {
            return Err(INVALID_PARAMETER);
        }
        let mut allocation = null_mut();
        ok((boot.allocate_pool)(
            LOADER_DATA,
            size.checked_add(alignment).ok_or(OUT_OF_RESOURCES)?,
            &mut allocation,
        ))?;
        let data = ((allocation as usize + alignment - 1) & !(alignment - 1)) as *mut u8;
        Ok(Self {
            boot,
            allocation,
            data,
        })
    }
}
impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.boot.free_pool)(self.allocation);
        }
    }
}

unsafe fn read_file(root: *mut File, name: &[u16], buffer: &mut [u8]) -> Result<usize, Status> {
    let mut file = null_mut();
    ok(((*root).open)(root, &mut file, name.as_ptr(), 1, 0))?;
    let mut size = buffer.len();
    let status = ((*file).read)(file, &mut size, buffer.as_mut_ptr().cast());
    ((*file).close)(file);
    ok(status)?;
    Ok(size)
}

/// Install before loading the kernel, or return the disk after a cold power cycle without loading a kernel.
/// An ordinary delivery has no request file and takes the existing boot path.
pub(super) unsafe fn run(
    boot: &BootServices,
    volume: *const u8,
    image: Handle,
) -> Result<(), Status> {
    let mut path = volume;
    let mut handle = null_mut();
    ok((boot.locate_device_path)(
        &SIMPLE_FILE_SYSTEM,
        &mut path,
        &mut handle,
    ))?;
    let mut fs: *mut c_void = null_mut();
    ok((boot.handle_protocol)(handle, &SIMPLE_FILE_SYSTEM, &mut fs))?;
    let fs = fs.cast::<FileSystem>();
    let mut root = null_mut();
    ok(((*fs).open_volume)(fs, &mut root))?;
    let mut config = [0u8; HEADER + 2048];
    let name: [u16; 15] = [
        115, 117, 105, 116, 101, 45, 100, 105, 115, 107, 46, 99, 102, 103, 0,
    ];
    let result = read_file(root, &name, &mut config);
    if result == Err(NOT_FOUND) {
        ((*root).close)(root);
        return Ok(());
    }
    let result = operate(boot, root, &config[..result?], image);
    ((*root).close)(root);
    result
}

unsafe fn operate(
    boot: &BootServices,
    root: *mut File,
    config: &[u8],
    image: Handle,
) -> Result<(), Status> {
    if config.len() < HEADER || &config[..16] != MAGIC {
        return Err(INVALID_PARAMETER);
    }
    let number = |at| u64::from_le_bytes(config[at..at + 8].try_into().unwrap());
    let action = number(16);
    let size = usize::try_from(number(24)).map_err(|_| INVALID_PARAMETER)?;
    if size < 2 * SECTOR || size % SECTOR != 0 || ![1, 2].contains(&action) {
        return Err(INVALID_PARAMETER);
    }
    let expected = &config[32..32 + SECTOR];
    let record = &config[32 + SECTOR..HEADER];
    if !expected.starts_with(b"BREENIX-STORAGE 1\nroot ")
        || !record.starts_with(b"BREENIX-STORAGE 1\nroot ")
    {
        return Err(INVALID_PARAMETER);
    }
    let disk = find_disk(boot, expected, size)?;
    let media = &*(*disk).media;
    let bytes = Buffer::new(boot, CHUNK, media.io_align as usize)?;
    if action == 1 {
        say!("staging the suite fixture on its recorded disk");
        let filename: [u16; 15] = [
            115, 117, 105, 116, 101, 45, 114, 111, 111, 116, 46, 105, 109, 103, 0,
        ];
        let mut file = null_mut();
        ok(((*root).open)(root, &mut file, filename.as_ptr(), 1, 0))?;
        let result = install(boot, disk, file, size, record, &bytes);
        ((*file).close)(file);
        result?;
        say!("suite fixture verified: {} bytes", size);
        Ok(())
    } else {
        if expected != record {
            return Err(INVALID_PARAMETER);
        }
        let url = config[HEADER..]
            .strip_suffix(&[0])
            .ok_or(INVALID_PARAMETER)?;
        if url.len() > 2046 || !url.starts_with(b"http://") || !url.is_ascii() {
            return Err(INVALID_PARAMETER);
        }
        say!("reading the suite disk after power-off: {} bytes", size);
        upload(boot, disk, size, &bytes, url, image)?;
        say!("disk readback complete: {} bytes", size);
        halt()
    }
}

unsafe fn find_disk(
    boot: &BootServices,
    expected: &[u8],
    size: usize,
) -> Result<*mut BlockIo, Status> {
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(
        BY_PROTOCOL,
        &BLOCK_IO,
        null(),
        &mut count,
        &mut handles,
    ))?;
    let mut found = null_mut();
    let mut matches = 0;
    for index in 0..count {
        let mut protocol = null_mut();
        if (boot.handle_protocol)(*handles.add(index), &BLOCK_IO, &mut protocol) != SUCCESS {
            continue;
        }
        let disk = protocol.cast::<BlockIo>();
        let media = &*(*disk).media;
        if media.logical_partition != 0
            || media.removable_media != 0
            || media.media_present == 0
            || media.read_only != 0
            || media.block_size != SECTOR as u32
            || media.last_block < (size / SECTOR - 1) as u64
        {
            continue;
        }
        let buffer = Buffer::new(boot, SECTOR, media.io_align as usize)?;
        if ((*disk).read_blocks)(disk, media.media_id, 0, SECTOR, buffer.data.cast()) == SUCCESS
            && core::slice::from_raw_parts(buffer.data, SECTOR) == expected
        {
            found = disk;
            matches += 1;
        }
    }
    (boot.free_pool)(handles.cast());
    if matches != 1 {
        say!(
            "suite disk selection refused: {} matching recorded disks",
            matches
        );
        return Err(NOT_FOUND);
    }
    Ok(found)
}

unsafe fn install(
    boot: &BootServices,
    disk: *mut BlockIo,
    file: *mut File,
    size: usize,
    record: &[u8],
    buffer: &Buffer<'_>,
) -> Result<(), Status> {
    let media_id = (*(*disk).media).media_id;
    let wire = core::slice::from_raw_parts_mut(buffer.data, CHUNK);
    let mut first = SECTOR;
    ok(((*file).read)(file, &mut first, buffer.data.cast()))?;
    if first != SECTOR || &wire[..SECTOR] != record {
        return Err(LOAD_ERROR);
    }
    // Invalidate first, data next, root record last; an interrupted stage cannot name a root.
    wire[..SECTOR].fill(0);
    ok(((*disk).write_blocks)(
        disk,
        media_id,
        0,
        SECTOR,
        buffer.data.cast(),
    ))?;
    ok(((*disk).flush_blocks)(disk))?;
    let check = Buffer::new(boot, CHUNK, (*(*disk).media).io_align as usize)?;
    let mut offset = SECTOR;
    while offset < size {
        let wanted = CHUNK.min(size - offset);
        let mut read = wanted;
        ok(((*file).read)(file, &mut read, buffer.data.cast()))?;
        if read != wanted {
            return Err(LOAD_ERROR);
        }
        ok(((*disk).write_blocks)(
            disk,
            media_id,
            (offset / SECTOR) as u64,
            wanted,
            buffer.data.cast(),
        ))?;
        ok(((*disk).flush_blocks)(disk))?;
        ok(((*disk).read_blocks)(
            disk,
            media_id,
            (offset / SECTOR) as u64,
            wanted,
            check.data.cast(),
        ))?;
        if core::slice::from_raw_parts(check.data, wanted) != &wire[..wanted] {
            return Err(LOAD_ERROR);
        }
        offset += wanted;
    }
    let mut extra = 1;
    ok(((*file).read)(file, &mut extra, buffer.data.cast()))?;
    if extra != 0 {
        return Err(LOAD_ERROR);
    }
    wire[..SECTOR].copy_from_slice(record);
    ok(((*disk).write_blocks)(
        disk,
        media_id,
        0,
        SECTOR,
        buffer.data.cast(),
    ))?;
    ok(((*disk).flush_blocks)(disk))?;
    ok(((*disk).read_blocks)(
        disk,
        media_id,
        0,
        SECTOR,
        check.data.cast(),
    ))?;
    if core::slice::from_raw_parts(check.data, SECTOR) != record {
        return Err(LOAD_ERROR);
    }
    Ok(())
}

#[repr(C)]
struct Binding {
    create: unsafe extern "efiapi" fn(*mut Binding, *mut Handle) -> Status,
    destroy: unsafe extern "efiapi" fn(*mut Binding, Handle) -> Status,
}
#[repr(C)]
struct Access {
    default: u8,
    address: [u8; 4],
    subnet: [u8; 4],
    port: u16,
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
struct Header {
    name: *const u8,
    value: *const u8,
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
    event: Handle,
    status: Status,
    message: *mut Message,
}
#[repr(C)]
struct Http {
    _mode: usize,
    configure: unsafe extern "efiapi" fn(*mut Http, *const Config) -> Status,
    request: unsafe extern "efiapi" fn(*mut Http, *mut Token) -> Status,
    cancel: unsafe extern "efiapi" fn(*mut Http, *mut Token) -> Status,
    response: unsafe extern "efiapi" fn(*mut Http, *mut Token) -> Status,
    poll: unsafe extern "efiapi" fn(*mut Http) -> Status,
}

unsafe extern "efiapi" fn completed(_event: Handle, context: *mut c_void) {
    ptr::write_volatile(context.cast::<bool>(), true);
}

unsafe fn exchange(
    boot: &BootServices,
    http: *mut Http,
    message: &mut Message,
    response: bool,
) -> Result<(), Status> {
    // Boot-services event slots, UEFI 2.10 §7.1; the original stub leaves these slots opaque.
    let create: unsafe extern "efiapi" fn(
        u32,
        usize,
        unsafe extern "efiapi" fn(Handle, *mut c_void),
        *mut c_void,
        *mut Handle,
    ) -> Status = core::mem::transmute(boot._events[0]);
    let close: unsafe extern "efiapi" fn(Handle) -> Status = core::mem::transmute(boot._events[4]);
    let stall: unsafe extern "efiapi" fn(usize) -> Status =
        core::mem::transmute(boot._exit_to_watchdog[4]);
    let mut done = false;
    let mut event = null_mut();
    ok(create(
        0x200,
        8,
        completed,
        ptr::addr_of_mut!(done).cast(),
        &mut event,
    ))?;
    let mut token = Token {
        event,
        status: ERROR | 6,
        message,
    };
    let status = if response {
        ((*http).response)(http, &mut token)
    } else {
        ((*http).request)(http, &mut token)
    };
    if status == SUCCESS {
        for _ in 0..60_000 {
            ((*http).poll)(http);
            if ptr::read_volatile(&done) {
                break;
            }
            stall(1000);
        }
        if !ptr::read_volatile(&done) {
            ((*http).cancel)(http, &mut token);
            token.status = ERROR | 18;
        }
    }
    close(event);
    ok(status)?;
    ok(token.status)
}

// UEFI 2.10 §9.1: the source device of this image, not an enumeration-order NIC.
#[repr(C)]
struct LoadedImage {
    _revision: u32,
    _parent: Handle,
    _system: *const SystemTable,
    device: Handle,
}

unsafe fn boot_http_binding(
    boot: &BootServices,
    image: Handle,
) -> Result<(*mut Binding, Access), Status> {
    const LOADED_IMAGE: Guid = Guid(
        0x5b1b_31a1,
        0x9562,
        0x11d2,
        [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
    );
    let mut loaded = null_mut();
    ok((boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded))?;
    let device = (*loaded.cast::<LoadedImage>()).device;
    let mut path = null_mut();
    ok((boot.handle_protocol)(device, &DEVICE_PATH, &mut path))?;
    // UEFI 2.10 §10.3.4.11: reuse this boot's DHCP/static source address.
    // HTTP boot need not leave a default mapping for a newly created TCP child.
    let bytes = core::slice::from_raw_parts(
        path.cast::<u8>(),
        path_length(path.cast()).ok_or(INVALID_PARAMETER)?,
    );
    let mut at = 0;
    let mut access = None;
    while at < bytes.len() {
        let len = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let node = &bytes[at..at + len];
        if node[0] == 3 && node[1] == 12 && len >= 27 {
            let address: [u8; 4] = node[4..8].try_into().unwrap();
            let subnet: [u8; 4] = node[23..27].try_into().unwrap();
            if address != [0; 4] && subnet != [0; 4] {
                access = Some(Access {
                    default: 0,
                    address,
                    subnet,
                    port: 0,
                });
            }
        }
        at += len;
    }
    let Some(access) = access else {
        say!("disk readback needs the HTTP boot source IPv4 address and subnet");
        return Err(NOT_FOUND);
    };
    let mut remaining = path as *const u8;
    let mut nic = null_mut();
    ok((boot.locate_device_path)(
        &HTTP_BINDING,
        &mut remaining,
        &mut nic,
    ))?;
    let mut binding = null_mut();
    ok((boot.handle_protocol)(nic, &HTTP_BINDING, &mut binding))?;
    Ok((binding.cast(), access))
}

unsafe fn upload(
    boot: &BootServices,
    disk: *mut BlockIo,
    size: usize,
    buffer: &Buffer<'_>,
    url: &[u8],
    image: Handle,
) -> Result<(), Status> {
    let (binding, mut access) = boot_http_binding(boot, image)?;
    say!("disk readback uses the HTTP boot interface");
    let mut child = null_mut();
    ok(((*binding).create)(binding, &mut child))?;
    let mut protocol = null_mut();
    let result = (|| {
        ok((boot.handle_protocol)(child, &HTTP, &mut protocol))?;
        let http = protocol.cast::<Http>();
        let config = Config {
            version: 1,
            timeout_ms: 60_000,
            ipv6: 0,
            access: &mut access,
        };
        let status = ((*http).configure)(http, &config);
        if status != SUCCESS {
            say!("disk readback HTTP configure failed: {:#x}", status);
        }
        ok(status)?;
        let mut wide = [0u16; 2048];
        for (out, &byte) in wide.iter_mut().zip(url) {
            *out = byte as u16;
        }
        let authority = url[7..]
            .split(|&b| b == b'/')
            .next()
            .ok_or(INVALID_PARAMETER)?;
        let mut host = [0u8; 512];
        if authority.len() >= host.len() {
            return Err(INVALID_PARAMETER);
        }
        host[..authority.len()].copy_from_slice(authority);
        let mut length = [0u8; 24];
        let mut remaining = size;
        let mut at = length.len() - 1;
        loop {
            at -= 1;
            length[at] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
            if remaining == 0 {
                break;
            }
        }
        let mut request = Request {
            method: 6,
            url: wide.as_ptr(),
        };
        let mut headers = [
            Header {
                name: b"Host\0".as_ptr(),
                value: host.as_ptr(),
            },
            Header {
                name: b"Content-Length\0".as_ptr(),
                value: length[at..].as_ptr(),
            },
            Header {
                name: b"Content-Type\0".as_ptr(),
                value: b"application/octet-stream\0".as_ptr(),
            },
        ];
        let mut message = Message {
            data: ptr::addr_of_mut!(request).cast(),
            count: headers.len(),
            headers: headers.as_mut_ptr(),
            len: 0,
            body: null_mut(),
        };
        let result = exchange(boot, http, &mut message, false);
        if let Err(status) = result {
            say!("disk readback HTTP request failed: {:#x}", status);
        }
        result?;
        let mut offset = 0;
        while offset < size {
            let len = CHUNK.min(size - offset);
            ok(((*disk).read_blocks)(
                disk,
                (*(*disk).media).media_id,
                (offset / SECTOR) as u64,
                len,
                buffer.data.cast(),
            ))?;
            let mut message = Message {
                data: null_mut(),
                count: 0,
                headers: null_mut(),
                len,
                body: buffer.data.cast(),
            };
            exchange(boot, http, &mut message, false)?;
            offset += len;
            if offset % (16 * 1024 * 1024) == 0 {
                say!("disk readback sent {} bytes", offset);
            }
        }
        let mut status_code = 0u32;
        let mut message = Message {
            data: ptr::addr_of_mut!(status_code).cast(),
            count: 0,
            headers: null_mut(),
            len: 0,
            body: null_mut(),
        };
        let result = exchange(boot, http, &mut message, true);
        if !message.headers.is_null() {
            for index in 0..message.count {
                let header = &*message.headers.add(index);
                (boot.free_pool)(header.name as *mut c_void);
                (boot.free_pool)(header.value as *mut c_void);
            }
            (boot.free_pool)(message.headers.cast());
        }
        result?;
        if status_code != 3 {
            return Err(LOAD_ERROR);
        } // HTTP_STATUS_200_OK, not the wire integer 200.
        Ok(())
    })();
    if !protocol.is_null() {
        ((*protocol.cast::<Http>()).configure)(protocol.cast(), null());
    }
    ((*binding).destroy)(binding, child);
    result
}
