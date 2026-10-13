//! A message queue's messages, its waiting senders and receivers, the
//! per-user charge for queues, and the checks on names, attributes and
//! timeouts: the parts of POSIX message queues that need no process, lock or
//! user memory. Allocations here are fallible and reported as ENOMEM.

use crate::syscall::errno::{EACCES, EINVAL, EMFILE, EMSGSIZE, ENAMETOOLONG, ENOENT, ENOMEM, ENOSPC};
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// Priorities run from 0 to MQ_PRIO_MAX - 1 (Linux's value).
pub const MQ_PRIO_MAX: u32 = 32768;
/// The attributes of a queue created with no `mq_attr`, and the most a
/// caller without privilege may ask for (Linux's msg_default and
/// msgsize_default, msg_max and msgsize_max).
pub const DFLT_MAXMSG: i64 = 10;
pub const DFLT_MSGSIZE: i64 = 8192;
/// The most a privileged caller may ask for (Linux's HARD_MSGMAX and
/// HARD_MSGSIZEMAX).
pub const HARD_MAXMSG: i64 = 65536;
pub const HARD_MSGSIZE: i64 = 16 * 1024 * 1024;
/// The longest queue name, without its leading slash.
pub const NAME_MAX: usize = 255;
/// The longest string read as a name (PATH_MAX, terminator included): a
/// longer one is ENAMETOOLONG before the name is checked.
pub const PATH_MAX: usize = 4096;
/// The only flag mq_setattr may change.
pub const O_NONBLOCK: i64 = 0o4000;
/// How many queues may exist at once for callers without privilege
/// (Linux's queues_max default).
pub const QUEUES_MAX: usize = 256;
/// RLIMIT_MSGQUEUE's default, in bytes (Linux's MQ_BYTES_MAX).
pub const DEFAULT_MSGQUEUE_BYTES: u64 = 819_200;
/// The bookkeeping getrlimit(2) adds to a queue's charge: per message, and
/// per priority up to MQ_PRIO_MAX.
const MSG_OVERHEAD: u64 = 48;
const PRIO_OVERHEAD: u64 = 48;

/// struct mq_attr as the Linux ABI lays it out on both architectures.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MqAttr {
    pub mq_flags: i64,
    pub mq_maxmsg: i64,
    pub mq_msgsize: i64,
    pub mq_curmsgs: i64,
    pub reserved: [i64; 4],
}

const _: () = assert!(core::mem::size_of::<MqAttr>() == 64);

/// Check a queue name as the kernel receives it, without the leading slash
/// the C library strips. Names are byte strings.
pub fn check_name(name: &[u8]) -> Result<(), u64> {
    check_scanned(name, name.len(), name.contains(&b'/'))
}

/// `check_name` for a name read from user memory: `stored` is its first
/// bytes, `len` its whole length and `slash` whether it holds a `/`
/// anywhere. Empty is ENOENT, `.`, `..` or one with a slash EACCES, and one
/// longer than NAME_MAX ENAMETOOLONG, checked in that order (Linux).
pub fn check_scanned(stored: &[u8], len: usize, slash: bool) -> Result<(), u64> {
    if len == 0 {
        return Err(ENOENT as u64);
    }
    if slash || (len == stored.len() && (stored == b"." || stored == b"..")) {
        return Err(EACCES as u64);
    }
    if len > NAME_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    Ok(())
}

/// The (maxmsg, msgsize) a new queue gets from `attr`: the defaults when no
/// attributes are given; EINVAL for a value of 0 or less, or above the caller's cap.
pub fn check_attr(attr: Option<&MqAttr>, privileged: bool) -> Result<(usize, usize), u64> {
    let Some(attr) = attr else {
        return Ok((DFLT_MAXMSG as usize, DFLT_MSGSIZE as usize));
    };
    let (max_msgs, max_size) = if privileged {
        (HARD_MAXMSG, HARD_MSGSIZE)
    } else {
        (DFLT_MAXMSG, DFLT_MSGSIZE)
    };
    if !(1..=max_msgs).contains(&attr.mq_maxmsg) || !(1..=max_size).contains(&attr.mq_msgsize) {
        return Err(EINVAL as u64);
    }
    Ok((attr.mq_maxmsg as usize, attr.mq_msgsize as usize))
}

