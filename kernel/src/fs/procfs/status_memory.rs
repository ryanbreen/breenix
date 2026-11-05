use alloc::{format, string::String};

pub(super) fn vm_pte(table_frames: usize) -> String {
    format!("VmPTE:\t{} kB\n", table_frames * 4)
}
