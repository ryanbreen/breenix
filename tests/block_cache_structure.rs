use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    let source = PathBuf::from(file!());
    let source = if source.is_absolute() { source } else { std::env::current_dir().unwrap().join(source) };
    source.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn ext2_uses_the_shared_cache() {
    let root = root();
    assert!(!root.join("kernel/src/fs/ext2/sector_cache.rs").exists());
    let ext2 = fs::read_to_string(root.join("kernel/src/fs/ext2/mod.rs")).unwrap();
    assert!(!ext2.contains("mod sector_cache"));
    let host = fs::read_to_string(root.join("tools/ext2-host-tests/src/ext2.rs")).unwrap();
    assert!(host.contains("kernel/src/block/cache.rs"));
    assert!(!host.contains("kernel/src/fs/ext2/sector_cache.rs"));
}

#[test]
fn cache_does_not_name_the_process_manager() {
    let cache = fs::read_to_string(root().join("kernel/src/block/cache.rs")).unwrap();
    assert!(!cache.contains("PROCESS_MANAGER"));
}
