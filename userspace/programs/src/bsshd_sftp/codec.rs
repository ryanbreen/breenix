//! SFTP version 3 framing and fields (draft-ietf-secsh-filexfer-02).
use std::io::{self, Read, Write};

const VERSION: u32 = 3;
const MAX_BODY: usize = 256 * 1024;
pub(super) const MAX_READ: usize = MAX_BODY - 9;

const SIZE: u32 = 1;
const UID_GID: u32 = 2;
const MODE: u32 = 4;
const TIMES: u32 = 8;
const EXTENDED_ATTRS: u32 = 0x8000_0000;

pub(super) const READ: u32 = 1;
pub(super) const WRITE: u32 = 2;
pub(super) const APPEND: u32 = 4;
pub(super) const CREATE: u32 = 8;
pub(super) const TRUNCATE: u32 = 16;
pub(super) const EXCLUSIVE: u32 = 32;

#[derive(Default)]
pub(super) struct Attrs {
    pub size: Option<u64>,
    pub owner: Option<(u32, u32)>,
    pub mode: Option<u32>,
    pub times: Option<(u32, u32)>,
}

pub(super) enum Request<'a> {
    Open(&'a [u8], u32, Attrs),
    Close(&'a [u8]),
    Read(&'a [u8], u64, u32),
    Write(&'a [u8], u64, &'a [u8]),
    Stat(&'a [u8], bool),
    Fstat(&'a [u8]),
    Setstat(&'a [u8], Attrs),
    Fsetstat(&'a [u8], Attrs),
    Opendir(&'a [u8]),
    Readdir(&'a [u8]),
    Remove(&'a [u8]),
    Mkdir(&'a [u8], Attrs),
    Rmdir(&'a [u8]),
    Realpath(&'a [u8]),
    Rename(&'a [u8], &'a [u8]),
    Readlink(&'a [u8]),
    Symlink(&'a [u8], &'a [u8]),
    Extended,
}

#[derive(Clone, Copy)]
pub(super) enum Status {
    Ok = 0,
    Eof = 1,
    NoSuchFile = 2,
    PermissionDenied = 3,
    Failure = 4,
    BadMessage = 5,
    Unsupported = 8,
}

pub(super) struct Name {
    pub short: Vec<u8>,
    pub long: Vec<u8>,
    pub attrs: Attrs,
}

pub(super) enum Reply {
    Status(Status),
    Handle(u64),
    Data(Vec<u8>),
    Name(Vec<Name>),
    Attrs(Attrs),
}

pub(super) fn read_frame(stream: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match stream.read_exact(&mut length[..1]) {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        result => result?,
    }
    stream.read_exact(&mut length[1..])?;
    let length = u32::from_be_bytes(length) as usize;
    if !(1..=MAX_BODY).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SFTP packet length",
        ));
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_frame(stream: &mut impl Write, mut frame: Vec<u8>) -> io::Result<()> {
    let length = frame.len() - 4;
    if length > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SFTP reply too large",
        ));
    }
    frame[..4].copy_from_slice(&(length as u32).to_be_bytes());
    stream.write_all(&frame)?;
    stream.flush()
}

pub(super) fn negotiate(stream: &mut (impl Read + Write), frame: &[u8]) -> io::Result<()> {
    let offer = || {
        let (&1, mut fields) = frame.split_first()? else {
            return None;
        };
        let version = fields.u32()?;
        while !fields.is_empty() {
            fields.string()?;
            fields.string()?;
        }
        (version >= VERSION).then_some(())
    };
    offer().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "expected SFTP v3 INIT"))?;
    let mut version = vec![0, 0, 0, 0, 2];
    put_u32(&mut version, VERSION);
    write_frame(stream, version)
}

pub(super) fn request_header(frame: &[u8]) -> io::Result<(u8, u32, &[u8])> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "missing SFTP request header");
    let (&kind, mut fields) = frame.split_first().ok_or_else(invalid)?;
    if kind == 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "duplicate SFTP INIT",
        ));
    }
    let id = fields.u32().ok_or_else(invalid)?;
    Ok((kind, id, fields))
}

pub(super) fn decode(kind: u8, mut fields: &[u8]) -> Option<Request<'_>> {
    let request = match kind {
        3 => {
            let path = fields.path()?;
            let flags = fields.u32()?;
            let writing = flags & WRITE != 0;
            let creating = flags & CREATE != 0;
            if flags & !0x3f != 0
                || flags & (READ | WRITE) == 0
                || (!writing && flags & (APPEND | CREATE | TRUNCATE) != 0)
                || (!creating && flags & (TRUNCATE | EXCLUSIVE) != 0)
            {
                return None;
            }
            Request::Open(path, flags, fields.attrs()?)
        }
        4 => Request::Close(fields.handle()?),
        5 => Request::Read(fields.handle()?, fields.u64()?, fields.u32()?),
        6 => Request::Write(fields.handle()?, fields.u64()?, fields.string()?),
        7 | 17 => Request::Stat(fields.path()?, kind == 17),
        8 => Request::Fstat(fields.handle()?),
        9 => Request::Setstat(fields.path()?, fields.attrs()?),
        10 => Request::Fsetstat(fields.handle()?, fields.attrs()?),
        11 => Request::Opendir(fields.path()?),
        12 => Request::Readdir(fields.handle()?),
        13 => Request::Remove(fields.path()?),
        14 => Request::Mkdir(fields.path()?, fields.attrs()?),
        15 => Request::Rmdir(fields.path()?),
        16 => Request::Realpath(fields.path()?),
        18 => Request::Rename(fields.path()?, fields.path()?),
        19 => Request::Readlink(fields.path()?),
        20 => Request::Symlink(fields.path()?, fields.path()?),
        200 => {
            fields.string()?;
            return Some(Request::Extended); // Remaining fields depend on the extension.
        }
        _ => return None,
    };
    fields.is_empty().then_some(request)
}

trait Fields<'a> {
    fn u32(&mut self) -> Option<u32>;
    fn u64(&mut self) -> Option<u64>;
    fn string(&mut self) -> Option<&'a [u8]>;
    fn path(&mut self) -> Option<&'a [u8]>;
    fn handle(&mut self) -> Option<&'a [u8]>;
    fn attrs(&mut self) -> Option<Attrs>;
}

