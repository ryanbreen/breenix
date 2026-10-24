//! UTF-8 clipboard access through a registered window (syscall 414, W4).

use crate::Window;
use libbreenix::window::abi::Plain;
use libbreenix::{
    errno::Errno,
    error::Error,
    window::{self, abi},
};

/// One complete clipboard generation, including embedded NUL characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardText {
    pub serial: u64,
    pub text: String,
}

impl Window {
    /// Replace the desktop's UTF-8 text and return its new change serial.
    pub fn clipboard_set(&self, text: &str) -> Result<u64, Error> {
        let mut a = abi::ClipboardSet::ZERO;
        a.id = self.id();
        a.ptr = text.as_ptr() as u64;
        a.len = text.len() as u64;
        a.mime = abi::CLIPBOARD_TEXT_UTF8;
        window::call(abi::op::CLIPBOARD_SET, &mut a)?;
        Ok(a.serial)
    }

    /// Read a whole generation. EAGAIN means another setter replaced it between
    /// the length query and copy; the caller can request the new generation.
    pub fn clipboard_get(&self) -> Result<ClipboardText, Error> {
        let mut a = abi::ClipboardGet::ZERO;
        a.id = self.id();
        a.mime = abi::CLIPBOARD_TEXT_UTF8;
        window::call(abi::op::CLIPBOARD_GET, &mut a)?;
        if a.len > abi::CLIPBOARD_MAX_BYTES {
            return Err(Error::Os(Errno::EIO));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(a.len as usize)
            .map_err(|_| Error::Os(Errno::ENOMEM))?;
        bytes.resize(a.len as usize, 0);
        a.dst = bytes.as_mut_ptr() as u64;
        a.capacity = bytes.len() as u64;
        // Repeat even for an empty generation: its serial can change too.
        window::call(abi::op::CLIPBOARD_GET, &mut a)?;
        let text = String::from_utf8(bytes).map_err(|_| Error::Os(Errno::EIO))?;
        Ok(ClipboardText {
            serial: a.serial,
            text,
        })
    }

    /// Enable or remove kernel ClipboardChanged notices on this window's event ring.
    pub fn clipboard_subscribe(&self, enabled: bool) -> Result<(), Error> {
        let mut a = abi::ClipboardSubscribe::ZERO;
        a.id = self.id();
        a.flags = u32::from(enabled);
        window::call(abi::op::CLIPBOARD_SUBSCRIBE, &mut a)?;
        Ok(())
    }
}
