//! E18 input reader and virtual injector (`docs/gui/input.md`).
//! Reads and writes are complete batches. Owning Reader keeps the compositor
//! grab alive; closing its last duplicated descriptor releases the grab.
use crate::{
    errno::Errno,
    error::Error,
    fs, io,
    syscall::{nr, raw},
    types::{Fd, OwnedFd},
};
pub use input_abi::{
    self as abi, axis, disposition, key, kind, modifier, reason, syn, Capability, Disposition,
    Event, SceneTag, DeviceInfo,
};

fn invalid() -> Error {
    Errno::EINVAL.into()
}
fn control(fd: Fd, op: u64, arg: u64) -> Result<(), Error> {
    Error::from_syscall(unsafe { raw::syscall3(nr::IOCTL, fd.raw(), op, arg) } as i64).map(|_| ())
}
/// An independently queued input stream; its fd also works with poll/select/epoll.
pub struct Reader(OwnedFd);
impl Reader {
    pub fn open(nonblocking: bool) -> Result<Self, Error> {
        fs::open(
            "/dev/input/events",
            fs::O_RDONLY
                | io::status_flags::O_CLOEXEC as u32
                | if nonblocking { fs::O_NONBLOCK } else { 0 },
        )
        .map(OwnedFd::new)
        .map(Self)
    }
    pub fn fd(&self) -> Fd {
        self.0.fd()
    }
    pub fn grab(&self, devices: &[u32]) -> Result<(), Error> {
        let selection = abi::Selection {
            devices: devices.as_ptr() as u64,
            count: devices.len().try_into().map_err(|_| invalid())?,
            reserved: 0,
        };
        control(self.fd(), abi::ioctl::GRAB, &selection as *const _ as u64)
    }
    /// Snapshot a physical or virtual device's declared capabilities (W29).
    pub fn device_info(&self, device: u32, capabilities: &mut [Capability]) -> Result<DeviceInfo, Error> {
        let mut query = DeviceInfo {
            device,
            capabilities: capabilities.as_mut_ptr() as u64,
            capacity: capabilities.len().try_into().map_err(|_| invalid())?,
            ..DeviceInfo::default()
        };
        control(self.fd(), abi::ioctl::INPUT_DEVICE_INFO, &mut query as *mut _ as u64)?;
        Ok(query)
    }
    pub fn ungrab(&self) -> Result<(), Error> {
        control(self.fd(), abi::ioctl::UNGRAB, 0)
    }
    pub fn dispose(&self, outcome: &Disposition) -> Result<(), Error> {
        control(
            self.fd(),
            abi::ioctl::DISPOSITION,
            outcome as *const _ as u64,
        )
    }
    /// A short buffer returns EINVAL without consuming the next batch.
    pub fn read<'a>(&self, buffer: &'a mut [u8]) -> Result<Records<'a>, Error> {
        let n = io::read(self.fd(), buffer)?;
        Ok(Records {
            bytes: &buffer[..n],
        })
    }
}
/// Decoded event and its borrowed TEXT or SYN_SCENE payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Record<'a> {
    pub event: Event,
    pub payload: &'a [u8],
}
/// Iterator over whole wire records, rejecting a malformed record rather than
/// silently skipping it. A SYN_REPORT terminates each published batch.
pub struct Records<'a> {
    bytes: &'a [u8],
}
impl<'a> Iterator for Records<'a> {
    type Item = Result<Record<'a>, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        if self.bytes.len() < abi::EVENT_SIZE {
            self.bytes = &[];
            return Some(Err(invalid()));
        }
        let b = self.bytes;
        let event = Event {
            serial: u64::from_le_bytes(b[..8].try_into().unwrap()),
            time_ns: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            device: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            kind: u16::from_le_bytes(b[20..22].try_into().unwrap()),
            code: u16::from_le_bytes(b[22..24].try_into().unwrap()),
            value: i32::from_le_bytes(b[24..28].try_into().unwrap()),
            flags: u32::from_le_bytes(b[28..32].try_into().unwrap()),
        };
        let len = match (event.kind, event.code) {
            (kind::TEXT, 0) if (1..=256).contains(&event.value) => event.value as usize,
            (kind::SYN, syn::SCENE) if event.value == 32 => 32,
            (kind::TEXT, _) | (kind::SYN, syn::SCENE) => {
                self.bytes = &[];
                return Some(Err(invalid()));
            }
            _ => 0,
        };
        if b.len() < 32 + len {
            self.bytes = &[];
            return Some(Err(invalid()));
        }
        self.bytes = &b[32 + len..];
        let payload = &b[32..32 + len];
        if event.kind == kind::TEXT && core::str::from_utf8(payload).is_err() {
            return Some(Err(invalid()));
        }
        Some(Ok(Record { event, payload }))
    }
}
/// Builds one atomic injection batch in caller-provided storage. No allocator
/// is needed, including in programs using libbreenix without Rust std.
pub struct Batch<'a> {
    bytes: &'a mut [u8],
    used: usize,
}
impl<'a> Batch<'a> {
    pub fn new(storage: &'a mut [u8]) -> Self {
        Self {
            bytes: storage,
            used: 0,
        }
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self
            .used
            .checked_add(bytes.len())
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(invalid)?;
        self.bytes[self.used..end].copy_from_slice(bytes);
        self.used = end;
        Ok(())
    }
    pub fn event(
        &mut self,
        kind: u16,
        code: u16,
        value: i32,
        time_ns: u64,
    ) -> Result<&mut Self, Error> {
        let mut header = [0; 32];
        header[8..16].copy_from_slice(&time_ns.to_le_bytes());
        header[20..22].copy_from_slice(&kind.to_le_bytes());
        header[22..24].copy_from_slice(&code.to_le_bytes());
        header[24..28].copy_from_slice(&value.to_le_bytes());
        self.append(&header)?;
        Ok(self)
    }
    pub fn text(&mut self, text: &str, time_ns: u64) -> Result<&mut Self, Error> {
        if text.is_empty()
            || text.len() > abi::TEXT_LIMIT
            || self.bytes.len() - self.used < 32 + text.len()
        {
            return Err(invalid());
        }
        self.event(kind::TEXT, 0, text.len() as i32, time_ns)?;
        self.append(text.as_bytes())?;
        Ok(self)
    }
    pub fn scene(&mut self, tag: SceneTag, time_ns: u64) -> Result<&mut Self, Error> {
        if self.bytes.len() - self.used < 64 {
            return Err(invalid());
        }
        self.event(kind::SYN, syn::SCENE, 32, time_ns)?;
        for v in [
            tag.scene_version,
            tag.window,
            tag.layout_version,
            tag.gesture,
        ] {
            self.append(&v.to_le_bytes())?;
        }
        Ok(self)
    }
    pub fn finish(&mut self, time_ns: u64) -> Result<&mut Self, Error> {
        self.event(kind::SYN, syn::REPORT, 0, time_ns)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.used]
    }
    pub fn clear(&mut self) {
        self.used = 0;
    }
}
/// A virtual device, removed when the last descriptor reference closes.
pub struct Injector {
    fd: OwnedFd,
    id: u32,
}
impl Injector {
    pub fn create(capabilities: &[Capability], injector_time: bool) -> Result<Self, Error> {
        let fd = OwnedFd::new(fs::open(
            "/dev/uinput",
            fs::O_RDWR | io::status_flags::O_CLOEXEC as u32,
        )?);
        if capabilities.len() > 512 {
            return Err(invalid());
        }
        let mut bytes = [0u8; 16 + 512 * 12];
        bytes[..8].copy_from_slice(&abi::CREATE_MAGIC);
        bytes[8..12]
            .copy_from_slice(&(if injector_time { abi::TIME_INJECTOR } else { 0 }).to_le_bytes());
        bytes[12..16].copy_from_slice(&(capabilities.len() as u32).to_le_bytes());
        for (c, out) in capabilities.iter().zip(bytes[16..].chunks_exact_mut(12)) {
            out[..2].copy_from_slice(&c.kind.to_le_bytes());
            out[2..4].copy_from_slice(&c.code.to_le_bytes());
            out[4..8].copy_from_slice(&c.minimum.to_le_bytes());
            out[8..12].copy_from_slice(&c.maximum.to_le_bytes());
        }
        let length = 16 + 12 * capabilities.len();
        if io::write(fd.fd(), &bytes[..length])? != length {
            return Err(invalid());
        }
        let mut id = [0; 4];
        if io::read(fd.fd(), &mut id)? != 4 {
            return Err(invalid());
        }
        Ok(Self {
            fd,
            id: u32::from_le_bytes(id),
        })
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn fd(&self) -> Fd {
        self.fd.fd()
    }
    pub fn inject(&self, batch: &Batch<'_>) -> Result<(), Error> {
        if batch.bytes().is_empty() {
            return Err(invalid());
        }
        if io::write(self.fd(), batch.bytes())? == batch.bytes().len() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
