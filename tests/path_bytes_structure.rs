//! Path syscalls must carry POSIX pathname bytes to the directory-entry layer.
use std::{fs, path::Path};

#[test]
fn path_syscalls_do_not_decode_user_pathnames_as_utf8() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in ["fs.rs", "metadata.rs", "fifo.rs"] {
        let source = fs::read_to_string(root.join("kernel/src/syscall").join(file)).unwrap();
        assert!(!source.contains("copy_cstr_from_user"), "{file} decodes a pathname as text");
        assert!(source.contains("copy_path_bytes_from_user"), "{file} must copy pathname bytes");
    }
    let source = fs::read_to_string(root.join("kernel/src/fs/namei.rs")).unwrap();
    for name in ["resolve_from", "resolve_create_from", "resolve_entry_from", "resolve_rename_from"] {
        let function = source.split(&format!("fn {name}(" )).nth(1).unwrap().split("\n}").next().unwrap();
        assert!(function.contains("path: &[u8]"), "{name} must take pathname bytes");
        assert!(function.contains("walk_bytes("), "{name} must use the byte walk");
        assert!(!function.contains("text_path("), "{name} must preserve the final name");
    }
}
