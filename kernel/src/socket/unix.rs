//! Unix stream, datagram and sequenced-packet sockets with inode and abstract names.
//! Queued rights retain their counted descriptions; weak peer links preserve EOF.
// The Unix protocol keeps all queues and state behind one masked mutex per
// socket. Peer links are weak; pending accepts retain the server endpoint.
// No wake, filesystem lookup, user copy or descriptor close runs under it.
use super::addr::{SockAddr, UnixName};
use super::object::{OptBuf, ProtoOps, RecvMsg, SendMsg, Socket, SocketProto, UCred};
use super::unix_msg::{UnixMsg, UnixQueue};
use super::unix_ns;
use super::wait::wait_until;
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::syscall::errno::*;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const ESOCKTNOSUPPORT: i32 = 94;
const STREAM: u16 = 1;
const DGRAM: u16 = 2;
const SEQPACKET: u16 = 5;
const PEEK: u32 = 2;
const TRUNC: u32 = 0x20;
const WAITALL: u32 = 0x100;
const OOB: u32 = 1;
const MAX_BACKLOG: usize = 4096;
const MAX_QUEUE_BYTES: usize = super::unix_msg::MAX_DATA;
static NEXT_AUTOBIND: AtomicU32 = AtomicU32::new(0);

struct UnixState {
    name: UnixName,
    bound: bool,
    namespace_id: usize,
    listening: bool,
    backlog: usize,
    pending: VecDeque<Arc<Socket>>,
    writers: Vec<Weak<Socket>>,
    peer: Option<Weak<Socket>>,
    peer_name: UnixName,
    peer_cred: UCred,
    listen_cred: UCred,
    connected: bool,
    peer_write_closed: bool,
    peer_read_closed: bool,
    released: bool,
    rx: UnixQueue,
}
impl UnixState {
    fn new() -> Self {
        Self {
            name: UnixName::Unnamed,
            bound: false,
            namespace_id: 0,
            listening: false,
            backlog: 1,
            pending: VecDeque::new(),
            writers: Vec::new(),
            peer: None,
            peer_name: UnixName::Unnamed,
            peer_cred: UCred {
                pid: 0,
                uid: u32::MAX,
                gid: u32::MAX,
            },
            listen_cred: UCred {
                pid: 0,
                uid: u32::MAX,
                gid: u32::MAX,
            },
            connected: false,
            peer_write_closed: false,
            peer_read_closed: false,
            released: false,
            rx: UnixQueue::new(),
        }
    }
}

pub struct UnixSock {
    kind: u16,
    inner: IrqSafeMutex<UnixState>,
}
impl UnixSock {
    pub fn supports_kind(kind: u16) -> bool {
        matches!(kind, STREAM | DGRAM | SEQPACKET)
    }

    pub fn new(kind: u16) -> Self {
        Self {
            kind,
            inner: IrqSafeMutex::new(UnixState::new()),
        }
    }

    pub fn new_pair(kind: u16, cred: UCred) -> Result<(Arc<Socket>, Arc<Socket>), i32> {
        if !Self::supports_kind(kind) {
            return Err(ESOCKTNOSUPPORT);
        }
        let a = Socket::new(1, kind, 0, SocketProto::Unix(Self::new(kind)))?;
        let b = Socket::new(1, kind, 0, SocketProto::Unix(Self::new(kind)))?;
        Self::link(&a, &b, UnixName::Unnamed, UnixName::Unnamed, &cred, &cred);
        Ok((a, b))
    }

    pub(crate) fn detach(&mut self) -> Self {
        let state = core::mem::replace(&mut *self.inner.lock(), UnixState::new());
        Self {
            kind: self.kind,
            inner: IrqSafeMutex::new(state),
        }
    }

