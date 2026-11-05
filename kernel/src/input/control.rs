//! Suite-latched completion replay, harvested from e753f9a2.
//! USB HID and the ARM input driver register the same bus-independent interface.
use crate::{ipc::object::IpcFile, syscall::{errno::*, userptr}};
use alloc::{sync::Arc, vec::Vec};

#[derive(Clone, Copy, Debug, Default)]
pub struct Receipt {
    pub calls: u32,
    pub posted: u32,
    pub ring: u32,
}

/// Implemented by bound drivers. A successful receipt follows completion and requeue.
pub trait Replay: Sync {
    fn replay_keyboard(&self, suppress: bool) -> Result<Receipt, u64>;
    fn replay_pointer(&self, x: i32, y: i32, suppress: bool) -> Result<Receipt, u64>;
}
static BOUND: spin::Mutex<[Option<&'static dyn Replay>; 2]> = spin::Mutex::new([None, None]);

/// First bound keyboard/pointer wins; the driver revalidates its instance on each replay.
pub fn register(driver: &'static dyn Replay, pointer: bool) {
    BOUND.lock()[usize::from(pointer)].get_or_insert(driver);
}
fn authorize() -> Result<(), u64> {
    if !crate::test_control::enabled() { return Err(ENOSYS as u64); }
    if !crate::window::Caller::current().map_err(|e| e as u64)?.is_root() {
        return Err(EPERM as u64);
    }
    Ok(())
}
#[derive(Debug)]
struct State { reply: Vec<u8>, at: usize }
#[derive(Debug)]
pub struct Control { state: spin::Mutex<State> }

pub fn open(path: &str) -> Result<Option<Arc<dyn IpcFile>>, u64> {
    if path != "/proc/breenix/input-replay" { return Ok(None); }
    authorize()?;
    Ok(Some(Arc::new(Control { state: spin::Mutex::new(State { reply: Vec::new(), at: 0 }) })))
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn replay(bytes: &[u8]) -> Result<Vec<u8>, u64> {
    if bytes.len() != 64 { return Err(EINVAL as u64); }
    let which = u32_at(bytes, 0);
    if ![1, 2, 0x101, 0x102].contains(&which)
        || bytes[4..8] != [0; 4]
        || bytes[8..16] != [0, 0, 4, 0, 0, 0, 0, 0]
        || bytes[24..].iter().any(|b| *b != 0) {
        return Err(EINVAL as u64);
    }
    let x = u32_at(bytes, 16) as i32;
    let y = u32_at(bytes, 20) as i32;
    if which & 0xff == 2 {
        let (width, height) = super::pointer::screen().ok_or(ENODEV as u64)?;
        if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
            return Err(EINVAL as u64);
        }
    }
    let driver = BOUND.lock()[usize::from(which & 0xff == 2)].ok_or(ENODEV as u64)?;
    let suppress = which & 0x100 != 0;
    let receipt = match which & 0xff {
        1 => driver.replay_keyboard(suppress),
        2 => driver.replay_pointer(x, y, suppress),
        _ => Err(EINVAL as u64),
    }?;
    let mut reply = bytes.to_vec();
    for (at, value) in [(32, receipt.calls), (36, receipt.posted), (40, receipt.ring)] {
        reply[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(reply)
}
impl IpcFile for Control {
    fn as_any(&self) -> &dyn core::any::Any { self }
    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        st.st_mode = crate::syscall::fs::S_IFREG | 0o600;
        st.st_nlink = 1;
        st.st_blksize = 64;
    }
    fn statfs_magic(&self) -> u64 { 0x9fa0 }
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        authorize()?;
        let mut state = self.state.lock();
        let n = count.min(state.reply.len() - state.at);
        userptr::write_user_bytes(buf, state.reply[state.at..].as_ptr(), n)?;
        state.at += n;
        Ok(n as u64)
    }
    fn write(&self, bytes: &[u8], _append: bool) -> Result<u64, u64> {
        authorize()?;
        // The driver may block for a real completion; no descriptor guard may
        // survive the wait's preemption release.
        let reply = replay(bytes)?;
        let mut state = self.state.lock();
        state.reply = reply;
        state.at = 0;
        Ok(bytes.len() as u64)
    }
}
