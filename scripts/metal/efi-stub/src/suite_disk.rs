//! Cold disk readback and suite fixture delivery, from UEFI 2.10 sections 10.3.5, 13.4, 13.5, 13.9 and 29.6.
//!
//! Format 1 names the previously owned whole disk by its exact storage record in LBA 0 (the layout before the
//! persistent install). Format 2 names the test root partition by its unique GUID (docs/metal/README.md, "Test boots
//! on the persistent layout"): the stub writes through that partition's own Block I/O, so a write cannot leave it.
//! Either way selection requires exactly one match.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::bootdisk::SECTOR;
use metal_efi::http;
use metal_efi::say;
use metal_efi::uefi::*;

const MAGIC_1: &[u8; 16] = b"BREENIX-SUITE-1\0";
const MAGIC_2: &[u8; 16] = b"BREENIX-SUITE-2\0";
const HEADER_1: usize = 32 + 2 * SECTOR;
const HEADER_2: usize = 48 + 2 * SECTOR;
const CHUNK: usize = 64 * 1024;
const RECORD: &[u8] = b"BREENIX-STORAGE 1\nroot ";

/// Pool memory aligned for firmware Block I/O. Owned by a Drop guard.
pub struct Buffer<'a> {
    boot: &'a BootServices,
    allocation: *mut c_void,
    pub data: *mut u8,
}

impl<'a> Buffer<'a> {
    pub unsafe fn new(boot: &'a BootServices, size: usize, alignment: usize) -> Result<Self, Status> {
        let alignment = alignment.max(8);
        if !alignment.is_power_of_two() {
            return Err(INVALID_PARAMETER);
        }
        let mut allocation = null_mut();
        ok((boot.allocate_pool)(LOADER_DATA, size.checked_add(alignment).ok_or(OUT_OF_RESOURCES)?, &mut allocation))?;
        let data = ((allocation as usize + alignment - 1) & !(alignment - 1)) as *mut u8;
        Ok(Self { boot, allocation, data })
    }
}

impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        // SAFETY: the allocation this guard made.
        unsafe {
            (self.boot.free_pool)(self.allocation);
        }
    }
}

pub unsafe fn read_file(root: *mut File, name: &[u16], buffer: &mut [u8]) -> Result<usize, Status> {
    let mut file = null_mut();
    ok(((*root).open)(root, &mut file, name.as_ptr(), FILE_MODE_READ, 0))?;
    let mut size = buffer.len();
    let status = ((*file).read)(file, &mut size, buffer.as_mut_ptr().cast());
    ((*file).close)(file);
    ok(status)?;
    Ok(size)
}

/// The root directory of the FAT volume at `volume`.
pub unsafe fn open_root(boot: &BootServices, volume: *const u8) -> Result<*mut File, Status> {
    let mut path = volume;
    let mut handle = null_mut();
    ok((boot.locate_device_path)(&SIMPLE_FILE_SYSTEM, &mut path, &mut handle))?;
    let mut fs: *mut c_void = null_mut();
    ok((boot.handle_protocol)(handle, &SIMPLE_FILE_SYSTEM, &mut fs))?;
    let fs = fs.cast::<FileSystem>();
    let mut root = null_mut();
    ok(((*fs).open_volume)(fs, &mut root))?;
    Ok(root)
}

/// Install before loading the kernel, or return the disk after a cold power cycle without loading a kernel.
/// An ordinary delivery has no request file and takes the existing boot path.
pub(super) unsafe fn run(boot: &BootServices, volume: *const u8, image: Handle) -> Result<(), Status> {
    let root = open_root(boot, volume)?;
    let mut config = [0u8; HEADER_2 + 2048];
    let name = Wide::<16>::new("suite-disk.cfg");
    let result = read_file(root, &name.units, &mut config);
    if result == Err(NOT_FOUND) {
        ((*root).close)(root);
        return Ok(());
    }
    let result = operate(boot, root, &config[..result?], image);
    ((*root).close)(root);
    result
}

/// The disk or partition a request names, and the request's fields once its format is read.
struct Request<'c> {
    action: u64,
    size: usize,
    expected: &'c [u8],
    record: &'c [u8],
    url: &'c [u8],
    disk: *mut BlockIo,
}

