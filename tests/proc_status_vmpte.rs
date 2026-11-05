extern crate alloc;

#[path = "../kernel/src/fs/procfs/status_memory.rs"]
mod status_memory;

#[test]
fn status_reports_page_table_frames_in_kibibytes() {
    for frames in [0, 1, 7, 123] {
        let text = status_memory::vm_pte(frames);
        assert_eq!(text, format!("VmPTE:\t{} kB\n", frames * 4));
    }
}

#[test]
fn status_uses_the_process_page_table_count() {
    let source = include_str!("../kernel/src/fs/procfs/mod.rs");
    let status = source.split("fn generate_pid_status(").nth(1).unwrap()
        .split("fn generate_pid_stat(").next().unwrap();
    assert!(status.contains("table.table_frames()"));
    assert!(status.contains("status_memory::vm_pte("));
}
