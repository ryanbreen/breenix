//! Pure per-mount request bookkeeping. The caller serializes access and wakes
//! waiters after releasing any locks that prohibit wakeups.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const QUEUE_CAP: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Full,
    Eio,
    Enotconn,
}

struct Entry<Request, Waiter> {
    id: u64,
    request: Option<Request>,
    waiter: Waiter,
    deadline: u64,
}

/// Completion records carry the waiter and its result; producing one does not
/// wake the waiter. Deadlines and `expire` use the same caller-defined clock.
pub struct RequestTable<Request, Waiter, Reply> {
    entries: VecDeque<Entry<Request, Waiter>>,
    next_id: Option<u64>,
    is_dead: bool,
    reply: core::marker::PhantomData<Reply>,
}

impl<Request, Waiter, Reply> RequestTable<Request, Waiter, Reply> {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: Some(0),
            is_dead: false,
            reply: core::marker::PhantomData,
        }
    }

    pub fn submit(
        &mut self,
        request: Request,
        waiter: Waiter,
        deadline: u64,
    ) -> Result<u64, Error> {
        if self.is_dead {
            return Err(Error::Enotconn);
        }
        if self.entries.len() == QUEUE_CAP {
            return Err(Error::Full);
        }
        // Never reuse an ID, including after timeout or integer exhaustion.
        let id = self.next_id.ok_or(Error::Full)?;
        self.next_id = id.checked_add(1);
        self.entries.push_back(Entry {
            id,
            request: Some(request),
            waiter,
            deadline,
        });
        Ok(id)
    }

    pub fn has_capacity(&self) -> bool {
        self.next_id.is_some() && self.entries.len() < QUEUE_CAP
    }

    pub fn take_next(&mut self) -> Option<(u64, Request)> {
        self.entries
            .iter_mut()
            .find_map(|entry| entry.request.take().map(|request| (entry.id, request)))
    }

    /// Inspect a record before copying it, so a short or faulty read does not
    /// consume a daemon request.
    pub fn peek_next(&self) -> Option<(u64, &Request)> {
        self.entries.iter().find_map(|entry| entry.request.as_ref().map(|r| (entry.id, r)))
    }

    /// A copied record may have expired while user memory was faulted in.
    /// Mark the copied id sent, leaving later requests queued.
    pub fn mark_sent(&mut self, id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.request = None;
        }
    }

    /// Withdraw an interrupted caller's request without manufacturing a reply.
    pub fn cancel(&mut self, id: u64) {
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries.remove(index);
        }
    }

    pub fn complete(&mut self, id: u64, reply: Reply) -> Option<(Waiter, Result<Reply, Error>)> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id)?;
        let entry = self.entries.remove(index)?;
        Some((entry.waiter, Ok(reply)))
    }

    pub fn expire(&mut self, now: u64) -> Vec<(Waiter, Result<Reply, Error>)> {
        let mut expired = Vec::new();
        let mut index = 0;
        while index < self.entries.len() {
            if self.entries[index].deadline <= now {
                let entry = self
                    .entries
                    .remove(index)
                    .expect("entry index is in bounds");
                expired.push((entry.waiter, Err(Error::Eio)));
            } else {
                index += 1;
            }
        }
        expired
    }

    pub fn dead(&mut self) -> Vec<(Waiter, Result<Reply, Error>)> {
        self.is_dead = true;
        self.entries
            .drain(..)
            .map(|entry| (entry.waiter, Err(Error::Enotconn)))
            .collect()
    }
}