unsafe fn operate(boot: &BootServices, root: *mut File, config: &[u8], image: Handle) -> Result<(), Status> {
    let number = |at: usize| u64::from_le_bytes(config[at..at + 8].try_into().unwrap());
    let request = if config.len() >= HEADER_1 && &config[..16] == MAGIC_1 {
        let size = usize::try_from(number(24)).map_err(|_| INVALID_PARAMETER)?;
        let expected = &config[32..32 + SECTOR];
        if !expected.starts_with(RECORD) || size < 2 * SECTOR {
            return Err(INVALID_PARAMETER);
        }
        let disk = find_disk(boot, expected, size)?;
        Request { action: number(16), size, expected, record: &config[32 + SECTOR..HEADER_1], url: &config[HEADER_1..], disk }
    } else if config.len() >= HEADER_2 && &config[..16] == MAGIC_2 {
        let size = usize::try_from(number(24)).map_err(|_| INVALID_PARAMETER)?;
        let guid: [u8; 16] = config[32..48].try_into().unwrap();
        let disk = find_partition(boot, &guid, size)?;
        Request {
            action: number(16),
            size,
            expected: &config[48..48 + SECTOR],
            record: &config[48 + SECTOR..HEADER_2],
            url: &config[HEADER_2..],
            disk,
        }
    } else {
        return Err(INVALID_PARAMETER);
    };
    if request.size < 2 * SECTOR || request.size % SECTOR != 0 || ![1, 2].contains(&request.action) {
        return Err(INVALID_PARAMETER);
    }
    if !request.record.starts_with(RECORD) {
        return Err(INVALID_PARAMETER);
    }
    let disk = request.disk;
    let media = &*(*disk).media;
    let bytes = Buffer::new(boot, CHUNK, media.io_align as usize)?;
    if request.action == 1 {
        say!("staging the suite fixture on its recorded disk");
        let filename = Wide::<16>::new("suite-root.img");
        let mut file = null_mut();
        ok(((*root).open)(root, &mut file, filename.as_ptr(), FILE_MODE_READ, 0))?;
        let result = install(boot, disk, file, request.size, request.record, &bytes);
        ((*file).close)(file);
        result?;
        say!("suite fixture verified: {} bytes", request.size);
        Ok(())
    } else {
        if request.expected != request.record {
            return Err(INVALID_PARAMETER);
        }
        // The partition's first sector must still be the suite's root record.
        let first = Buffer::new(boot, SECTOR, media.io_align as usize)?;
        ok(((*disk).read_blocks)(disk, media.media_id, 0, SECTOR, first.data.cast()))?;
        if core::slice::from_raw_parts(first.data, SECTOR) != request.expected {
            say!("suite disk selection refused: the recorded root is no longer on its disk");
            return Err(NOT_FOUND);
        }
        let url = request.url.strip_suffix(&[0]).ok_or(INVALID_PARAMETER)?;
        if url.len() > 2046 || !url.starts_with(b"http://") || !url.is_ascii() {
            return Err(INVALID_PARAMETER);
        }
        say!("reading the suite disk after power-off: {} bytes", request.size);
        upload(boot, disk, request.size, &bytes, url, image)?;
        say!("disk readback complete: {} bytes", request.size);
        metal_efi::com1::halt()
    }
}

unsafe fn find_disk(boot: &BootServices, expected: &[u8], size: usize) -> Result<*mut BlockIo, Status> {
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles))?;
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
        let buffer = match Buffer::new(boot, SECTOR, media.io_align as usize) {
            Ok(buffer) => buffer,
            Err(status) => {
                (boot.free_pool)(handles.cast());
                return Err(status);
            }
        };
        if ((*disk).read_blocks)(disk, media.media_id, 0, SECTOR, buffer.data.cast()) == SUCCESS
            && core::slice::from_raw_parts(buffer.data, SECTOR) == expected
        {
            found = disk;
            matches += 1;
        }
    }
    (boot.free_pool)(handles.cast());
    if matches != 1 {
        say!("suite disk selection refused: {} matching recorded disks", matches);
        return Err(NOT_FOUND);
    }
    Ok(found)
}

/// The unique partition signature of a GPT partition's device path: its last Hard Drive node with a GUID signature
/// (the Hard Drive media node, section 10.3.5).
pub unsafe fn partition_signature(boot: &BootServices, handle: Handle) -> Option<[u8; 16]> {
    let mut path: *mut c_void = null_mut();
    if (boot.handle_protocol)(handle, &DEVICE_PATH, &mut path) != SUCCESS || path.is_null() {
        return None;
    }
    let length = path_length(path.cast())?;
    let bytes = core::slice::from_raw_parts(path.cast::<u8>(), length);
    let mut signature = None;
    for (kind, subtype, node) in Nodes::new(bytes) {
        // Header, partition number (4), start (8), size (8), signature (16), MBR type, signature type.
        if kind == node::MEDIA && subtype == node::HARD_DRIVE && node.len() >= 42 && node[41] == 2 {
            signature = Some(node[24..40].try_into().unwrap());
        }
    }
    signature
}

