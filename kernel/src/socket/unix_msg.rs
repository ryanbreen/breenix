//! Owned, bounded Unix messages. Rights remain counted open references while
//! queued and use the normal close hooks if the receiver discards them.

use super::addr::UnixName;
use super::object::UCred;
use crate::ipc::fd::{FdKind, FileDescriptor};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::syscall::errno::{EAGAIN, EMSGSIZE, ENOMEM};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_RIGHTS: usize = 253; // Linux SCM_MAX_FD.
pub const MAX_DATA: usize = 4 * 1024 * 1024;
const MAX_MESSAGES: usize = 1024;

// A bounded in-flight graph, including unnamed sockets. Checking all roots
// prevents a later send into a child from growing an existing chain past four.
const MAX_INFLIGHT: usize = 16384;
const ETOOMANYREFS: i32 = 109;
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static NEXT_MESSAGE: AtomicUsize = AtomicUsize::new(1);
struct Edge {
    message: usize,
    from: usize,
    to: usize,
    weight: u8,
}
static RIGHTS_GRAPH: IrqSafeMutex<Vec<Edge>> = IrqSafeMutex::new(Vec::new());

fn validate_graph(edges: &[Edge]) -> Result<(), i32> {
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(edges.len() * 2)
        .map_err(|_| ENOMEM)?;
    for edge in edges {
        nodes.push((edge.from, 0u8));
        nodes.push((edge.to, 0));
    }
    nodes.sort_unstable_by_key(|n| n.0);
    nodes.dedup_by_key(|n| n.0);
    for _ in 0..10 {
        let mut changed = false;
        for edge in edges {
            let from = nodes.binary_search_by_key(&edge.from, |n| n.0).unwrap();
            let to = nodes.binary_search_by_key(&edge.to, |n| n.0).unwrap();
            let depth = nodes[to].1 + edge.weight;
            if depth > 4 {
                return Err(ETOOMANYREFS);
            }
            if depth > nodes[from].1 {
                nodes[from].1 = depth;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
    }
    Err(ETOOMANYREFS)
}

/// A queued accept owns its server endpoint too. Include that zero-cost edge
/// so rights sent to an unaccepted endpoint cannot hide a longer release chain.
pub struct PendingLink(usize);
impl PendingLink {
    pub fn new(listener: usize, server: usize) -> Result<Self, i32> {
        let id = NEXT_MESSAGE.fetch_add(1, Ordering::Relaxed);
        let mut graph = RIGHTS_GRAPH.lock();
        if graph.len() == MAX_INFLIGHT {
            return Err(ETOOMANYREFS);
        }
        graph.try_reserve(1).map_err(|_| ENOMEM)?;
        graph.push(Edge {
            message: id,
            from: listener,
            to: server,
            weight: 0,
        });
        if let Err(e) = validate_graph(&graph) {
            graph.pop();
            return Err(e);
        }
        Ok(Self(id))
    }
}
impl Drop for PendingLink {
    fn drop(&mut self) {
        RIGHTS_GRAPH.lock().retain(|edge| edge.message != self.0);
    }
}

pub struct UnixMsg {
    pub data: Vec<u8>,
    pub rights: Vec<FileDescriptor>,
    pub cred: UCred,
    pub from: UnixName,
    offset: usize,
    flight_charge: usize,
    graph_id: usize,
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
            flight_charge: 0,
            graph_id: 0,
        };
        if msg.rights.len() > MAX_RIGHTS || data.len() > MAX_DATA {
            return Err(EMSGSIZE);
        }
        let charge = msg.rights.len();
        INFLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(charge).filter(|n| *n <= MAX_INFLIGHT)
            })
            .map_err(|_| ETOOMANYREFS)?;
        msg.flight_charge = charge;
        msg.data.try_reserve_exact(data.len()).map_err(|_| ENOMEM)?;
        msg.data.extend_from_slice(data);
        Ok(msg)
    }

    pub fn register_rights(&mut self, receiver: usize) -> Result<(), i32> {
        if self.graph_id != 0 || self.rights.is_empty() {
            return Ok(());
        }
        let id = NEXT_MESSAGE.fetch_add(1, Ordering::Relaxed);
        let mut graph = RIGHTS_GRAPH.lock();
        if graph.len().saturating_add(self.rights.len()) > MAX_INFLIGHT {
            return Err(ETOOMANYREFS);
        }
        graph.try_reserve(self.rights.len()).map_err(|_| ENOMEM)?;
        for right in &self.rights {
            if let FdKind::Socket(socket) = &right.kind {
                if socket.family == 1 {
                    graph.push(Edge {
                        message: id,
                        from: receiver,
                        to: alloc::sync::Arc::as_ptr(socket) as usize,
                        weight: 1,
                    });
                }
            }
        }
        if let Err(e) = validate_graph(&graph) {
            graph.retain(|edge| edge.message != id);
            return Err(e);
        }
        self.graph_id = id;
        Ok(())
    }
    fn leave_flight(&mut self) {
        if self.graph_id != 0 {
            RIGHTS_GRAPH
                .lock()
                .retain(|edge| edge.message != self.graph_id);
            self.graph_id = 0;
        }
        INFLIGHT.fetch_sub(core::mem::take(&mut self.flight_charge), Ordering::AcqRel);
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
        self.leave_flight();
        let rights = core::mem::take(&mut self.rights);
        if !rights.is_empty() {
            // Bounded by SCM_MAX_FD, including error/close paths.
            crate::task::process_task::close_extracted_fds(rights.into_iter().enumerate());
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
        msg.leave_flight();
        let rights = core::mem::take(&mut msg.rights);
        self.bytes -= before - msg.charge();
        rights
    }
}