    fn of(sk: &Socket) -> &Self {
        match &sk.proto {
            SocketProto::Unix(unix) => unix,
            _ => unreachable!(),
        }
    }
    fn link(
        a: &Arc<Socket>,
        b: &Arc<Socket>,
        aname: UnixName,
        bname: UnixName,
        ac: &UCred,
        bc: &UCred,
    ) {
        {
            let mut s = Self::of(a).inner.lock();
            s.peer = Some(Arc::downgrade(b));
            s.peer_name = bname;
            s.peer_cred = copy_cred(bc);
            s.connected = true;
            s.peer_read_closed = false;
            s.peer_write_closed = false;
        }
        {
            let mut s = Self::of(b).inner.lock();
            s.peer = Some(Arc::downgrade(a));
            s.peer_name = aname;
            s.peer_cred = copy_cred(ac);
            s.connected = true;
            s.peer_read_closed = false;
            s.peer_write_closed = false;
        }
    }
    fn peer(&self) -> Option<Arc<Socket>> {
        self.inner.lock().peer.as_ref().and_then(Weak::upgrade)
    }
    fn register_writer(state: &mut UnixState, sk: &Arc<Socket>) -> Result<(), i32> {
        state.writers.retain(|weak| weak.strong_count() != 0);
        if state
            .writers
            .iter()
            .any(|weak| weak.as_ptr() == Arc::as_ptr(sk))
        {
            return Ok(());
        }
        if state.writers.len() == MAX_BACKLOG {
            return Err(ENOBUFS);
        }
        state.writers.try_reserve(1).map_err(|_| ENOMEM)?;
        state.writers.push(Arc::downgrade(sk));
        Ok(())
    }
    fn notify_writers(&self) {
        let writers = core::mem::take(&mut self.inner.lock().writers);
        for weak in writers {
            if let Some(writer) = weak.upgrade() {
                writer.common.notify();
            }
        }
    }
    #[cfg(feature = "boot_tests")]
    pub(crate) fn is_peer(&self, other: &Arc<Socket>) -> bool {
        self.inner
            .lock()
            .peer
            .as_ref()
            .is_some_and(|peer| peer.as_ptr() == Arc::as_ptr(other))
    }

    #[cfg(feature = "boot_tests")]
    pub(crate) fn write_witness(&self, sk: &Socket, tid: u64) -> (u64, u64, u64) {
        let queued = sk.common.wq.contains_waiter(tid) as u64;
        let occupancy = if let Some(peer) = self.peer() {
            let state = Self::of(&peer).inner.lock();
            let bytes = state.rx.charged_bytes() as u64;
            drop(state);
            bytes
        } else {
            0
        };
        (
            queued,
            occupancy,
            sk.common.sndbuf().min(MAX_QUEUE_BYTES) as u64,
        )
    }
    fn autobind(&self, sk: &Arc<Socket>) -> Result<UnixName, i32> {
        for _ in 0..(1 << 20) {
            let id = NEXT_AUTOBIND.fetch_add(1, Ordering::Relaxed) & 0xfffff;
            let mut name = Vec::new();
            name.try_reserve_exact(5).map_err(|_| ENOMEM)?;
            for shift in [16, 12, 8, 4, 0] {
                name.push(b"0123456789abcdef"[((id >> shift) & 15) as usize]);
            }
            let name = UnixName::Abstract(name);
            match unix_ns::bind_name(&name, Arc::downgrade(sk)) {
                Ok(()) => return Ok(name),
                Err(EADDRINUSE) => {}
                Err(e) => return Err(e),
            }
        }
        Err(EADDRINUSE)
    }
}
fn copy_cred(c: &UCred) -> UCred {
    UCred {
        pid: c.pid,
        uid: c.uid,
        gid: c.gid,
    }
}