/// The one partition whose unique GUID is `guid`, at least `size` bytes long.
pub unsafe fn find_partition(boot: &BootServices, guid: &[u8; 16], size: usize) -> Result<*mut BlockIo, Status> {
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles))?;
    let mut found = null_mut();
    let mut matches = 0;
    for index in 0..count {
        let handle = *handles.add(index);
        let mut protocol = null_mut();
        if (boot.handle_protocol)(handle, &BLOCK_IO, &mut protocol) != SUCCESS {
            continue;
        }
        let disk = protocol.cast::<BlockIo>();
        let media = &*(*disk).media;
        if media.logical_partition == 0 || partition_signature(boot, handle).as_ref() != Some(guid) {
            continue;
        }
        matches += 1;
        if media.media_present != 0
            && media.read_only == 0
            && media.block_size == SECTOR as u32
            && media.last_block >= (size / SECTOR - 1) as u64
        {
            found = disk;
        }
    }
    (boot.free_pool)(handles.cast());
    if matches != 1 || found.is_null() {
        say!("suite disk selection refused: {} partitions with the test root's GUID, usable {}", matches, !found.is_null());
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
    ok(((*disk).write_blocks)(disk, media_id, 0, SECTOR, buffer.data.cast()))?;
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
        ok(((*disk).write_blocks)(disk, media_id, (offset / SECTOR) as u64, wanted, buffer.data.cast()))?;
        ok(((*disk).flush_blocks)(disk))?;
        ok(((*disk).read_blocks)(disk, media_id, (offset / SECTOR) as u64, wanted, check.data.cast()))?;
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
    ok(((*disk).write_blocks)(disk, media_id, 0, SECTOR, buffer.data.cast()))?;
    ok(((*disk).flush_blocks)(disk))?;
    ok(((*disk).read_blocks)(disk, media_id, 0, SECTOR, check.data.cast()))?;
    if core::slice::from_raw_parts(check.data, SECTOR) != record {
        return Err(LOAD_ERROR);
    }
    Ok(())
}

/// The HTTP binding and IPv4 station address of the network port this image was loaded from (UEFI 2.10 sections 9.1
/// and 10.3.4, the IPv4 node): the source device of this image, not an enumeration-order NIC.
pub unsafe fn boot_http_binding(boot: &BootServices, image: Handle) -> Result<(Handle, http::Access), Status> {
    let mut loaded = null_mut();
    ok((boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded))?;
    let device = (*loaded.cast::<LoadedImage>()).device_handle;
    let mut path = null_mut();
    ok((boot.handle_protocol)(device, &DEVICE_PATH, &mut path))?;
    // Reuse this boot's DHCP or static source address.
    let bytes = core::slice::from_raw_parts(path.cast::<u8>(), path_length(path.cast()).ok_or(INVALID_PARAMETER)?);
    let mut access = None;
    for (kind, subtype, node) in Nodes::new(bytes) {
        if kind == node::MESSAGING && subtype == node::IPV4 && node.len() >= 27 {
            let address: [u8; 4] = node[4..8].try_into().unwrap();
            let subnet: [u8; 4] = node[23..27].try_into().unwrap();
            if address != [0; 4] && subnet != [0; 4] {
                access = Some(http::Access { default: 0, address, subnet, port: 0 });
            }
        }
    }
    let Some(access) = access else {
        say!("disk readback needs the HTTP boot source IPv4 address and subnet");
        return Err(NOT_FOUND);
    };
    let mut remaining = path as *const u8;
    let mut nic = null_mut();
    ok((boot.locate_device_path)(&HTTP_SERVICE_BINDING, &mut remaining, &mut nic))?;
    Ok((nic, access))
}

unsafe fn upload(
    boot: &BootServices,
    disk: *mut BlockIo,
    size: usize,
    buffer: &Buffer<'_>,
    url: &[u8],
    image: Handle,
) -> Result<(), Status> {
    let (nic, access) = boot_http_binding(boot, image)?;
    say!("disk readback uses the HTTP boot interface");
    let client = match http::Client::new(boot, nic, access, 60_000) {
        Ok(client) => client,
        Err(status) => {
            say!("disk readback HTTP configure failed: {:#x}", status);
            return Err(status);
        }
    };
    let wide = Wide::<2048>::new(core::str::from_utf8(url).map_err(|_| INVALID_PARAMETER)?);
    let mut host = [0u8; 256];
    http::host_header(url, &mut host)?;
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
    let mut headers = [
        http::Header { name: b"Host\0".as_ptr(), value: host.as_ptr() },
        http::Header { name: b"Content-Length\0".as_ptr(), value: length[at..].as_ptr() },
        http::Header { name: b"Content-Type\0".as_ptr(), value: b"application/octet-stream\0".as_ptr() },
    ];
    if let Err(status) = client.request(http::PUT, wide.as_ptr(), &mut headers) {
        say!("disk readback HTTP request failed: {:#x}", status);
        return Err(status);
    }
    let mut offset = 0;
    while offset < size {
        let len = CHUNK.min(size - offset);
        ok(((*disk).read_blocks)(disk, (*(*disk).media).media_id, (offset / SECTOR) as u64, len, buffer.data.cast()))?;
        client.send_body(buffer.data, len)?;
        offset += len;
        if offset % (16 * 1024 * 1024) == 0 {
            say!("disk readback sent {} bytes", offset);
        }
    }
    let (status, _) = client.response(&mut [])?;
    if status != http::STATUS_200_OK {
        return Err(LOAD_ERROR);
    }
    Ok(())
}
