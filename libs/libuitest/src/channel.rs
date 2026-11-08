//! Bounded nonblocking control I/O, shared by the case and its supervisor.
use libbreenix::{error::Error, errno::Errno, io, time, types::Fd};
use crate::supervisor::{encode, Decoder, Message};

pub(crate) fn now() -> Result<i128, String> {
    time::now_monotonic().map(|t| t.as_nanos() / 1_000_000).map_err(|e| e.to_string())
}
pub(crate) struct Channel {
    pub fd: Fd,
    decoder: Decoder,
    pending: Vec<u8>,
    sent: usize,
}
impl Channel {
    pub fn new(fd: Fd) -> Result<Self, String> {
        io::fcntl_setfd(fd, io::fd_flags::FD_CLOEXEC).map_err(|e| e.to_string())?;
        io::fcntl_setfl(fd, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        Ok(Self { fd, decoder: Decoder::default(), pending: Vec::new(), sent: 0 })
    }
    /// Gate bytes at enqueue, before a partial frame can enter the decoder.
    pub fn require_root_sender(&self) -> Result<(), String> {
        use libbreenix::{socket::SO_BREENIX_ROOT_ONLY, syscall::{nr, raw}};
        let on = 1i32;
        let result = unsafe { raw::syscall5(nr::SETSOCKOPT, self.fd.raw(), 1,
            SO_BREENIX_ROOT_ONLY as u64, &on as *const i32 as u64, 4) };
        Error::from_syscall(result as i64).map(|_| ()).map_err(|e|e.to_string())
    }
    pub fn queue(&mut self, message: &Message) -> Result<(), String> {
        let bytes = encode(message)?;
        if self.pending.len() - self.sent + bytes.len() > 256 * 1024 { return Err("control backpressure exceeds limit".into()); }
        if self.sent != 0 { self.pending.drain(..self.sent); self.sent = 0; }
        self.pending.extend_from_slice(&bytes); Ok(())
    }
    pub fn flush(&mut self) -> Result<bool, String> {
        while self.sent < self.pending.len() {
            match io::write(self.fd, &self.pending[self.sent..]) {
                Ok(0) => return Err("control socket closed".into()),
                Ok(n) => self.sent += n,
                Err(Error::Os(Errno::EINTR)) => continue,
                Err(Error::Os(Errno::EAGAIN)) => return Ok(false),
                Err(error) => return Err(error.to_string()),
            }
        }
        self.pending.clear(); self.sent = 0; Ok(true)
    }
    pub fn receive(&mut self) -> Result<Option<Message>, String> {
        if let Some(message) = self.decoder.next()? { return Ok(Some(message)); }
        let mut bytes = [0; 8192];
        match io::read(self.fd, &mut bytes) {
            Ok(0) | Err(Error::Os(Errno::ECONNRESET)) => Err("session supervisor/control peer closed".into()),
            Ok(n) => { self.decoder.push(&bytes[..n])?; self.decoder.next() }
            Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn send(&mut self, message: &Message) -> Result<(), String> {
        self.queue(message)?;
        let end = now()? + 1000;
        while !self.flush()? {
            if now()? >= end { return Err("control send deadline".into()); }
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl Drop for Channel { fn drop(&mut self) { let _ = io::close(self.fd); } }
