#!/usr/bin/env python3
"""Run the breenfs dispatch against the production mount-table custody checks."""
from pathlib import Path
import hashlib
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ufs = (ROOT / "kernel/src/fs/ufs/mod.rs").read_text()
start = ufs.index("fn umount(")
fallback = ufs[ufs.index("    let mount = {", start):ufs.index("\npub fn sys_mount", start)]
assert hashlib.sha256(fallback.encode()).hexdigest() == "a50c38aa6be649eef5a341d8536cbb0791f8f4b8a2173e9c55f8e64ba8804c61", "existing UFS path changed"
dispatch = "    if let Some(result) = crate::fs::breenfs::mount::unmount(&point.target, flags) {\n        return result;\n    }\n"
assert dispatch in ufs[start:ufs.index("    let mount = {", start)], "native breenfs unmount has no dispatch"
source = (ROOT / "kernel/src/fs/breenfs/mount.rs").read_text()
body = source[source.index("pub fn unmount("):]
harness = r'''
extern crate alloc;
#[path = "@ROOT@/kernel/src/fs/attrs.rs"] pub mod attrs;
#[path = "@ROOT@/kernel/src/fs/errno.rs"] pub mod errno;
#[path = "@ROOT@/kernel/src/syscall/errno.rs"] pub mod syscall_errno;
mod syscall { pub use crate::syscall_errno as errno; }
mod fs { pub use crate::{attrs, errno}; }
#[path = "@ROOT@/kernel/src/fs/vfs/mount_table.rs"] pub mod mount_table;
use std::sync::{Arc, Mutex};
static EVENTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
#[allow(dead_code)]
struct Fs;
#[allow(dead_code)]
impl Fs {
    fn shutdown(&self, _name: &str, _deadline: u64) -> Result<(), errno::Errno> {
        EVENTS.lock().unwrap().push("commit-stop"); Ok(())
    }
    fn init_service(&self) -> Result<(), errno::Errno> {
        EVENTS.lock().unwrap().push("resume"); Ok(())
    }
}
#[allow(dead_code)]
mod time {
    pub fn get_monotonic_time_ns() -> (u64, u32) { (0, 0) }
}
#[derive(Clone, Copy)] pub struct Mount(pub usize);
pub enum Target { Inode { mount: Mount, ino: u64 }, User, Virtual, Absent }
mod registry {
    use super::*;
    pub static TABLE: Mutex<mount_table::MountTable<()>> = Mutex::new(mount_table::MountTable::new());
    pub fn get_mount_info(id: usize) -> Option<(Vec<u8>, &'static str)> {
        TABLE.lock().unwrap().get(id).map(|e| (e.path.clone(), e.kind))
    }
    pub fn root_inode(id: usize) -> Option<u64> { Some(TABLE.lock().unwrap().get(id)?.root) }
    pub fn covered_directory(id: usize) -> Option<(usize, u64)> { TABLE.lock().unwrap().get(id)?.cover }
    #[allow(dead_code)]
    pub fn filesystem(_id: usize) -> Result<Arc<Fs>, errno::Errno> { Ok(Arc::new(Fs)) }
    pub fn unmount(id: usize, force: bool) -> Result<(), errno::Errno> {
        EVENTS.lock().unwrap().push("unregister");
        let removed = TABLE.lock().unwrap().remove(id, force)?;
        drop(removed);
        Ok(())
    }
}
BODY
#[test]
fn only_breenfs_roots_dispatch_and_custody_is_preserved() {
    for (id, kind, cover) in [(1, "ext2", None), (2, "ufs", Some((1, 16))),
        (3, "breenfs", Some((1, 17))), (4, "breenfs", None)] {
        registry::TABLE.lock().unwrap().insert(mount_table::Entry::new(
            id, format!("/mnt{id}").into_bytes(), kind, attrs::MountFlags::default(),
            (), 2, cover, Some((1, 2)))).unwrap();
    }
    let target = |id, ino| Target::Inode { mount: Mount(id), ino };
    for t in [Target::User, Target::Virtual, Target::Absent, target(1, 2),
        target(2, 2), target(3, 16), target(4, 2), target(99, 2)] {
        assert_eq!(unmount(&t, 0), None, "existing targets must fall through");
    }
    let held = registry::TABLE.lock().unwrap().pin(3).unwrap();
    assert_eq!(unmount(&target(3, 2), 0), Some(Err(syscall_errno::EBUSY as u64)));
    assert!(registry::TABLE.lock().unwrap().get(3).is_some());
    assert_eq!(*EVENTS.lock().unwrap(), ["commit-stop", "unregister", "resume"]);
    EVENTS.lock().unwrap().clear();
    drop(held);
    assert_eq!(unmount(&target(3, 2), 0), Some(Ok(())));
    assert!(registry::TABLE.lock().unwrap().get(3).is_none());
    assert_eq!(*EVENTS.lock().unwrap(), ["commit-stop", "unregister"]);
    assert!(registry::TABLE.lock().unwrap().get(1).is_some());
    assert!(registry::TABLE.lock().unwrap().get(2).is_some());
    let _: Option<Arc<()>> = None;
}
'''.replace("@ROOT@", str(ROOT)).replace("BODY", body)
with tempfile.TemporaryDirectory(prefix="breenfs-unmount-") as temporary:
    path = Path(temporary)
    (path / "check.rs").write_text(harness)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(path / "check.rs"), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check"), "only_breenfs_roots_dispatch"], check=True)
