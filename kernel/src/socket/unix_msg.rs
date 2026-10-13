//! Owned, bounded Unix messages. Rights remain counted open references while
//! queued and use the normal close hooks if the receiver discards them.

use super::addr::UnixName;
use super::object::UCred;
use crate::ipc::fd::FileDescriptor;
use crate::syscall::errno::{EAGAIN, EMSGSIZE, ENOMEM};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const MAX_RIGHTS: usize = 253; // Linux SCM_MAX_FD.
pub const MAX_DATA: usize = 4 * 1024 * 1024;
const MAX_MESSAGES: usize = 1024;

pub struct UnixMsg {
    pub data: Vec<u8>,
    pub rights: Vec<FileDescriptor>,
    pub cred: UCred,
    pub from: UnixName,
    offset: usize,
}

impl UnixMsg {
    pub fn new(
        data: &[u8],
        rights: Vec<FileDescriptor>,
        cred: UCred,
        from: UnixName,
    ) -> Result<Self, i32> {
        let mut msg = Self {
            data: Vec::new(),
            rights,
            cred,
            from,
            offset: 0,
        };
        if msg.rights.len() > MAX_RIGHTS || data.len() > MAX_DATA {
            return Err(EMSGSIZE);
        }
        msg.data.try_reserve_exact(data.len()).map_err(|_| ENOMEM)?;
        msg.data.extend_from_slice(data);
        Ok(msg)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data[self.offset..]
    }
    pub fn consume(&mut self, bytes: usize) {
        self.offset += bytes.min(self.bytes().len());
    }
    fn charge(&self) -> usize {
        self.bytes().len().max(1) + self.rights.len() * core::mem::size_of::<FileDescriptor>()
    }
}

impl Drop for UnixMsg {
    fn drop(&mut self) {
        let rights = core::mem::take(&mut self.rights);
        if !rights.is_empty() {
            // Bounded by SCM_MAX_FD, including error/close paths.
            crate::task::process_task::close_extracted_fds(
                rights.into_iter().enumerate().collect(),
            );
        }
    }
}

pub struct UnixQueue {
    messages: VecDeque<UnixMsg>,
    bytes: usize,
}

impl UnixQueue {
    pub const fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            bytes: 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
    pub fn room(&self, limit: usize) -> usize {
        if self.messages.len() >= MAX_MESSAGES {
            0
        } else {
            limit.saturating_sub(self.bytes)
        }
    }
    pub fn charged_bytes(&self) -> usize {
        self.bytes
    }
    pub fn stream_room(&self, limit: usize) -> usize {
        if self.messages.back().is_some_and(|m| m.rights.is_empty()) {
            limit.saturating_sub(self.bytes)
        } else {
            self.room(limit)
        }
    }
    /// Adjacent plain stream writes share storage; record sockets always use
    /// push instead. Ancillary data and credential boundaries are retained.
    pub fn push_stream(&mut self, msg: UnixMsg, limit: usize) -> Result<(), (i32, UnixMsg)> {
        if msg.rights.is_empty() {
            if let Some(last) = self.messages.back_mut() {
                if last.rights.is_empty()
                    && last.from == msg.from
                    && last.cred.pid == msg.cred.pid
                    && last.cred.uid == msg.cred.uid
                    && last.cred.gid == msg.cred.gid
                {
                    let before = last.charge();
                    let additional = msg.bytes().len();
                    if additional > limit.saturating_sub(self.bytes) {
                        return Err((EAGAIN, msg));
                    }
                    // Consumed prefixes cannot make physical storage grow
                    // beyond the byte budget as readers and writers alternate.
                    if last.offset != 0 {
                        let remaining = last.bytes().len();
                        last.data.copy_within(last.offset.., 0);
                        last.data.truncate(remaining);
                        last.offset = 0;
                    }
                    if last.data.try_reserve(additional).is_err() {
                        return Err((ENOMEM, msg));
                    }
                    last.data.extend_from_slice(msg.bytes());
                    self.bytes += last.charge() - before;
                    return Ok(());
                }
            }
        }
        self.push(msg, limit)
    }
    pub fn push(&mut self, msg: UnixMsg, limit: usize) -> Result<(), (i32, UnixMsg)> {
        let charge = msg.charge();
        if charge > limit {
            return Err((EMSGSIZE, msg));
        }
        if charge > self.room(limit) {
            return Err((EAGAIN, msg));
        }
        if self.messages.try_reserve(1).is_err() {
            return Err((ENOMEM, msg));
        }
        self.bytes += charge;
        self.messages.push_back(msg);
        Ok(())
    }
    pub fn front(&self) -> Option<&UnixMsg> {
        self.messages.front()
    }
    pub fn iter(&self) -> impl Iterator<Item = &UnixMsg> {
        self.messages.iter()
    }
    pub fn front_mut(&mut self) -> Option<&mut UnixMsg> {
        self.messages.front_mut()
    }
    pub fn pop(&mut self) -> Option<UnixMsg> {
        let msg = self.messages.pop_front()?;
        self.bytes -= msg.charge();
        Some(msg)
    }
    pub fn consume(&mut self, n: usize) {
        if let Some(msg) = self.messages.front_mut() {
            let before = msg.charge();
            msg.consume(n);
            self.bytes -= before - msg.charge();
        }
    }
    pub fn take_rights(&mut self) -> Vec<FileDescriptor> {
        let Some(msg) = self.messages.front_mut() else {
            return Vec::new();
        };
        let before = msg.charge();
        let rights = core::mem::take(&mut msg.rights);
        self.bytes -= before - msg.charge();
        rights
    }
}
