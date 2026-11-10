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
daemon = (ROOT / "userspace/programs/src/smbfsd.rs").read_text()
reply_start = daemon.index("        let written = loop {")
reply_end = daemon.index("        if written != n", reply_start)
reply = daemon[reply_start:reply_end]
harness = r'''
use std::io;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};
struct Mutex<T>(StdMutex<T>);
impl<T> Mutex<T> {
    fn lock(&self) -> MutexGuard<'_, T> { self.0.lock().unwrap() }
}
mod ufs_abi { pub mod payload { pub const ROOT: u64 = 1; } }
mod crate_fs {
    pub mod namei {
        pub enum Target { User { mount: u64, node: u64 } }
    }
}
use crate_fs as fs;
const EINVAL: i32 = 22;
pub const EBUSY: i32 = 16;
struct State { dead: bool, detached: bool, open_files: usize }
impl State { fn disconnect(&mut self) { self.dead = true; } }
struct Mount { id: u64, state: Mutex<State>, woken: AtomicBool }
impl Mount { fn wake(&self) { self.woken.store(true, Ordering::SeqCst); } }
struct Point { target: fs::namei::Target }
static MOUNTS: Mutex<Vec<Arc<Mount>>> = Mutex(StdMutex::new(Vec::new()));
fn unmount(point: Point) -> Result<(), u64> {
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
    let mount = Arc::new(Mount {
        id: 9, state: Mutex(StdMutex::new(State {
            dead: false, detached: false, open_files: 1,
        })), woken: AtomicBool::new(false),
    });
    MOUNTS.lock().push(mount.clone());
    assert_eq!(unmount(Point { target: fs::namei::Target::User { mount: 9, node: 1 } }), Ok(()));
    assert!(MOUNTS.lock().is_empty(), "later fd calls cannot find the detached mount");
    let state = mount.state.lock();
    assert!(state.dead && state.detached, "endpoint must observe detached EOF");
    assert_eq!(state.open_files, 1, "detach must not consume the open lease");
    assert!(mount.woken.load(Ordering::SeqCst));
}
'''.replace("BODY", body).replace("REPLY", reply)
with tempfile.TemporaryDirectory(prefix="breenix-ufs-unmount-") as temporary:
    path = Path(temporary)
    test = path / "check.rs"
    test.write_text(harness)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(test), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
