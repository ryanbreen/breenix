//! The UEFI interfaces the metal applications call, declared from the UEFI 2.10 specification: the system table
//! (section 4.3), boot services (4.4, 7), runtime services (4.5, 8), device paths (10), console input (12.3),
//! simple text output (12.4), graphics output (12.9), Block I/O (13.9), the simple file system and file protocols
//! (13.4, 13.5), load file (13.1), DHCPv4 (29.2) and HTTP (29.6). Services the applications do not call are kept as
//! placeholders so the fields after them stay where the specification puts them.
use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Status = usize;
pub type Event = *mut c_void;

pub const ERROR: usize = 1 << (usize::BITS - 1);
pub const SUCCESS: Status = 0;
pub const LOAD_ERROR: Status = ERROR | 1;
pub const INVALID_PARAMETER: Status = ERROR | 2;
pub const UNSUPPORTED: Status = ERROR | 3;
pub const BAD_BUFFER_SIZE: Status = ERROR | 4;
pub const BUFFER_TOO_SMALL: Status = ERROR | 5;
pub const NOT_READY: Status = ERROR | 6;
pub const DEVICE_ERROR: Status = ERROR | 7;
pub const WRITE_PROTECTED: Status = ERROR | 8;
pub const OUT_OF_RESOURCES: Status = ERROR | 9;
pub const VOLUME_CORRUPTED: Status = ERROR | 10;
pub const MEDIA_CHANGED: Status = ERROR | 13;
pub const NOT_FOUND: Status = ERROR | 14;
pub const ACCESS_DENIED: Status = ERROR | 15;
pub const NO_RESPONSE: Status = ERROR | 16;
pub const TIMEOUT: Status = ERROR | 18;
pub const ABORTED: Status = ERROR | 21;

/// `EfiLoaderData`, the memory type an application allocates its own data as.
pub const LOADER_DATA: u32 = 2;
/// `AllocateAnyPages`.
pub const ALLOCATE_ANY_PAGES: u32 = 0;
/// `EFI_NATIVE_INTERFACE`.
pub const NATIVE_INTERFACE: u32 = 0;
/// `ByProtocol`, the `LocateHandleBuffer` search type.
pub const BY_PROTOCOL: u32 = 2;
/// Event types and task priority levels (section 7.1).
pub const EVT_TIMER: u32 = 0x8000_0000;
pub const EVT_NOTIFY_SIGNAL: u32 = 0x0000_0200;
pub const TPL_CALLBACK: usize = 8;
/// `TimerPeriodic` and `TimerRelative`, `SetTimer`'s delay kinds.
pub const TIMER_PERIODIC: u32 = 1;
pub const TIMER_RELATIVE: u32 = 2;
/// Variable attributes (section 8.2).
pub const VARIABLE_NON_VOLATILE: u32 = 1;
pub const VARIABLE_BOOTSERVICE_ACCESS: u32 = 2;
pub const VARIABLE_RUNTIME_ACCESS: u32 = 4;
/// `EfiResetCold`.
pub const RESET_COLD: u32 = 0;
/// `EfiResetWarm`.
pub const RESET_WARM: u32 = 1;
/// Load option attributes (section 3.1.3).
pub const LOAD_OPTION_ACTIVE: u32 = 1;
pub const LOAD_OPTION_HIDDEN: u32 = 8;
pub const LOAD_OPTION_CATEGORY: u32 = 0x1f00;
/// `EFI_OS_INDICATIONS_BOOT_TO_FW_UI` (section 8.5.4).
pub const OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 1;
/// File open modes (section 13.5).
pub const FILE_MODE_READ: u64 = 1;
pub const FILE_MODE_WRITE: u64 = 2;
pub const FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;
pub const FILE_DIRECTORY: u64 = 0x10;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

impl Guid {
    /// The GUID in its on-disk (mixed-endian) byte order, as GPT entries and device path nodes store it.
    pub fn bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..4].copy_from_slice(&self.0.to_le_bytes());
        out[4..6].copy_from_slice(&self.1.to_le_bytes());
        out[6..8].copy_from_slice(&self.2.to_le_bytes());
        out[8..].copy_from_slice(&self.3);
        out
    }
}