/// What a queue of `maxmsg` messages of `msgsize` bytes is charged against
/// its creator's RLIMIT_MSGQUEUE: the message space plus the bookkeeping
/// getrlimit(2) describes.
pub fn queue_bytes(maxmsg: usize, msgsize: usize) -> u64 {
    let maxmsg = maxmsg as u64;
    maxmsg * msgsize as u64 + maxmsg * MSG_OVERHEAD + maxmsg.min(u64::from(MQ_PRIO_MAX)) * PRIO_OVERHEAD
}

/// Check a send before its message is copied in: a priority of MQ_PRIO_MAX
/// or more is EINVAL, a message longer than the queue's `msgsize` EMSGSIZE.
pub fn check_send(len: usize, prio: u32, msgsize: usize) -> Result<(), u64> {
    if prio >= MQ_PRIO_MAX {
        return Err(EINVAL as u64);
    }
    if len > msgsize {
        return Err(EMSGSIZE as u64);
    }
    Ok(())
}

/// Check a receive's buffer: shorter than the queue's `msgsize` is
/// EMSGSIZE, whatever the length of the message at the head.
pub fn check_receive(len: usize, msgsize: usize) -> Result<(), u64> {
    if len < msgsize {
        return Err(EMSGSIZE as u64);
    }
    Ok(())
}

/// An absolute timeout's nanoseconds since the epoch. A negative tv_sec, or
/// a tv_nsec outside 0..1000000000, is EINVAL whether or not the call would
/// wait (Linux checks the timespec before the queue). Later dates saturate.
pub fn check_timeout(sec: i64, nsec: i64) -> Result<u64, u64> {
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL as u64);
    }
    Ok((sec as u64).saturating_mul(1_000_000_000).saturating_add(nsec as u64))
}

/// mq_setattr's new flags: only O_NONBLOCK may be set; any other bit is
/// EINVAL (Linux). mq_maxmsg, mq_msgsize and mq_curmsgs are ignored.
pub fn check_setattr(attr: &MqAttr) -> Result<bool, u64> {
    if attr.mq_flags & !O_NONBLOCK != 0 {
        return Err(EINVAL as u64);
    }
    Ok(attr.mq_flags & O_NONBLOCK != 0)
}

/// Why a message was not queued.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The queue holds `maxmsg` messages; the message is handed back.
    Full(Vec<u8>),
    /// No memory for the message's place in the queue.
    NoMemory(Vec<u8>),
}

/// A queued message: ordered by priority, then by arrival, older first.
#[derive(Debug)]
struct Stored {
    prio: u32,
    seq: u64,
    msg: Vec<u8>,
}

impl PartialEq for Stored {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Stored {}

impl PartialOrd for Stored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Stored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.prio.cmp(&other.prio).then(other.seq.cmp(&self.seq))
    }
}

/// A queue's messages: at most `maxmsg` sent ones, taken highest priority
/// first and oldest first within a priority. The heap grows fallibly and
/// keeps its capacity, so a push into room a pop made needs no memory.
///
/// A place can also be reserved for a message the kernel will deliver later
/// (a SIGEV_THREAD notification, `push_reserved`): the heap always has room
/// for every reserved place, and a reserved message is queued even past
/// `maxmsg`, so delivering one needs no memory and cannot find the queue
/// full.
#[derive(Debug)]
pub struct MessageStore {
    maxmsg: usize,
    next_seq: u64,
    reserved: usize,
    heap: BinaryHeap<Stored>,
}

impl MessageStore {
    pub const fn new(maxmsg: usize) -> Self {
        Self { maxmsg, next_seq: 0, reserved: 0, heap: BinaryHeap::new() }
    }