impl ProtoOps for UnixSock {
    fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        let SockAddr::Unix(name) = addr else {
            return Err(EINVAL);
        };
        // Claim this socket before dropping its mutex to create the node.
        {
            let mut s = self.inner.lock();
            if s.bound || s.listening {
                return Err(EINVAL);
            }
            s.bound = true;
        }
        let result = if matches!(name, UnixName::Unnamed) {
            self.autobind(sk)
        } else {
            unix_ns::bind_name(name, Arc::downgrade(sk)).map(|()| name.clone())
        };
        let mut s = self.inner.lock();
        match result {
            Ok(name) => {
                s.name = name;
                s.namespace_id = Arc::as_ptr(sk) as usize;
                Ok(())
            }
            Err(e) => {
                s.bound = false;
                Err(e)
            }
        }
    }
    fn listen(&self, sk: &Arc<Socket>, backlog: i32) -> Result<(), i32> {
        if self.kind == DGRAM {
            return Err(EOPNOTSUPP);
        }
        let cred = super::object::caller_ucred();
        let mut s = self.inner.lock();
        if !s.bound {
            return Err(EINVAL);
        }
        if s.connected {
            return Err(EINVAL);
        }
        s.listening = true;
        s.listen_cred = cred;
        s.backlog = if backlog < 0 {
            MAX_BACKLOG
        } else {
            (backlog as usize).min(MAX_BACKLOG - 1) + 1
        };
        drop(s);
        sk.common.notify();
        Ok(())
    }
    fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32> {
        if matches!(addr, SockAddr::Unspec) && self.kind == DGRAM {
            let mut s = self.inner.lock();
            s.peer = None;
            s.connected = false;
            s.peer_name = UnixName::Unnamed;
            drop(s);
            sk.common.notify();
            return Ok(());
        }
        let SockAddr::Unix(name) = addr else {
            return Err(EINVAL);
        };
        let cred = super::object::caller_ucred();
        let listener = unix_ns::lookup(name)?;
        let proto = Self::of(&listener);
        if listener.kind != self.kind {
            return Err(EPROTOTYPE);
        }
        let local_name = {
            let s = self.inner.lock();
            if s.listening {
                return Err(EINVAL);
            }
            if s.connected && self.kind != DGRAM {
                return Err(EISCONN);
            }
            s.name.clone()
        };
        if self.kind == DGRAM {
            let remote_name = proto.inner.lock().name.clone();
            let mut s = self.inner.lock();
            s.peer = Some(Arc::downgrade(&listener));
            s.peer_name = remote_name;
            s.peer_cred = cred;
            s.connected = true;
            s.peer_read_closed = false;
            s.peer_write_closed = false;
            drop(s);
            sk.common.notify();
            return Ok(());
        }
        let server = Socket::new(1, self.kind, 0, SocketProto::Unix(Self::new(self.kind)))?;
        let listener_ref = Arc::downgrade(&listener);
        drop(listener);
        wait_until(sk, nonblock, sk.common.snd_deadline(), || {
            let Some(listener) = listener_ref.upgrade() else {
                return Some(Err(ECONNREFUSED));
            };
            let proto = Self::of(&listener);
            let mut state = proto.inner.lock();
            if !state.listening || state.released {
                return Some(Err(ECONNREFUSED));
            }
            if state.pending.len() >= state.backlog {
                if !nonblock {
                    if let Err(e) = Self::register_writer(&mut state, sk) {
                        return Some(Err(e));
                    }
                }
                return None;
            }
            if state.pending.try_reserve(1).is_err() {
                return Some(Err(ENOMEM));
            }
            let server_name = state.name.clone();
            let server_cred = copy_cred(&state.listen_cred);
            // Fully link both private endpoints before accept can see the
            // server. Only connect nests endpoint locks below a listener.
            // Other operations release their local lock before peer access.
            {
                let mut endpoint = Self::of(&server).inner.lock();
                endpoint.name = server_name.clone();
                endpoint.bound = true;
            }
            Self::link(
                sk,
                &server,
                local_name.clone(),
                server_name,
                &cred,
                &server_cred,
            );
            state.pending.push_back(server.clone());
            drop(state);
            Some(Ok(()))
        })?;
        if let Some(listener) = listener_ref.upgrade() {
            listener.common.notify();
        }
        sk.common.notify();
        server.common.notify();
        Ok(())
    }
    fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        let accepted = wait_until(sk, nonblock, sk.common.rcv_deadline(), || {
            let mut state = self.inner.lock();
            if !state.listening {
                return Some(Err(EINVAL));
            }
            state.pending.pop_front().map(Ok)
        })?;
        sk.common.notify();
        self.notify_writers();
        let name = Self::of(&accepted).inner.lock().peer_name.clone();
        Ok((accepted, SockAddr::Unix(name)))
    }
    fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
        if msg.flags & OOB != 0 {
            return Err(EOPNOTSUPP);
        }
        if sk.common.shut_wr() {
            return Err(EPIPE);
        }
        let peer = if let Some(addr) = &msg.dest {
            if self.kind != DGRAM {
                return Err(EISCONN);
            }
            let SockAddr::Unix(name) = addr else {
                return Err(EINVAL);
            };
            unix_ns::lookup(name)?
        } else {
            let state = self.inner.lock();
            state
                .peer
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or(if self.kind == DGRAM {
                    if state.connected {
                        ECONNREFUSED
                    } else {
                        EDESTADDRREQ
                    }
                } else {
                    EPIPE
                })?
        };
        if peer.kind != self.kind {
            return Err(EPROTOTYPE);
        }
        if self.kind == STREAM && msg.data.is_empty() {
            return if peer.common.shut_rd() {
                Err(EPIPE)
            } else {
                Ok(0)
            };
        }
        let peer_ref = Arc::downgrade(&peer);
        drop(peer);
        let limit = sk.common.sndbuf().min(MAX_QUEUE_BYTES);
        if self.kind != STREAM && msg.data.len().max(1) > limit.saturating_sub(32) {
            return Err(EMSGSIZE);
        }
        let from = self.inner.lock().name.clone();
        let payload = if self.kind == STREAM {
            &msg.data[..msg.data.len().min(limit)]
        } else {
            msg.data
        };
        let mut pending = Some(UnixMsg::new(
            payload,
            core::mem::take(&mut msg.rights),
            copy_cred(&msg.cred),
            from,
        )?);
        let n = wait_until(sk, msg.nonblock, sk.common.snd_deadline(), || {
            if sk.common.shut_wr() {
                return Some(Err(EPIPE));
            }
            let Some(peer) = peer_ref.upgrade() else {
                return Some(Err(if self.kind == DGRAM {
                    ECONNREFUSED
                } else {
                    EPIPE
                }));
            };
            let mut state = Self::of(&peer).inner.lock();
            if state.released || peer.common.shut_rd() {
                return Some(Err(if self.kind == DGRAM {
                    ECONNREFUSED
                } else {
                    EPIPE
                }));
            }
            if self.kind == DGRAM {
                if let Some(connected) = &state.peer {
                    if connected.as_ptr() != Arc::as_ptr(sk) {
                        return Some(Err(EPERM));
                    }
                }
            }
            let message = pending.as_ref().unwrap();
            let room = if self.kind == STREAM && message.rights.is_empty() {
                state.rx.stream_room(limit)
            } else {
                state.rx.room(limit)
            };
            let rights_charge =
                message.rights.len() * core::mem::size_of::<crate::ipc::fd::FileDescriptor>();
            if room <= rights_charge {
                if !msg.nonblock {
                    if let Err(e) = Self::register_writer(&mut state, sk) {
                        return Some(Err(e));
                    }
                }
                return None;
            }
            let mut message = pending.take().unwrap();
            let n = if self.kind == STREAM {
                message.data.len().min(room - rights_charge)
            } else {
                msg.data.len()
            };
            if self.kind == STREAM {
                message.data.truncate(n);
            }
            let result = if self.kind == STREAM {
                state.rx.push_stream(message, limit)
            } else {
                state.rx.push(message, limit)
            };
            match result {
                Ok(()) => Some(Ok(n)),
                Err((EAGAIN, message)) => {
                    pending = Some(message);
                    if !msg.nonblock {
                        if let Err(e) = Self::register_writer(&mut state, sk) {
                            return Some(Err(e));
                        }
                    }
                    None
                }
                Err((e, message)) => {
                    pending = Some(message);
                    Some(Err(e))
                }
            }
        });
        // A failed send's rights close outside both protocol locks.
        drop(pending);
        let n = n?;
        if let Some(peer) = peer_ref.upgrade() {
            peer.common.notify();
        }
        Ok(n)
    }
    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
        if msg.flags & OOB != 0 {
            return Err(EOPNOTSUPP);
        }
        if self.kind == STREAM && msg.cap == 0 {
            return Ok(0);
        }
        let peek = msg.flags & PEEK != 0;
        let deadline = sk.common.rcv_deadline();
        let mut copied = 0;
        loop {
            let result = wait_until(sk, msg.nonblock, deadline, || {
                let mut state = self.inner.lock();
                if sk.common.shut_rd() {
                    return Some(Ok((0, true)));
                }
                if state.rx.is_empty() {
                    if self.kind != DGRAM && !state.connected {
                        return Some(Err(ENOTCONN));
                    }
                    if self.kind != DGRAM
                        && (state.peer_write_closed
                            || state.peer.as_ref().is_some_and(|p| p.strong_count() == 0))
                    {
                        return Some(Ok((0, true)));
                    }
                    return None;
                }
                let mut n = 0;
                let mut length = 0;
                if peek {
                    for front in state.rx.iter() {
                        length = front.bytes().len();
                        let take = length.min(msg.cap - copied - n);
                        if msg.data.try_reserve(take).is_err() {
                            return Some(Err(ENOMEM));
                        }
                        if !front.rights.is_empty() {
                            match clone_rights(&front.rights) {
                                Ok(rights) => msg.rights = rights,
                                Err(e) => return Some(Err(e)),
                            }
                        }
                        if n == 0 {
                            msg.from = Some(SockAddr::Unix(front.from.clone()));
                        }
                        msg.data.extend_from_slice(&front.bytes()[..take]);
                        n += take;
                        if self.kind != STREAM {
                            if take < length {
                                msg.out_flags |= TRUNC;
                            }
                            break;
                        }
                        if copied + n == msg.cap || !msg.rights.is_empty() {
                            break;
                        }
                    }
                } else {
                    while let Some(front) = state.rx.front() {
                        length = front.bytes().len();
                        let take = length.min(msg.cap - copied - n);
                        if msg.data.try_reserve(take).is_err() {
                            return Some(if n == 0 { Err(ENOMEM) } else { Ok((n, false)) });
                        }
                        if n == 0 {
                            msg.from = Some(SockAddr::Unix(front.from.clone()));
                        }
                        msg.data.extend_from_slice(&front.bytes()[..take]);
                        let rights = state.rx.take_rights();
                        if !rights.is_empty() {
                            msg.rights = rights;
                        }
                        n += take;
                        if self.kind != STREAM {
                            if take < length {
                                msg.out_flags |= TRUNC;
                            }
                            state.rx.pop();
                            break;
                        }
                        state.rx.consume(take);
                        if state.rx.front().is_some_and(|m| m.bytes().is_empty()) {
                            state.rx.pop();
                        }
                        if copied + n == msg.cap || !msg.rights.is_empty() {
                            break;
                        }
                    }
                }
                Some(Ok((
                    if self.kind != STREAM && msg.flags & TRUNC != 0 {
                        length
                    } else {
                        n
                    },
                    false,
                )))
            });
            match result {
                Ok((n, eof)) => {
                    copied += n;
                    if !peek {
                        sk.common.notify();
                        self.notify_writers();
                        if let Some(peer) = self.peer() {
                            peer.common.notify();
                        }
                    }
                    if eof
                        || self.kind != STREAM
                        || peek
                        || msg.flags & WAITALL == 0
                        || copied == msg.cap
                        || !msg.rights.is_empty()
                    {
                        return Ok(copied);
                    }
                }
                Err(e) => return if copied > 0 { Ok(copied) } else { Err(e) },
            }
        }
    }
    fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32> {
        if how > 2 {
            return Err(EINVAL);
        }
        if !self.inner.lock().connected {
            return Err(ENOTCONN);
        }
        sk.common.set_shut(how);
        let discarded = if how == 0 || how == 2 {
            Some(core::mem::replace(
                &mut self.inner.lock().rx,
                UnixQueue::new(),
            ))
        } else {
            None
        };
        drop(discarded);
        self.notify_writers();
        if let Some(peer) = self.peer() {
            let mut state = Self::of(&peer).inner.lock();
            if how == 0 || how == 2 {
                state.peer_read_closed = true;
            }
            if how == 1 || how == 2 {
                state.peer_write_closed = true;
            }
            drop(state);
            peer.common.notify();
        }
        sk.common.notify();
        Ok(())
    }
    fn local_name(&self, _sk: &Socket) -> SockAddr {
        SockAddr::Unix(self.inner.lock().name.clone())
    }
    fn peer_name(&self, _sk: &Socket) -> Result<SockAddr, i32> {
        let state = self.inner.lock();
        if !state.connected {
            return Err(ENOTCONN);
        }
        Ok(SockAddr::Unix(state.peer_name.clone()))
    }
    fn poll(&self, sk: &Socket) -> u16 {
        let (listening, queued, connected, read_closed, write_closed, peer) = {
            let s = self.inner.lock();
            (
                s.listening,
                if s.listening {
                    !s.pending.is_empty()
                } else {
                    !s.rx.is_empty()
                },
                s.connected,
                s.peer_write_closed,
                s.peer_read_closed,
                s.peer.as_ref().and_then(Weak::upgrade),
            )
        };
        if listening {
            return if queued { 1 } else { 0 };
        }
        let peer_gone = connected && peer.is_none();
        let eof = self.kind != DGRAM && (read_closed || peer_gone || sk.common.shut_rd());
        let mut events = if queued || eof { 1 } else { 0 };
        if eof {
            events |= 0x2000;
        }
        if self.kind != DGRAM
            && (peer_gone
                || ((read_closed || sk.common.shut_rd()) && (write_closed || sk.common.shut_wr())))
        {
            events |= 0x10;
        }
        if self.kind == DGRAM && !connected {
            events |= 4;
        } else if let Some(peer) = peer {
            if !peer.common.shut_rd()
                && !sk.common.shut_wr()
                && (if self.kind == STREAM {
                    Self::of(&peer)
                        .inner
                        .lock()
                        .rx
                        .stream_room(sk.common.sndbuf().min(MAX_QUEUE_BYTES))
                } else {
                    Self::of(&peer)
                        .inner
                        .lock()
                        .rx
                        .room(sk.common.sndbuf().min(MAX_QUEUE_BYTES))
                }) > 0
            {
                events |= 4;
            }
        }
        events
    }
    fn notifies(&self) -> bool {
        true
    }
    fn is_listening(&self, _sk: &Socket) -> bool {
        self.inner.lock().listening
    }
    fn getsockopt(&self, _sk: &Socket, level: i32, name: i32, out: &mut OptBuf) -> Result<(), i32> {
        if level != 1 || name != 17 {
            return Err(ENOPROTOOPT);
        }
        let s = self.inner.lock();
        out.bytes[..4].copy_from_slice(&s.peer_cred.pid.to_ne_bytes());
        out.bytes[4..8].copy_from_slice(&s.peer_cred.uid.to_ne_bytes());
        out.bytes[8..12].copy_from_slice(&s.peer_cred.gid.to_ne_bytes());
        out.len = 12;
        Ok(())
    }
    fn release(&self, _sk: &Socket) {
        let namespace_id = self.inner.lock().namespace_id;
        if namespace_id != 0 {
            unix_ns::unbind_id(namespace_id);
        }
        let old = {
            let mut s = self.inner.lock();
            let old = core::mem::replace(&mut *s, UnixState::new());
            s.released = true;
            old
        };
        if let Some(peer) = old.peer.as_ref().and_then(Weak::upgrade) {
            let mut state = Self::of(&peer).inner.lock();
            state.peer_write_closed = true;
            state.peer_read_closed = true;
            drop(state);
            peer.common.notify();
        }
        for weak in &old.writers {
            if let Some(writer) = weak.upgrade() {
                writer.common.notify();
            }
        }
        drop(old);
    }
}

fn clone_rights(
    rights: &[crate::ipc::fd::FileDescriptor],
) -> Result<Vec<crate::ipc::fd::FileDescriptor>, i32> {
    let mut result = Vec::new();
    result.try_reserve_exact(rights.len()).map_err(|_| ENOMEM)?;
    for fd in rights {
        result.push(fd.counted_clone());
    }
    Ok(result)
}
