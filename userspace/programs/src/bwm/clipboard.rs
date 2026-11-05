//! BCLIP001 framing and clipboard state; transport and peer authorization are separate.

pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const HEADER_LEN: usize = 24;
const EINVAL: i32 = 22;
const E2BIG: i32 = 7;
const EOVERFLOW: i32 = 75;

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub window: u64,
    pub op: u32,
    pub payload: Vec<u8>,
}

pub struct Decoder {
    header: [u8; HEADER_LEN],
    header_len: usize,
    request: Option<Request>,
    length: usize,
    finished: bool,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            header: [0; HEADER_LEN],
            header_len: 0,
            request: None,
            length: 0,
            finished: false,
        }
    }

    /// Consume at most one request, returning the number of input bytes consumed.
    /// A completed or rejected decoder must be replaced before the next request.
    pub fn feed(&mut self, input: &[u8]) -> (usize, Option<Result<Request, i32>>) {
        assert!(!self.finished);
        let mut used = 0;
        if self.header_len < HEADER_LEN {
            let n = (HEADER_LEN - self.header_len).min(input.len());
            self.header[self.header_len..self.header_len + n].copy_from_slice(&input[..n]);
            self.header_len += n;
            used += n;
            if self.header_len < HEADER_LEN {
                return (used, None);
            }
            let op = u32::from_le_bytes(self.header[8..12].try_into().unwrap());
            self.length = u32::from_le_bytes(self.header[12..16].try_into().unwrap()) as usize;
            let window = u64::from_le_bytes(self.header[16..24].try_into().unwrap());
            let error = if &self.header[..8] != b"BCLIP001"
                || !(op == 1 || op == 2)
                || (op == 2 && self.length != 0)
            {
                Some(-EINVAL)
            } else if self.length > MAX_PAYLOAD {
                Some(-E2BIG)
            } else {
                None
            };
            if let Some(error) = error {
                self.finished = true;
                return (used, Some(Err(error)));
            }
            self.request = Some(Request {
                window,
                op,
                payload: Vec::new(),
            });
        }
        let request = self.request.as_mut().unwrap();
        let n = (self.length - request.payload.len()).min(input.len() - used);
        request.payload.extend_from_slice(&input[used..used + n]);
        used += n;
        if request.payload.len() == self.length {
            self.finished = true;
            (used, Some(Ok(self.request.take().unwrap())))
        } else {
            (used, None)
        }
    }
}

pub struct Reply {
    pub status: i32,
    pub serial: u64,
    pub payload: Vec<u8>,
}

impl Reply {
    pub fn error(status: i32, serial: u64) -> Self {
        Self {
            status,
            serial,
            payload: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + self.payload.len());
        bytes.extend_from_slice(&self.status.to_le_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.serial.to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

pub struct Store {
    serial: u64,
    payload: Vec<u8>,
}
impl Store {
    pub fn new() -> Self {
        Self {
            serial: 1,
            payload: Vec::new(),
        }
    }
    pub fn serial(&self) -> u64 {
        self.serial
    }
    pub fn execute(&mut self, request: Request) -> Reply {
        match request.op {
            1 if request.payload.len() <= MAX_PAYLOAD => {
                if let Some(serial) = self.serial().checked_add(1) {
                    self.serial = serial;
                    self.payload = request.payload;
                    Reply {
                        status: 0,
                        serial,
                        payload: Vec::new(),
                    }
                } else {
                    Reply::error(-EOVERFLOW, self.serial)
                }
            }
            1 => Reply::error(-E2BIG, self.serial),
            2 if request.payload.is_empty() => Reply {
                status: 0,
                serial: self.serial,
                payload: self.payload.clone(),
            },
            _ => Reply::error(-EINVAL, self.serial),
        }
    }
}
