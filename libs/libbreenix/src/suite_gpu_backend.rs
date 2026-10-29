//! Suite-owned expectations for backend evidence (gpu.md section 1.8).
#[derive(Clone, Copy)]
pub struct BackendValues {
    pub success: u32,
    pub draw: u32,
    pub upload: u32,
    pub readback: u32,
    pub copy: u32,
    pub boundary: u32,
}

pub fn backend_values(backend: u32) -> Option<BackendValues> {
    match backend {
        4 => Some(BackendValues { success: 0x1100, draw: 0x207, upload: 0x205, readback: 0x206, copy: 0, boundary: 4 }),
        5 => Some(BackendValues { success: 1, draw: 1152, upload: 1102, readback: 1104, copy: 1042, boundary: 4 }),
        6 => Some(BackendValues { success: 0x60000000, draw: 0xa1971b0c, upload: 0xa0b50300, readback: 0xa0b50300, copy: 0xa0b50300, boundary: 5 }),
        _ => None,
    }
}

pub fn native_classes(backend: u32, features: u32, ids: &[u32]) -> bool {
    backend == 6 && features & (1 << 9) != 0 && ids == [0x927d, 0xa06f, 0xa0b5, 0xa197]
}

pub fn device_error(backend: u32, answer: u32) -> bool {
    match backend {
        4 => (0x1200..=0x12ff).contains(&answer),
        5 => answer == 3,
        6 => (0x60010000..=0x6001000f).contains(&answer),
        _ => false,
    }
}
