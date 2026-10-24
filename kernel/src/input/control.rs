//! Suite-latched decoder and bound-completion observations (W1/W8).
use crate::drivers::{
    hid_boot::{pointer, report::Keyboard},
    virtio::{
        input_decode,
        input_mmio::{self, VirtioInputEvent},
    },
};
use crate::{
    ipc::object::IpcFile,
    syscall::{errno::*, userptr},
};
use alloc::{sync::Arc, vec::Vec};

fn authorize() -> Result<(), u64> {
    if !crate::test_control::enabled() {
        return Err(ENOSYS as u64);
    }
    if !crate::window::Caller::current()
        .map_err(|e| e as u64)?
        .is_root()
    {
        return Err(EPERM as u64);
    }
    Ok(())
}
struct State {
    reply: Vec<u8>,
    at: usize,
    keyboard: Keyboard,
    buttons: [u32; 32],
}
pub struct Control {
    decode: bool,
    state: spin::Mutex<State>,
}
impl core::fmt::Debug for Control {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InputControl")
            .field("decode", &self.decode)
            .finish()
    }
}
pub fn open(path: &str) -> Result<Option<Arc<dyn IpcFile>>, u64> {
    let decode = match path {
        "/proc/breenix/input-decode" => true,
        "/proc/breenix/input-replay" => false,
        _ => return Ok(None),
    };
    authorize()?;
    Ok(Some(Arc::new(Control {
        decode,
        state: spin::Mutex::new(State {
            reply: Vec::new(),
            at: 0,
            keyboard: Keyboard::new(),
            buttons: [0; 32],
        }),
    })))
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn event(kind: u16, code: u16, value: i32) -> VirtioInputEvent {
    VirtioInputEvent {
        event_type: kind,
        code,
        value: value as u32,
    }
}
impl Control {
    fn replay(&self, bytes: &[u8]) -> Result<Vec<u8>, u64> {
        if bytes.len() != 64 {
            return Err(EINVAL as u64);
        }
        let which = u32_at(bytes, 0);
        if ![1, 2, 0x101, 0x102].contains(&which)
            || bytes[4..8] != [0; 4]
            || bytes[8..16] != [0, 0, 4, 0, 0, 0, 0, 0]
            || bytes[24..].iter().any(|b| *b != 0)
        {
            return Err(EINVAL as u64);
        }
        let tablet = which & 0xff == 2;
        let (width, height) = crate::input::pointer::screen().ok_or(ENODEV as u64)?;
        let x = u32_at(bytes, 16) as i32;
        let y = u32_at(bytes, 20) as i32;
        if tablet && (x < 0 || y < 0 || x as u32 >= width || y as u32 >= height) {
            return Err(EINVAL as u64);
        }
        // One complete recorded frame retains both edges in the input stream.
        let events = if tablet {
            let ax = ((x as u64 * 32767 + width.saturating_sub(2) as u64)
                / width.saturating_sub(1).max(1) as u64) as i32;
            let ay = ((y as u64 * 32767 + height.saturating_sub(2) as u64)
                / height.saturating_sub(1).max(1) as u64) as i32;
            alloc::vec![
                event(3, 0, ax),
                event(3, 1, ay),
                event(1, 272, 1),
                event(1, 272, 0),
                event(0, 0, 0)
            ]
        } else {
            alloc::vec![event(1, 30, 1), event(1, 30, 0), event(0, 0, 0)]
        };
        // Bit 8 is the contract's report-suppression point, not a second path.
        let suppression = if which & 0x100 != 0 {
            Some(crate::test_control::arm_input_report_skip()?)
        } else {
            None
        };
        let result = input_mmio::replay(tablet, &events);
        drop(suppression);
        let (calls, posted, ring) = result?;
        let mut reply = bytes.to_vec();
        for (at, value) in [(32, calls), (36, posted), (40, ring)] {
            reply[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(reply)
    }
    fn decode(&self, bytes: &[u8], state: &mut State) -> Result<Vec<u8>, u64> {
        if bytes.len() < 8 {
            return Err(EINVAL as u64);
        }
        match u32_at(bytes, 0) {
            1 => {
                if bytes.len() != 16 || u32_at(bytes, 4) != 0 {
                    return Err(EINVAL as u64);
                }
                let e =
                    input_decode::decode(bytes[8..16].try_into().unwrap()).ok_or(EINVAL as u64)?;
                let mut reply = Vec::new();
                reply.extend_from_slice(&e.kind.to_le_bytes());
                reply.extend_from_slice(&e.code.to_le_bytes());
                reply.extend_from_slice(&e.value.to_le_bytes());
                Ok(reply)
            }
            2 => {
                if bytes.len() < 20 {
                    return Err(EINVAL as u64);
                }
                let endpoint = u32_at(bytes, 4) as usize;
                let descriptor = u32_at(bytes, 8) as usize;
                let report = u32_at(bytes, 12) as usize;
                let reset = u32_at(bytes, 16);
                if endpoint >= state.buttons.len()
                    || reset > 1
                    || descriptor > 4096
                    || report > 4096
                    || bytes.len() != 20 + descriptor + report
                {
                    return Err(EINVAL as u64);
                }
                let layout = pointer::Layout::parse(&bytes[20..20 + descriptor])
                    .map_err(|_| EINVAL as u64)?;
                let motion = layout
                    .decode(&bytes[20 + descriptor..])
                    .map_err(|_| EINVAL as u64)?
                    .ok_or(EINVAL as u64)?;
                if reset == 1 {
                    state.buttons.fill(0);
                }
                if let Some(buttons) = motion.buttons {
                    state.buttons[endpoint] = buttons;
                }
                let mut reply = Vec::new();
                for value in [
                    motion.x.map_or(0, |a| a.value),
                    motion.y.map_or(0, |a| a.value),
                    pointer::merged_buttons(state.buttons.iter().copied()) as i32,
                    motion.wheel,
                ] {
                    reply.extend_from_slice(&value.to_le_bytes());
                }
                Ok(reply)
            }
            3 => {
                if bytes.len() != 16 || u32_at(bytes, 4) > 1 {
                    return Err(EINVAL as u64);
                }
                if u32_at(bytes, 4) == 1 {
                    state.keyboard = Keyboard::new();
                }
                let mut edges = Vec::new();
                let summary = state
                    .keyboard
                    .report(&bytes[8..], 0, &mut |code, value| {
                        edges.extend_from_slice(&1u16.to_le_bytes());
                        edges.extend_from_slice(&code.to_le_bytes());
                        edges.extend_from_slice(&value.to_le_bytes());
                    })
                    .map_err(|_| EINVAL as u64)?;
                let mut reply = Vec::new();
                reply.extend_from_slice(&u32::from(summary.phantom).to_le_bytes());
                reply.extend_from_slice(&(edges.len() as u32 / 8).to_le_bytes());
                reply.extend(edges);
                Ok(reply)
            }
            _ => Err(EINVAL as u64),
        }
    }
}
impl IpcFile for Control {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        st.st_mode = crate::syscall::fs::S_IFREG | 0o600;
        st.st_nlink = 1;
        st.st_blksize = 64;
    }
    fn statfs_magic(&self) -> u64 {
        0x9fa0
    }
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
        let mut state = self.state.lock();
        let reply = if self.decode {
            self.decode(bytes, &mut state)?
        } else {
            self.replay(bytes)?
        };
        state.reply = reply;
        state.at = 0;
        Ok(bytes.len() as u64)
    }
}