pub const DEVICE_PATH: Guid = Guid(0x0957_6e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const BLOCK_IO: Guid = Guid(0x964e_5b21, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const SIMPLE_FILE_SYSTEM: Guid = Guid(0x964e_5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const LOADED_IMAGE: Guid = Guid(0x5b1b_31a1, 0x9562, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const LOAD_FILE: Guid = Guid(0x56ec_3091, 0x954c, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const FILE_INFO: Guid = Guid(0x0957_6e92, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const RAM_DISK: Guid = Guid(0xab38_a0df, 0x6873, 0x44a9, [0x87, 0xe6, 0xd4, 0xeb, 0x56, 0x14, 0x84, 0x49]);
/// `EFI_VIRTUAL_DISK_GUID`: a RAM disk holding a raw disk image.
pub const VIRTUAL_DISK: Guid = Guid(0x77ab_535a, 0x45fc, 0x624b, [0x55, 0x60, 0xf7, 0xb2, 0x81, 0xd1, 0xf9, 0x6e]);
pub const GRAPHICS_OUTPUT: Guid = Guid(0x9042_a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);
pub const GLOBAL_VARIABLE: Guid = Guid(0x8be4_df61, 0x93ca, 0x11d2, [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
pub const SIMPLE_NETWORK: Guid = Guid(0xa198_32b9, 0xac25, 0x11d3, [0x9a, 0x2d, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);
pub const DHCP4_SERVICE_BINDING: Guid = Guid(0x9d9a_39d8, 0xbd42, 0x4a73, [0xa4, 0xd5, 0x8e, 0xe9, 0x4b, 0xe1, 0x13, 0x80]);
pub const DHCP4: Guid = Guid(0x8a21_9718, 0x4ef5, 0x4761, [0x91, 0xc8, 0xc0, 0xf0, 0x4b, 0xda, 0x9e, 0x56]);
pub const HTTP_SERVICE_BINDING: Guid = Guid(0xbdc8_e6af, 0xd9bc, 0x4379, [0xa7, 0x2a, 0xe0, 0xc4, 0xe7, 0x5d, 0xae, 0x1c]);
pub const HTTP: Guid = Guid(0x7a59_b29b, 0x910b, 0x4171, [0x82, 0x42, 0xa8, 0x5a, 0x0d, 0xf2, 0x5b, 0x5b]);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *const RuntimeServices,
    pub boot_services: *const BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *const c_void,
}

pub type EventNotify = unsafe extern "efiapi" fn(Event, *mut c_void);

/// The boot services table in the specification's order.
#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: usize,
    pub restore_tpl: usize,
    pub allocate_pages: unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    pub free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status,
    pub get_memory_map: usize,
    pub allocate_pool: unsafe extern "efiapi" fn(u32, usize, *mut *mut c_void) -> Status,
    pub free_pool: unsafe extern "efiapi" fn(*mut c_void) -> Status,
    pub create_event: unsafe extern "efiapi" fn(u32, usize, Option<EventNotify>, *mut c_void, *mut Event) -> Status,
    pub set_timer: unsafe extern "efiapi" fn(Event, u32, u64) -> Status,
    pub wait_for_event: unsafe extern "efiapi" fn(usize, *const Event, *mut usize) -> Status,
    pub signal_event: usize,
    pub close_event: unsafe extern "efiapi" fn(Event) -> Status,
    pub check_event: unsafe extern "efiapi" fn(Event) -> Status,
    pub install_protocol_interface: unsafe extern "efiapi" fn(*mut Handle, *const Guid, u32, *mut c_void) -> Status,
    pub reinstall_protocol_interface: usize,
    pub uninstall_protocol_interface: usize,
    pub handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    pub reserved: usize,
    pub register_protocol_notify: usize,
    pub locate_handle: usize,
    pub locate_device_path: unsafe extern "efiapi" fn(*const Guid, *mut *const u8, *mut Handle) -> Status,
    pub install_configuration_table: usize,
    pub load_image: unsafe extern "efiapi" fn(u8, Handle, *const u8, *const c_void, usize, *mut Handle) -> Status,
    pub start_image: unsafe extern "efiapi" fn(Handle, *mut usize, *mut *mut u16) -> Status,
    pub exit: usize,
    pub unload_image: unsafe extern "efiapi" fn(Handle) -> Status,
    pub exit_boot_services: usize,
    pub get_next_monotonic_count: usize,
    pub stall: unsafe extern "efiapi" fn(usize) -> Status,
    pub set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    pub connect_controller: unsafe extern "efiapi" fn(Handle, *const Handle, *const u8, u8) -> Status,
    pub disconnect_controller: unsafe extern "efiapi" fn(Handle, Handle, Handle) -> Status,
    pub open_protocol: usize,
    pub close_protocol: usize,
    pub open_protocol_information: usize,
    pub protocols_per_handle: usize,
    pub locate_handle_buffer: unsafe extern "efiapi" fn(u32, *const Guid, *const c_void, *mut usize, *mut *mut Handle) -> Status,
    pub locate_protocol: unsafe extern "efiapi" fn(*const Guid, *const c_void, *mut *mut c_void) -> Status,
    pub install_multiple_protocol_interfaces: usize,
    pub uninstall_multiple_protocol_interfaces: usize,
    pub calculate_crc32: unsafe extern "efiapi" fn(*const c_void, usize, *mut u32) -> Status,
    pub copy_mem: usize,
    pub set_mem: usize,
    pub create_event_ex: usize,
}

/// The runtime services table in the specification's order.
#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    pub get_time: usize,
    pub set_time: usize,
    pub get_wakeup_time: usize,
    pub set_wakeup_time: usize,
    pub set_virtual_address_map: usize,
    pub convert_pointer: usize,
    pub get_variable: unsafe extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut c_void) -> Status,
    pub get_next_variable_name: unsafe extern "efiapi" fn(*mut usize, *mut u16, *mut Guid) -> Status,
    pub set_variable: unsafe extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const c_void) -> Status,
    pub get_next_high_monotonic_count: usize,
    pub reset_system: unsafe extern "efiapi" fn(u32, Status, usize, *const c_void) -> !,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

pub const SCAN_UP: u16 = 0x01;
pub const SCAN_DOWN: u16 = 0x02;

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: unsafe extern "efiapi" fn(*mut SimpleTextInput, u8) -> Status,
    pub read_key_stroke: unsafe extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: usize,
    pub output_string: usize,
    pub test_string: usize,
    pub query_mode: usize,
    pub set_mode: usize,
    pub set_attribute: usize,
    pub clear_screen: usize,
    pub set_cursor_position: usize,
    pub enable_cursor: unsafe extern "efiapi" fn(*mut SimpleTextOutput, u8) -> Status,
    pub mode: *const c_void,
}

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *const SystemTable,
    pub device_handle: Handle,
    pub file_path: *const u8,
}

/// `EFI_BLOCK_IO_MEDIA`, revision 1.
#[repr(C)]
pub struct BlockIoMedia {
    pub media_id: u32,
    pub removable_media: u8,
    pub media_present: u8,
    pub logical_partition: u8,
    pub read_only: u8,
    pub write_caching: u8,
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
}

#[repr(C)]
pub struct BlockIo {
    pub revision: u64,
    pub media: *const BlockIoMedia,
    pub reset: unsafe extern "efiapi" fn(*mut BlockIo, u8) -> Status,
    pub read_blocks: unsafe extern "efiapi" fn(*mut BlockIo, u32, u64, usize, *mut c_void) -> Status,
    pub write_blocks: unsafe extern "efiapi" fn(*mut BlockIo, u32, u64, usize, *const c_void) -> Status,
    pub flush_blocks: unsafe extern "efiapi" fn(*mut BlockIo) -> Status,
}

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    pub close: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub delete: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut c_void) -> Status,
    pub write: unsafe extern "efiapi" fn(*mut File, *mut usize, *const c_void) -> Status,
    pub get_position: usize,
    pub set_position: unsafe extern "efiapi" fn(*mut File, u64) -> Status,
    pub get_info: unsafe extern "efiapi" fn(*mut File, *const Guid, *mut usize, *mut c_void) -> Status,
    pub set_info: unsafe extern "efiapi" fn(*mut File, *const Guid, usize, *const c_void) -> Status,
    pub flush: unsafe extern "efiapi" fn(*mut File) -> Status,
}

#[repr(C)]
pub struct FileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(*mut FileSystem, *mut *mut File) -> Status,
}