    /// Make room in the heap for the queued messages, the reserved places
    /// and `more`.
    fn room(&mut self, more: usize) -> bool {
        let wanted = self.reserved + more;
        self.heap.capacity() - self.heap.len() >= wanted || self.heap.try_reserve(wanted).is_ok()
    }

    /// The messages queued.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.heap.len() >= self.maxmsg
    }

    /// Queue `msg` at `prio` behind any of the same priority.
    pub fn push(&mut self, prio: u32, msg: Vec<u8>) -> Result<(), Refused> {
        if self.is_full() {
            return Err(Refused::Full(msg));
        }
        if !self.room(1) {
            return Err(Refused::NoMemory(msg));
        }
        self.queue(prio, msg);
        Ok(())
    }

    fn queue(&mut self, prio: u32, msg: Vec<u8>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Stored { prio, seq, msg });
    }

    /// Reserve a place for a message `push_reserved` queues later: ENOMEM
    /// with no memory for it.
    pub fn reserve(&mut self) -> Result<(), u64> {
        if !self.room(1) {
            return Err(ENOMEM as u64);
        }
        self.reserved += 1;
        Ok(())
    }

    /// Give back a place `reserve` took and no message used.
    pub fn unreserve(&mut self) {
        self.reserved = self.reserved.saturating_sub(1);
    }

    /// Queue `msg` at `prio` in a place `reserve` took: no memory is
    /// allocated, and `maxmsg` does not apply.
    pub fn push_reserved(&mut self, prio: u32, msg: Vec<u8>) {
        self.unreserve();
        self.queue(prio, msg);
    }

    /// The bytes of the messages queued.
    pub fn bytes(&self) -> usize {
        self.heap.iter().map(|s| s.msg.len()).sum()
    }

    /// The places reserved and not yet used.
    pub fn reserved(&self) -> usize {
        self.reserved
    }

    /// Take the oldest message of the highest priority, with its priority.
    pub fn pop(&mut self) -> Option<(Vec<u8>, u32)> {
        self.heap.pop().map(|s| (s.msg, s.prio))
    }
}

/// A sender or receiver waiting on a queue.
#[derive(Debug)]
pub struct Waiter {
    /// The waiting thread's ID.
    pub id: u64,
    seq: u64,
    /// The operation was completed for it: a receiver was handed a message,
    /// or a sender's message was queued.
    pub done: bool,
    /// A receiver's message once handed over; a sender's until it is queued.
    pub msg: Option<(Vec<u8>, u32)>,
}

/// The senders or the receivers waiting on a queue. The one served next is
/// the waiting one of highest scheduling rank that has waited longest. A
/// waiter's rank is read when a waiter is chosen, not when it began to wait,
/// so a priority changed while it waits counts (POSIX mq_send and
/// mq_receive).
#[derive(Debug, Default)]
pub struct WaitList {
    next_seq: u64,
    list: Vec<Waiter>,
}

impl WaitList {
    pub const fn new() -> Self {
        Self { next_seq: 0, list: Vec::new() }
    }

