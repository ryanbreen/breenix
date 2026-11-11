// Compile the shipped command so the no-table check reaches its real I/O path.
include!("../../../userspace/programs/src/mount.rs");

#[test]
fn absent_shares_table_does_nothing() {
    let directory = std::env::temp_dir().join(format!("mount-no-table-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let table = directory.join("shares");
    assert!(!table.exists());
    assert_eq!(mount_all_from(&table), Ok(()));
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    std::fs::remove_dir(directory).unwrap();
}
