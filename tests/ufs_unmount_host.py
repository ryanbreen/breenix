#!/usr/bin/env python3
"""Exercise the production detach block with a live open-file lease."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/fs/ufs/mod.rs").read_text()
start = source.index("    let mount = {", source.index("fn umount("))
end = source.index("\n}\n", start)
body = source[start:end]
flags_start = source.index("    if flags", source.index("fn umount("))
flags_end = source.index("    let raw", flags_start)
flags_check = source[flags_start:flags_end]
daemon = (ROOT / "userspace/programs/src/smbfsd.rs").read_text()
reply_start = daemon.index("        let written = loop {")
reply_end = daemon.index("        if written != n", reply_start)
reply = daemon[reply_start:reply_end]
harness = r'''
extern crate alloc;
#[path = "@ROOT@/kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "@ROOT@/kernel/src/fs/errno.rs"]
pub mod errno;
#[path = "@ROOT@/kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod syscall { pub use crate::syscall_errno as errno; }
#[path = "@ROOT@/kernel/src/fs/vfs/mount_table.rs"]
pub mod mount_table;
use std::io;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};
struct Mutex<T>(StdMutex<T>);
impl<T> Mutex<T> {
    fn lock(&self) -> MutexGuard<'_, T> { self.0.lock().unwrap() }
}
mod ufs_abi { pub mod payload { pub const ROOT: u64 = 1; } }
mod crate_fs {
    pub use crate::{attrs, errno};
    pub mod vfs { pub mod mount {
        use crate::{mount_table::MountTable, errno::Errno, Mutex, StdMutex};
        pub static TABLE: Mutex<MountTable<()>> = Mutex(StdMutex::new(MountTable::new()));
        pub fn unmount(id: usize, force: bool) -> Result<(), Errno> {
            let removed = TABLE.lock().remove(id, force)?;
            drop(removed);
            Ok(())
        }
    } }
    pub mod namei {
        pub enum Target { User { mount: u64, node: u64 } }
    }
}
use crate_fs as fs;
const EINVAL: i32 = 22;
pub const EBUSY: i32 = 16;
struct State { dead: bool, detached: bool, open_files: usize }
impl State { fn disconnect(&mut self) { self.dead = true; } }
struct Pin(Arc<AtomicBool>);
impl Drop for Pin { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
struct Mount { id: u64, state: Mutex<State>, woken: AtomicBool }
impl Mount { fn wake(&self) { self.woken.store(true, Ordering::SeqCst); } }
struct Point { target: fs::namei::Target }
static MOUNTS: Mutex<Vec<Arc<Mount>>> = Mutex(StdMutex::new(Vec::new()));
fn unmount(point: Point, flags: u64) -> Result<(), u64> {
    FLAGS_CHECK
    BODY
}
struct Dev(i32);
impl Dev {
    fn write(&self, _: &[u8]) -> io::Result<usize> { Err(io::Error::from_raw_os_error(self.0)) }
}
fn reply_after_detach(dev: Dev) -> Result<(), String> {
    let output = [0];
    let n = 1;
    REPLY
    assert_eq!(written, n);
    Ok(())
}
#[test]
fn daemon_exits_cleanly_if_detach_races_a_reply() {
    assert_eq!(reply_after_detach(Dev(107)), Ok(()));
    assert!(reply_after_detach(Dev(5)).is_err(), "unrelated reply errors remain failures");
}
#[test]
fn detach_succeeds_with_a_live_open_file_and_wakes_the_daemon() {
    for flags in [1, 2] {
    let unpinned = Arc::new(AtomicBool::new(false));
    let mount = Arc::new(Mount {
        id: 9, state: Mutex(StdMutex::new(State {
            dead: false, detached: false, open_files: 1,
        })), woken: AtomicBool::new(false),
    });
    MOUNTS.lock().push(mount.clone());
    let mut entry = mount_table::Entry::new(9, b"/mnt".to_vec(), "ufs", attrs::MountFlags::default(),
        (), 1, Some((0, 42)), Some((0, 2)));
    entry.custody = Some(Arc::new(Pin(unpinned.clone())));
    fs::vfs::mount::TABLE.lock().insert(entry).unwrap();
    let held = fs::vfs::mount::TABLE.lock().pin(9).unwrap();
    let point = || Point { target: fs::namei::Target::User { mount: 9, node: 1 } };
    assert_eq!(unmount(point(), 0), Err(EBUSY as u64));
    assert_eq!(unmount(point(), 4), Err(EINVAL as u64));
    assert_eq!(MOUNTS.lock().len(), 1);
    {
        let state = mount.state.lock();
        assert!(!state.dead && !state.detached);
        assert_eq!(state.open_files, 1);
    }
    assert!(!mount.woken.load(Ordering::SeqCst));
    assert!(fs::vfs::mount::TABLE.lock().get(9).unwrap().custody.is_some());
    assert!(!unpinned.load(Ordering::SeqCst));
    assert_eq!(unmount(point(), flags), Ok(()));
    assert!(unpinned.load(Ordering::SeqCst), "detach must release the covered directory even while the daemon or an open file retains the mount");
    assert!(fs::vfs::mount::TABLE.lock().get(9).is_none());
    drop(held);
    assert!(MOUNTS.lock().is_empty(), "later fd calls cannot find the detached mount");
    let state = mount.state.lock();
    assert!(state.dead && state.detached, "endpoint must observe detached EOF");
    assert_eq!(state.open_files, 1, "detach must not consume the open lease");
    assert!(mount.woken.load(Ordering::SeqCst));
    }
}
'''.replace("@ROOT@", str(ROOT)).replace("BODY", body).replace("REPLY", reply).replace("FLAGS_CHECK", flags_check)
with tempfile.TemporaryDirectory(prefix="breenix-ufs-unmount-") as temporary:
    path = Path(temporary)
    test = path / "check.rs"
    test.write_text(harness)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(test), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
