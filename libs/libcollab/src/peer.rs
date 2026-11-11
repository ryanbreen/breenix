//! Transport-independent state for a collaboration peer.
use crate::wire::{DrawOp, Message};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Ack { global: u64 },
    Refused(Vec<u8>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub op: DrawOp,
    pub response: Option<Response>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Sequence,
    SyncOffset,
    UnknownSubmission,
    DuplicateResponse,
}

#[derive(Default)]
pub struct PeerCore {
    submissions: BTreeMap<u32, Submission>,
    next_local: u32,
    base: Vec<u8>,
    log: Vec<Message>,
    disconnect_reason: Option<Vec<u8>>,
}
impl PeerCore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn submit(&mut self, op: DrawOp) -> u32 {
        self.next_local = self
            .next_local
            .checked_add(1)
            .expect("local sequence exhausted");
        self.submissions
            .insert(self.next_local, Submission { op, response: None });
        self.next_local
    }
    pub fn submission(&self, local: u32) -> Option<&Submission> {
        self.submissions.get(&local)
    }
    pub fn response(&self, local: u32) -> Option<&Response> {
        self.submission(local)?.response.as_ref()
    }
    pub fn base_canvas(&self) -> &[u8] {
        &self.base
    }
    pub fn log(&self) -> &[Message] {
        &self.log
    }
    pub fn disconnect_reason(&self) -> Option<&[u8]> {
        self.disconnect_reason.as_deref()
    }
    pub fn receive(&mut self, message: Message) -> Result<(), Error> {
        match message {
            Message::SyncChunk { offset, data } => {
                if offset as usize != self.base.len() {
                    return Err(Error::SyncOffset);
                }
                self.base.extend_from_slice(&data);
            }
            Message::Ack { local, global } => self.answer(local, Response::Ack { global })?,
            Message::Refused { local, reason } => self.answer(local, Response::Refused(reason))?,
            Message::Apply { global, .. } => {
                if global != self.log.len() as u64 + 1 {
                    return Err(Error::Sequence);
                }
                self.log.push(message);
            }
            _ => {}
        }
        Ok(())
    }
    fn answer(&mut self, local: u32, response: Response) -> Result<(), Error> {
        let submission = self
            .submissions
            .get_mut(&local)
            .ok_or(Error::UnknownSubmission)?;
        if submission.response.is_some() {
            return Err(Error::DuplicateResponse);
        }
        submission.response = Some(response);
        Ok(())
    }
    pub fn disconnect(&mut self) {
        self.disconnect_reason = Some(b"disconnected".to_vec());
        for submission in self.submissions.values_mut() {
            if submission.response.is_none() {
                submission.response = Some(Response::Refused(b"disconnected".to_vec()));
            }
        }
    }
}