impl<'a> Fields<'a> for &'a [u8] {
    fn u32(&mut self) -> Option<u32> {
        let (bytes, tail) = self.split_first_chunk()?;
        *self = tail;
        Some(u32::from_be_bytes(*bytes))
    }

    fn u64(&mut self) -> Option<u64> {
        let (bytes, tail) = self.split_first_chunk()?;
        *self = tail;
        Some(u64::from_be_bytes(*bytes))
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let length = self.u32()? as usize;
        let (value, tail) = self.split_at_checked(length)?;
        *self = tail;
        Some(value)
    }

    fn path(&mut self) -> Option<&'a [u8]> {
        let path = self.string()?;
        // POSIX paths are NUL-terminated. Reject a byte string that would
        // designate a different path at the kernel boundary.
        (!path.contains(&0)).then_some(path)
    }

    fn handle(&mut self) -> Option<&'a [u8]> {
        let handle = self.string()?;
        (handle.len() <= 256).then_some(handle)
    }

    fn attrs(&mut self) -> Option<Attrs> {
        let flags = self.u32()?;
        if flags & !(SIZE | UID_GID | MODE | TIMES | EXTENDED_ATTRS) != 0 {
            return None;
        }
        let mut attrs = Attrs::default();
        if flags & SIZE != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & UID_GID != 0 {
            attrs.owner = Some((self.u32()?, self.u32()?));
        }
        if flags & MODE != 0 {
            attrs.mode = Some(self.u32()?);
        }
        if flags & TIMES != 0 {
            attrs.times = Some((self.u32()?, self.u32()?));
        }
        if flags & EXTENDED_ATTRS != 0 {
            let count = self.u32()?;
            for _ in 0..count {
                self.string()?;
                self.string()?;
            }
        }
        Some(attrs)
    }
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn put_string(bytes: &mut Vec<u8>, value: &[u8]) {
    put_u32(bytes, value.len() as u32);
    bytes.extend_from_slice(value);
}

impl Attrs {
    fn encode(&self, bytes: &mut Vec<u8>) {
        let flags = (if self.size.is_some() { SIZE } else { 0 })
            | (if self.owner.is_some() { UID_GID } else { 0 })
            | (if self.mode.is_some() { MODE } else { 0 })
            | (if self.times.is_some() { TIMES } else { 0 });
        put_u32(bytes, flags);
        if let Some(size) = self.size {
            bytes.extend_from_slice(&size.to_be_bytes());
        }
        if let Some((uid, gid)) = self.owner {
            put_u32(bytes, uid);
            put_u32(bytes, gid);
        }
        if let Some(mode) = self.mode {
            put_u32(bytes, mode);
        }
        if let Some((accessed, modified)) = self.times {
            put_u32(bytes, accessed);
            put_u32(bytes, modified);
        }
    }
}

impl Reply {
    pub(super) fn send(&self, stream: &mut impl Write, id: u32) -> io::Result<()> {
        let kind = match self {
            Self::Status(_) => 101,
            Self::Handle(_) => 102,
            Self::Data(_) => 103,
            Self::Name(_) => 104,
            Self::Attrs(_) => 105,
        };
        let mut body = vec![0, 0, 0, 0, kind];
        put_u32(&mut body, id);
        match self {
            Self::Status(status) => {
                let message: &[u8] = match status {
                    Status::Ok => b"",
                    Status::Eof => b"End of file",
                    Status::NoSuchFile => b"No such file",
                    Status::PermissionDenied => b"Permission denied",
                    Status::Failure => b"Failure",
                    Status::BadMessage => b"Malformed SFTP request",
                    Status::Unsupported => b"Operation unsupported",
                };
                put_u32(&mut body, *status as u32);
                put_string(&mut body, message);
                put_string(&mut body, b"");
            }
            Self::Handle(handle) => put_string(&mut body, &handle.to_be_bytes()),
            Self::Data(data) => put_string(&mut body, data),
            Self::Name(names) => {
                put_u32(&mut body, names.len() as u32);
                for name in names {
                    put_string(&mut body, &name.short);
                    put_string(&mut body, &name.long);
                    name.attrs.encode(&mut body);
                }
            }
            Self::Attrs(attrs) => attrs.encode(&mut body),
        }
        write_frame(stream, body)
    }
}
