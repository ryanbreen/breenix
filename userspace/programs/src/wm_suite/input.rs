//! Input ABI and test-control client, specified in docs/gui/window-manager-suite.md.
//! These use the production input stream, not WRITE_EVENTS or bwm's stdin.
use super::desktop::{now, pause};
use libbreenix::io;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub serial: u64,
    pub time: u64,
    pub device: u32,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
    pub flags: u32,
    pub payload: Vec<u8>,
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
            payload: Vec::new(),
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
    pub fn poll(&self, ms: i32) -> Result<bool, String> {
        let mut f = [io::PollFd {
            fd: self.0.as_raw_fd(),
            events: 1,
            revents: 0,
        }];
        io::poll(&mut f, ms).map_err(|e| e.to_string())?;
        Ok(f[0].revents & 1 != 0)
    }
    pub fn grab(&self, devices: &[u32]) -> Result<(), String> {
        let a = input_abi::Selection {
            devices: devices.as_ptr() as u64,
            count: devices.len() as u32,
            reserved: 0,
        };
        let r = unsafe {
            libbreenix::syscall::raw::syscall3(
                libbreenix::syscall::nr::IOCTL,
                self.0.as_raw_fd() as u64,
                input_abi::ioctl::GRAB as u64,
                &a as *const _ as u64,
            )
        } as i64;
        if r < 0 {
            Err(format!("input GRAB errno {}", -r))
        } else {
            Ok(())
        }
    }
    pub fn dispose(
        &self,
        serial: u64,
        window: u64,
        disposition: u32,
        reason: u32,
    ) -> Result<(), String> {
        let a = input_abi::Disposition {
            serial,
            window,
            disposition,
            reason,
            derived: 0,
            reserved: 0,
        };
        let r = unsafe {
            libbreenix::syscall::raw::syscall3(
                libbreenix::syscall::nr::IOCTL,
                self.0.as_raw_fd() as u64,
                input_abi::ioctl::DISPOSITION,
                &a as *const _ as u64,
            )
        } as i64;
        if r < 0 {
            Err(format!("input DISPOSITION errno {}", -r))
        } else {
            Ok(())
        }
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
            let mut bytes = [0; 32768];
            let count = self.0.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("input stream returned EOF or a partial 32-byte event".into());
            }
            let mut at = 0;
            while at < count {
                if count - at < 32 {
                    return Err("partial input header".into());
                }
                let mut e = Event::decode(&bytes[at..at + 32]);
                at += 32;
                let length = if e.kind == 6 {
                    usize::try_from(e.value).map_err(|_| "negative text length")?
                } else if e.kind == 0 && e.code == 4 {
                    32
                } else {
                    0
                };
                if length > count - at {
                    return Err("partial input payload".into());
                }
                e.payload.extend_from_slice(&bytes[at..at + length]);
                at += length;
                if devices.is_empty() || devices.contains(&e.device) {
                    result.push(e);
                }
            }
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
        Self::create_timed(1)
    }
    pub fn create_timed(timing: u32) -> Result<Self, String> {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| format!("missing input-core virtual devices /dev/uinput: {e}"))?;
        // A clean-room Breenix creation record, followed by the declared capabilities.
        let mut caps: Vec<(u16, u16, i32, i32)> = (1..256).map(|code| (1, code, 0, 1)).collect();
        caps.extend([
            (1, 0x110, 0, 1),
            (1, 0x111, 0, 1),
            (1, 0x112, 0, 1),
            (3, 0, 0, 32767),
            (3, 1, 0, 32767),
            (2, 0, -32767, 32767),
            (2, 1, -32767, 32767),
            (2, 6, -32767, 32767),
            (2, 8, -32767, 32767),
            (6, 0, 1, 256),
        ]);
        let mut header = b"BINPUT01".to_vec();
        header.extend_from_slice(&timing.to_le_bytes());
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
    pub fn raw(&mut self, bytes: &[u8]) -> Result<usize, std::io::Error> {
        self.file.write(bytes)
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

impl Virtual {
    pub fn tagged(
        &mut self,
        d: &super::desktop::Desktop,
        window: u64,
        scene: u64,
        layout: u64,
        gesture: u64,
        x: i32,
        y: i32,
        pressed: i32,
        stamp: u64,
    ) -> Result<(), String> {
        let mut bytes = Vec::new();
        let mut header = [0u8; 32];
        header[8..16].copy_from_slice(&stamp.to_le_bytes());
        header[22..24].copy_from_slice(&4u16.to_le_bytes());
        header[24..28].copy_from_slice(&32i32.to_le_bytes());
        bytes.extend_from_slice(&header);
        for v in [scene, window, layout, gesture] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let ax = ((x as i64 * 32767 + d.width as i64 - 2) / (d.width as i64 - 1)) as i32;
        let ay = ((y as i64 * 32767 + d.height as i64 - 2) / (d.height as i64 - 1)) as i32;
        for (kind, code, value) in [(3u16, 0u16, ax), (3, 1, ay), (1, 0x110, pressed), (0, 0, 0)] {
            let mut h = [0u8; 32];
            h[8..16].copy_from_slice(&stamp.to_le_bytes());
            h[20..22].copy_from_slice(&kind.to_le_bytes());
            h[22..24].copy_from_slice(&code.to_le_bytes());
            h[24..28].copy_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&h);
        }
        let n = self
            .raw(&bytes)
            .map_err(|e| format!("tagged pointer batch: {e}"))?;
        if n != bytes.len() {
            return Err("tagged pointer batch accepted partially".into());
        }
        Ok(())
    }
}
