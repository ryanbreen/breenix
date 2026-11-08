//! Live reader lifetime, serialization count and access-control checks.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{memory, window::{self, abi::{self, op}}};
use libuitest::Session;

pub(crate) struct TreeReader(abi::MapTree);
impl TreeReader {
    pub(crate) fn attach(id: u64) -> Result<Self, libbreenix::error::Error> {
        let mut map = abi::MapTree { id, ..Default::default() };
        window::call(op::MAP_TREE, &mut map)?;
        Ok(Self(map))
    }
}
impl Drop for TreeReader {
    fn drop(&mut self) { let _ = memory::munmap(self.0.addr as *mut u8, self.0.len as usize); }
}

pub fn no_reader_no_serialize() -> CaseResult {
    let mut session = Session::connect()?;
    let mut app = crate::frames::App::launch(&mut session)?;
    let first = app.command("create-tree")?;
    const N: u64 = 11;
    let count = || window::read_meta(first.id).map(|(_, meta)| meta.tree_serializations);
    check(count()? == 0, "initial unattached commit serialized")?;
    for _ in 0..N { app.command("commit 0")?; }
    check(count()? == 0, "unattached commits serialized")?;
    let reader = TreeReader::attach(first.id)?;
    for i in 1..=N {
        app.command("commit 0")?;
        check(count()? == i, "attached commit did not serialize exactly once")?;
    }
    drop(reader);
    for _ in 0..N { app.command("commit 0")?; }
    check(count()? == N, "unmapped reader still triggered serialization")?;
    // A different address space maps and exits without explicitly unmapping.
    let mut attach_and_exit = || -> i32 {
        match TreeReader::attach(first.id) {
            Ok(mapping) => { std::mem::forget(mapping); 0 }
            Err(_) => 1,
        }
    };
    let pid = breenix_suite::suite::spawn(&[], breenix_suite::suite::Run::Call(&mut attach_and_exit))?;
    let mut status = -1;
    libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0)?;
    check(status == 0, "reader helper did not map and exit successfully")?;
    for _ in 0..N { app.command("commit 0")?; }
    check(count()? == N, "exited process still triggered serialization")?;
    app.command("quit")?;
    session.wait(app.pid)?;
    session.teardown()?;
    Ok(())
}

pub fn permission() -> CaseResult {
    let mut session = Session::connect()?;
    let mut app = crate::frames::App::launch(&mut session)?;
    let first = app.command("create-tree")?;
    let mut denied = || -> i32 {
        extern "C" { fn setuid(uid: u32) -> i32; fn getuid() -> u32; fn geteuid() -> u32; }
        if unsafe { setuid(1000) } != 0 || unsafe { getuid() } != 1000 || unsafe { geteuid() } != 1000 { return 2; }
        let mut map = abi::MapTree { id: first.id, ..Default::default() };
        match window::call(op::MAP_TREE, &mut map) {
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EPERM)) => 0,
            _ => 3,
        }
    };
    let pid = breenix_suite::suite::spawn(&[], breenix_suite::suite::Run::Call(&mut denied))?;
    let mut status = -1;
    libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0)?;
    check(status == 0, "unrelated uid1000 MAP_TREE must return EPERM")?;
    app.command("quit")?;
    session.wait(app.pid)?;
    session.teardown()?;
    Ok(())
}
