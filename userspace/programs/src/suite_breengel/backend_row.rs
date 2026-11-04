#[derive(Debug, PartialEq, Eq)]
pub enum CopyFields {
    Zero,
    Vmware,
    Native,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BackendRow {
    pub status: u32,
    pub render: u32,
    pub readback: u32,
    pub boundary: u32,
    pub copy_fields: CopyFields,
}

pub fn backend_row(backend: u32) -> Option<BackendRow> {
    match backend {
        4 => Some(BackendRow { status: 0x1100, render: 0x207, readback: 0x206, boundary: 4, copy_fields: CopyFields::Zero }),
        5 => Some(BackendRow { status: 1, render: 1152, readback: 1104, boundary: 4, copy_fields: CopyFields::Vmware }),
        6 => Some(BackendRow { status: 0x60000000, render: 0xa1971b0c, readback: 0xa0b50300, boundary: 5, copy_fields: CopyFields::Native }),
        _ => None,
    }
}
