//! Input ABI and test-control client, specified in docs/gui/window-manager-suite.md.
//! These use the production input stream, not WRITE_EVENTS or bwm's stdin.
use super::desktop::{now, pause};
use libbreenix::io;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub serial: u64,
    pub time: u64,
    pub device: u32,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
    pub flags: u32,
}
impl Event {
    fn decode(b: &[u8]) -> Self {
        Self {
            serial: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            time: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            device: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            kind: u16::from_le_bytes(b[20..22].try_into().unwrap()),
            code: u16::from_le_bytes(b[22..24].try_into().unwrap()),
            value: i32::from_le_bytes(b[24..28].try_into().unwrap()),
            flags: u32::from_le_bytes(b[28..32].try_into().unwrap()),
        }
    }
}
pub struct Stream(File);
impl Stream {
    pub fn open() -> Result<Self, String> {
        File::open("/dev/input/events")
            .map(Self)
            .map_err(|e| format!("missing input-core event stream /dev/input/events: {e}"))
    }
    pub fn read(&mut self, n: usize) -> Result<Vec<Event>, String> {
        self.read_from(n, &[])
    }
    pub fn read_from(&mut self, n: usize, devices: &[u32]) -> Result<Vec<Event>, String> {
        let mut result = Vec::new();
        let end = now() + 1_000_000_000;
        while result.len() < n && now() < end {
            let mut p = [io::PollFd {
                fd: self.0.as_raw_fd(),
                events: 1,
                revents: 0,
            }];
            io::poll(&mut p, 10).map_err(|e| e.to_string())?;
            if p[0].revents == 0 {
                continue;
            }
            let mut bytes = [0; 32 * 64];
            let count = self.0.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 || count % 32 != 0 {
                return Err("input stream returned EOF or a partial 32-byte event".into());
            }
            result.extend(
                bytes[..count]
                    .chunks_exact(32)
                    .map(Event::decode)
                    .filter(|e| devices.is_empty() || devices.contains(&e.device)),
            );
        }
        if result.len() < n {
            return Err(format!(
                "input stream delivered {} events, expected at least {n}",
                result.len()
            ));
        }
        Ok(result)
    }
}
pub struct Virtual {
    file: File,
    pub id: u32,
}
impl Virtual {
    pub fn create() -> Result<Self, String> {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| format!("missing input-core virtual devices /dev/uinput: {e}"))?;
        // A clean-room Breenix creation record, followed by the declared capabilities.
        let caps: [(u16, u16, i32, i32); 13] = [
            (1, 30, 0, 1),
            (1, 48, 0, 1),
            (1, 42, 0, 1),
            (1, 29, 0, 1),
            (1, 56, 0, 1),
            (1, 125, 0, 1),
            (1, 0x110, 0, 1),
            (1, 0x111, 0, 1),
            (1, 0x112, 0, 1),
            (3, 0, 0, 32767),
            (3, 1, 0, 32767),
            (2, 8, -1, 1),
            (6, 0, 1, 256),
        ];
        let mut header = b"BINPUT01".to_vec();
        header.extend_from_slice(&1u32.to_le_bytes()); // time=injector
        header.extend_from_slice(&(caps.len() as u32).to_le_bytes());
        for (kind, code, min, max) in caps {
            header.extend_from_slice(&kind.to_le_bytes());
            header.extend_from_slice(&code.to_le_bytes());
            header.extend_from_slice(&min.to_le_bytes());
            header.extend_from_slice(&max.to_le_bytes());
        }
        let written = f
            .write(&header)
            .map_err(|e| format!("uinput CREATE: {e}"))?;
        if written != header.len() {
            return Err("uinput creation was not atomic".into());
        }
        let mut id = [0; 4];
        f.read_exact(&mut id)
            .map_err(|e| format!("uinput device identity: {e}"))?;
        let id = u32::from_le_bytes(id);
        if id == 0 {
            return Err("uinput created device id 0".into());
        }
        Ok(Self { file: f, id })
    }
    pub fn batch(&mut self, events: &[(u16, u16, i32)], stamp: u64) -> Result<(), String> {
        let mut bytes = Vec::new();
        for &(kind, code, value) in events.iter().chain(std::iter::once(&(0, 0, 0))) {
            let mut b = [0; 32];
            b[8..16].copy_from_slice(&stamp.to_le_bytes());
            b[20..22].copy_from_slice(&kind.to_le_bytes());
            b[22..24].copy_from_slice(&code.to_le_bytes());
            b[24..28].copy_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&b);
        }
        let n = self
            .file
            .write(&bytes)
            .map_err(|e| format!("uinput batch errno {}: {e}", e.raw_os_error().unwrap_or(0)))?;
        if n != bytes.len() {
            return Err("uinput accepted a partial batch".into());
        }
        Ok(())
    }
    pub fn text(&mut self, text: &[u8], stamp: u64) -> Result<(), String> {
        let mut bytes = [0; 32].to_vec();
        bytes[8..16].copy_from_slice(&stamp.to_le_bytes());
        bytes[20..22].copy_from_slice(&6u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&(text.len() as i32).to_le_bytes());
        bytes.extend_from_slice(text);
        let mut syn = [0; 32];
        syn[8..16].copy_from_slice(&stamp.to_le_bytes());
        bytes.extend_from_slice(&syn);
        let n = self
            .file
            .write(&bytes)
            .map_err(|e| format!("EV_TEXT batch errno {}: {e}", e.raw_os_error().unwrap_or(0)))?;
        if n != bytes.len() {
            return Err("EV_TEXT batch was not atomic".into());
        }
        Ok(())
    }
}

/// Production-completion replay, deliberately distinct from host QMP smoke.
pub fn replay(which: u32, x: i32, y: i32) -> Result<(), String> {
    let mut request = [0u8; 64];
    request[..4].copy_from_slice(&which.to_le_bytes());
    // Keyboard: A down/up. Pointer: the requested absolute position, left down/up.
    request[8..16].copy_from_slice(&[0, 0, 4, 0, 0, 0, 0, 0]);
    request[16..20].copy_from_slice(&x.to_le_bytes());
    request[20..24].copy_from_slice(&y.to_le_bytes());
    let mut replay = OpenOptions::new().read(true).write(true).open("/proc/breenix/input-replay")
        .map_err(|e| format!("input-devices production completion replay /proc/breenix/input-replay is absent: {e}"))?;
    replay
        .write_all(&request)
        .map_err(|e| format!("completion replay request: {e}"))?;
    replay
        .read_exact(&mut request)
        .map_err(|e| format!("completion replay receipt: {e}"))?;
    let reports = u32::from_le_bytes(request[32..36].try_into().unwrap());
    let posted = u32::from_le_bytes(request[36..40].try_into().unwrap());
    let ring = u32::from_le_bytes(request[40..44].try_into().unwrap());
    let report_ok = if which & 0x100 != 0 {
        reports == 0
    } else {
        reports != 0
    };
    if !report_ok || ring == 0 || posted != ring {
        return Err(
            "completion replay did not report and rearm its bound device as requested".into(),
        );
    }
    pause();
    Ok(())
}
