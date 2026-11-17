//! read(2)'s regular-file arm copies through the bounded helper (#3040).
//!
//! `src/lib.rs` checks the helper's behavior; this ties sys_read to it, so a
//! kernel buffer the size of the request cannot come back in that arm.

const HANDLERS: &str = include_str!("../../../kernel/src/syscall/handlers.rs");

fn regular_file_arm() -> &'static str {
    let sys_read = HANDLERS.find("pub fn sys_read(").expect("sys_read in handlers.rs");
    let rest = &HANDLERS[sys_read..];
    let start = rest
        .find("FdKind::RegularFile(file_ref) => {")
        .expect("sys_read's regular-file arm");
    let arm = &rest[start..];
    let end = arm.find("FdKind::Directory(_) =>").expect("the arm after it");
    &arm[..end]
}

#[test]
fn regular_file_read_uses_the_bounded_buffer() {
    let arm = regular_file_arm();
    assert!(
        arm.contains("regular_read::read(") && arm.contains("regular_read::CHUNK"),
        "sys_read's regular-file arm must copy through regular_read::read with CHUNK:\n{arm}"
    );
    assert!(
        !arm.contains("try_reserve") && !arm.contains("Vec::"),
        "sys_read's regular-file arm allocates its own buffer:\n{arm}"
    );
}