/// `EFI_FILE_INFO` up to its name (section 13.5).
pub const FILE_INFO_FIXED: usize = 80;

#[repr(C)]
pub struct LoadFile {
    pub load_file: unsafe extern "efiapi" fn(*mut LoadFile, *const u8, u8, *mut usize, *mut c_void) -> Status,
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct BltPixel {
    pub blue: u8,
    pub green: u8,
    pub red: u8,
    pub reserved: u8,
}

/// `EfiBltVideoFill`, `EfiBltVideoToBltBuffer` and `EfiBltBufferToVideo`.
pub const BLT_VIDEO_FILL: u32 = 0;
pub const BLT_VIDEO_TO_BUFFER: u32 = 1;
pub const BLT_BUFFER_TO_VIDEO: u32 = 2;

#[repr(C)]
pub struct GraphicsModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct GraphicsMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *const GraphicsModeInfo,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct GraphicsOutput {
    pub query_mode: usize,
    pub set_mode: usize,
    pub blt: unsafe extern "efiapi" fn(*mut GraphicsOutput, *mut BltPixel, u32, usize, usize, usize, usize, usize, usize, usize) -> Status,
    pub mode: *const GraphicsMode,
}

#[repr(C)]
pub struct ServiceBinding {
    pub create_child: unsafe extern "efiapi" fn(*mut ServiceBinding, *mut Handle) -> Status,
    pub destroy_child: unsafe extern "efiapi" fn(*mut ServiceBinding, Handle) -> Status,
}

#[repr(C)]
pub struct Dhcp4PacketOption {
    pub op_code: u8,
    pub length: u8,
    pub data: [u8; 3],
}

#[repr(C)]
pub struct Dhcp4ConfigData {
    pub discover_try_count: u32,
    pub discover_timeout: *mut u32,
    pub request_try_count: u32,
    pub request_timeout: *mut u32,
    pub client_address: [u8; 4],
    pub dhcp4_callback: usize,
    pub callback_context: *mut c_void,
    pub option_count: u32,
    pub option_list: *mut *mut Dhcp4PacketOption,
}

#[repr(C)]
pub struct Dhcp4ModeData {
    pub state: u32,
    pub config_data: Dhcp4ConfigData,
    pub client_address: [u8; 4],
    pub client_mac_address: [u8; 32],
    pub server_address: [u8; 4],
    pub router_address: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub lease_time: u32,
    pub reply_packet: *mut c_void,
}

/// `Dhcp4Bound`.
pub const DHCP4_BOUND: u32 = 4;

#[repr(C)]
pub struct Dhcp4 {
    pub get_mode_data: unsafe extern "efiapi" fn(*mut Dhcp4, *mut Dhcp4ModeData) -> Status,
    pub configure: unsafe extern "efiapi" fn(*mut Dhcp4, *const Dhcp4ConfigData) -> Status,
    pub start: unsafe extern "efiapi" fn(*mut Dhcp4, Event) -> Status,
    pub renew_rebind: usize,
    pub release: usize,
    pub stop: unsafe extern "efiapi" fn(*mut Dhcp4) -> Status,
}

/// Device path node types and subtypes (section 10.3).
pub mod node {
    pub const HARDWARE: u8 = 1;
    pub const ACPI: u8 = 2;
    pub const MESSAGING: u8 = 3;
    pub const MEDIA: u8 = 4;
    pub const END: u8 = 0x7f;
    pub const MAC: u8 = 11;
    pub const IPV4: u8 = 12;
    pub const IPV6: u8 = 13;
    pub const URI: u8 = 24;
    pub const HARD_DRIVE: u8 = 1;
    pub const FILE_PATH: u8 = 4;
    pub const RAM_DISK: u8 = 9;
}

pub const END_NODE: [u8; 4] = [0x7f, 0xff, 4, 0];

/// Bytes of a device path before its end node; absent for a node shorter than its header or a path longer than 64
/// nodes.
///
/// # Safety
/// `path` must point at a device path in memory the caller may read.
pub unsafe fn path_length(path: *const u8) -> Option<usize> {
    let mut at = 0;
    for _ in 0..64 {
        let node = core::slice::from_raw_parts(path.add(at), 4);
        if node[0] == END_NODE[0] && node[1] == END_NODE[1] {
            return Some(at);
        }
        let length = u16::from_le_bytes([node[2], node[3]]) as usize;
        if length < 4 {
            return None;
        }
        at += length;
    }
    None
}

/// Whether `bytes` is exactly one device path: nodes that fit, the last of them the end node.
pub fn is_whole_path(bytes: &[u8]) -> bool {
    let mut at = 0;
    for (kind, subtype, bytes_of_node) in Nodes::new(bytes) {
        at += bytes_of_node.len();
        if kind == node::END && subtype == 0xff {
            return at == bytes.len();
        }
    }
    false
}

/// The nodes of a device path, the end node included: (type, subtype, the node's bytes including its header).
pub struct Nodes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Nodes<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
}

impl<'a> Iterator for Nodes<'a> {
    type Item = (u8, u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.bytes.get(self.at..)?;
        if rest.len() < 4 {
            return None;
        }
        let length = u16::from_le_bytes([rest[2], rest[3]]) as usize;
        if length < 4 || length > rest.len() {
            return None;
        }
        self.at += length;
        Some((rest[0], rest[1], &rest[..length]))
    }
}

/// A NUL-terminated UCS-2 string from ASCII, in a fixed buffer.
pub struct Wide<const N: usize> {
    pub units: [u16; N],
}

impl<const N: usize> Wide<N> {
    pub fn new(text: &str) -> Self {
        let mut units = [0u16; N];
        for (unit, byte) in units.iter_mut().zip(text.bytes().take(N - 1)) {
            *unit = byte as u16;
        }
        Self { units }
    }

    pub fn as_ptr(&self) -> *const u16 {
        self.units.as_ptr()
    }
}

pub fn ok(status: Status) -> Result<(), Status> {
    if status == SUCCESS { Ok(()) } else { Err(status) }
}