    /// Add thread `id`, carrying `msg` for a sender. With no memory for its
    /// entry the message is handed back.
    pub fn add(&mut self, id: u64, msg: Option<(Vec<u8>, u32)>) -> Result<(), Option<(Vec<u8>, u32)>> {
        if self.list.try_reserve(1).is_err() {
            return Err(msg);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.list.push(Waiter { id, seq, done: false, msg });
        Ok(())
    }

    /// Whether any waiter is still waiting.
    pub fn any_waiting(&self) -> bool {
        self.list.iter().any(|w| !w.done)
    }

    /// The still-waiting waiter to serve next, given each thread's current
    /// scheduling rank (`rank`, higher first).
    pub fn next_waiting(&mut self, rank: &dyn Fn(u64) -> u8) -> Option<&mut Waiter> {
        self.list
            .iter_mut()
            .filter(|w| !w.done)
            .map(|w| (rank(w.id), w))
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.seq.cmp(&a.1.seq)))
            .map(|(_, w)| w)
    }

    /// Thread `id`'s entry.
    pub fn get(&self, id: u64) -> Option<&Waiter> {
        self.list.iter().find(|w| w.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Waiter> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    /// Remove thread `id`'s entry.
    pub fn remove(&mut self, id: u64) -> Option<Waiter> {
        let at = self.list.iter().position(|w| w.id == id)?;
        Some(self.list.swap_remove(at))
    }
}

/// What a send did.
#[derive(Debug, PartialEq, Eq)]
pub enum Sent<N> {
    /// A waiting receiver was handed the message: the queue did not change.
    HandedOff,
    /// The message was queued; `notify` is the registration it used up,
    /// when it arrived at an empty queue. `served` says waiting receivers
    /// were handed messages a delivery had left queued (`Channel::serve`).
    Queued { notify: Option<N>, served: bool },
    /// The queue is full; the message is handed back.
    Full(Vec<u8>),
    /// No memory for the message's place in the queue.
    NoMemory,
}

/// A queue's messages, its waiters and its notification registration: the
/// decisions a send or a receive makes. A message for a waiting receiver goes
/// to it directly, and the room a receive makes goes to a waiting sender, so
/// the queue holds messages only while no receiver waits and has room only
/// while no sender waits. The one exception is a reserved delivery
/// (`MessageStore::push_reserved`), which is queued without serving a
/// receiver because it may be made where the receivers' ranks cannot be
/// read; the next operation on the queue serves it (`serve`).
#[derive(Debug)]
pub struct Channel<N> {
    pub msgs: MessageStore,
    pub receivers: WaitList,
    pub senders: WaitList,
    pub notify: Option<N>,
}

impl<N> Channel<N> {
    pub const fn new(maxmsg: usize) -> Self {
        Self { msgs: MessageStore::new(maxmsg), receivers: WaitList::new(), senders: WaitList::new(), notify: None }
    }

    /// Send `msg` at `prio`: to the next waiting receiver, or into the queue.
    /// Only a message queued into an empty queue uses the registration (POSIX
    /// mq_notify: a waiting receiver takes the message and no notification
    /// is sent). `rank` gives each waiting thread's scheduling rank.
    pub fn send(&mut self, prio: u32, msg: Vec<u8>, rank: &dyn Fn(u64) -> u8) -> Sent<N> {
        if self.msgs.is_empty() {
            if let Some(receiver) = self.receivers.next_waiting(rank) {
                receiver.msg = Some((msg, prio));
                receiver.done = true;
                return Sent::HandedOff;
            }
        }
        match self.msgs.push(prio, msg) {
            Ok(()) => {
                let notify = if self.msgs.len() == 1 { self.notify.take() } else { None };
                let served = self.serve(rank).0;
                Sent::Queued { notify, served }
            }
            Err(Refused::Full(msg)) => Sent::Full(msg),
            Err(Refused::NoMemory(_)) => Sent::NoMemory,
        }
    }

    /// Hand queued messages to waiting receivers, best message to the
    /// receiver served next, while both remain: whether a receiver and
    /// whether a sender (whose message took the room) was served.
    pub fn serve(&mut self, rank: &dyn Fn(u64) -> u8) -> (bool, bool) {
        let (mut receivers, mut senders) = (false, false);
        while !self.msgs.is_empty() {
            let Some(id) = self.receivers.next_waiting(rank).map(|w| w.id) else { break };
            let Some((taken, sender)) = self.receive(rank) else { break };
            if let Some(receiver) = self.receivers.get_mut(id) {
                receiver.msg = Some(taken);
                receiver.done = true;
            }
            receivers = true;
            senders |= sender;
        }
        (receivers, senders)
    }

    /// Take the next message, and queue the next waiting sender's message in
    /// the room that leaves: the message, and whether a sender was served.
    /// The heap keeps the place the pop freed, so that push needs no memory.
    pub fn receive(&mut self, rank: &dyn Fn(u64) -> u8) -> Option<((Vec<u8>, u32), bool)> {
        let taken = self.msgs.pop()?;
        let mut served = false;
        if let Some(sender) = self.senders.next_waiting(rank) {
            if let Some((msg, prio)) = sender.msg.take() {
                match self.msgs.push(prio, msg) {
                    Ok(()) => {
                        sender.done = true;
                        served = true;
                    }
                    Err(Refused::Full(msg) | Refused::NoMemory(msg)) => sender.msg = Some((msg, prio)),
                }
            }
        }
        Some((taken, served))
    }
}

/// The queues that exist and what each real user is charged for them.
#[derive(Debug, Default)]
pub struct Usage {
    queues: usize,
    by_user: Vec<(u32, u64)>,
}

impl Usage {
    pub const fn new() -> Self {
        Self { queues: 0, by_user: Vec::new() }
    }

    /// The queues that exist.
    pub fn queues(&self) -> usize {
        self.queues
    }

    /// What real user `uid` is charged.
    pub fn charged(&self, uid: u32) -> u64 {
        self.by_user.iter().find(|e| e.0 == uid).map_or(0, |e| e.1)
    }

    /// Charge a new queue of `bytes` to real user `uid` (Linux's order):
    /// ENOSPC when QUEUES_MAX queues exist and the caller is not
    /// `privileged`, EMFILE when the user's charge would pass `limit`
    /// (RLIMIT_MSGQUEUE, which privilege does not lift), ENOMEM with no
    /// memory for the user's entry.
    pub fn charge(&mut self, uid: u32, bytes: u64, limit: u64, privileged: bool) -> Result<(), u64> {
        if self.queues >= QUEUES_MAX && !privileged {
            return Err(ENOSPC as u64);
        }
        let at = self.by_user.iter().position(|e| e.0 == uid);
        let now = at.map_or(0, |i| self.by_user[i].1);
        match now.checked_add(bytes) {
            Some(total) if total <= limit => {}
            _ => return Err(EMFILE as u64),
        }
        match at {
            Some(i) => self.by_user[i].1 += bytes,
            None => {
                self.by_user.try_reserve(1).map_err(|_| ENOMEM as u64)?;
                self.by_user.push((uid, bytes));
            }
        }
        self.queues += 1;
        Ok(())
    }

    /// Release a queue `charge` placed.
    pub fn release(&mut self, uid: u32, bytes: u64) {
        self.queues = self.queues.saturating_sub(1);
        if let Some(i) = self.by_user.iter().position(|e| e.0 == uid) {
            self.by_user[i].1 = self.by_user[i].1.saturating_sub(bytes);
            if self.by_user[i].1 == 0 {
                self.by_user.swap_remove(i);
            }
        }
    }
}

/// The queue names: byte strings without the leading slash, kept sorted so
/// a lookup is a binary search. Insertion is fallible.
#[derive(Debug)]
pub struct NameTable<T> {
    entries: Vec<(Vec<u8>, T)>,
}

impl<T> NameTable<T> {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    fn find(&self, name: &[u8]) -> Result<usize, usize> {
        self.entries.binary_search_by(|e| e.0.as_slice().cmp(name))
    }

    pub fn get(&self, name: &[u8]) -> Option<&T> {
        self.find(name).ok().map(|i| &self.entries[i].1)
    }

    /// Insert `value` under a name not yet present; ENOMEM with no memory
    /// for the name or its entry, which gives `value` back.
    pub fn insert(&mut self, name: &[u8], value: T) -> Result<(), (u64, T)> {
        let Err(at) = self.find(name) else {
            return Err((EINVAL as u64, value));
        };
        let mut key = Vec::new();
        if key.try_reserve_exact(name.len()).is_err() || self.entries.try_reserve(1).is_err() {
            return Err((ENOMEM as u64, value));
        }
        key.extend_from_slice(name);
        self.entries.insert(at, (key, value));
        Ok(())
    }

    pub fn remove(&mut self, name: &[u8]) -> Option<T> {
        self.find(name).ok().map(|i| self.entries.remove(i).1)
    }

    /// The names and their values, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &T)> {
        self.entries.iter().map(|(name, value)| (name.as_slice(), value))
    }
}
