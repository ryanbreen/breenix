#[path = "../userspace/programs/src/suite_breengel/backend_row.rs"]
mod backend_row;

use backend_row::{backend_row, BackendRow, CopyFields};

#[test]
fn backend_rows_match_the_contract() {
    assert_eq!(backend_row(4), Some(BackendRow { status: 0x1100, render: 0x207, readback: 0x206, boundary: 4, copy_fields: CopyFields::Zero }));
    assert_eq!(backend_row(5), Some(BackendRow { status: 1, render: 1152, readback: 1104, boundary: 4, copy_fields: CopyFields::Vmware }));
    assert_eq!(backend_row(6), Some(BackendRow { status: 0x60000000, render: 0xa1971b0c, readback: 0xa0b50300, boundary: 5, copy_fields: CopyFields::Zero }));
    assert_eq!(backend_row(0), None);
}
