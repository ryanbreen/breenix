//! System V IPC state: keys, identifiers and permissions, message queues,
//! semaphore sets and their undo adjustments, and the Linux ABI's structures.
//!
//! Nothing in this file locks, sleeps, reads a clock or touches user memory;
//! `msg`, `sem` and `shm` do that around it. Everything here that grows
//! does so fallibly and reports ENOMEM, since a user's calls drive it. The
//! host tests in tests/sysv_ipc_state.rs build this file on its own.

use crate::syscall::errno::{E2BIG, EEXIST, EINVAL, ENOENT, ENOMEM, ENOSPC, ERANGE};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const IPC_PRIVATE: i32 = 0;
pub const IPC_CREAT: u32 = 0o1000;
pub const IPC_EXCL: u32 = 0o2000;
pub const IPC_NOWAIT: u32 = 0o4000;

pub const IPC_RMID: u32 = 0;
pub const IPC_SET: u32 = 1;
pub const IPC_STAT: u32 = 2;
/// Linux's control command for a class's limits.
pub const IPC_INFO: u32 = 3;
/// Linux's request for the 64-bit structures, the only ones Breenix has.
pub const IPC_64: u32 = 0x100;

pub const MSG_NOERROR: u32 = 0o10000;
/// Linux's msgrcv flags: the first message not of the type, and a copy of
/// the message at a position, left queued.
pub const MSG_EXCEPT: u32 = 0o20000;
pub const MSG_COPY: u32 = 0o40000;

/// Linux's msgctl commands beyond POSIX's.
pub const MSG_STAT: u32 = 11;
pub const MSG_INFO: u32 = 12;
pub const MSG_STAT_ANY: u32 = 13;

pub const GETPID: u32 = 11;
pub const GETVAL: u32 = 12;
pub const GETALL: u32 = 13;
pub const GETNCNT: u32 = 14;
pub const GETZCNT: u32 = 15;
pub const SETVAL: u32 = 16;
pub const SETALL: u32 = 17;
pub const SEM_UNDO: i16 = 0x1000;
/// Linux's semctl commands beyond POSIX's.
pub const SEM_STAT: u32 = 18;
pub const SEM_INFO: u32 = 19;
pub const SEM_STAT_ANY: u32 = 20;

pub const SHM_RDONLY: u32 = 0o10000;
pub const SHM_RND: u32 = 0o20000;
pub const SHM_EXEC: u32 = 0o100000;
/// The mode bit that marks a segment removed while still attached.
pub const SHM_DEST: u32 = 0o1000;
/// The mode bit that marks a segment SHM_LOCK locked.
pub const SHM_LOCKED: u32 = 0o2000;
/// Linux's shmat flag: attach over whatever is mapped at the address.
pub const SHM_REMAP: u32 = 0o40000;

/// Linux's shmctl commands beyond POSIX's.
pub const SHM_LOCK: u32 = 11;
pub const SHM_UNLOCK: u32 = 12;
pub const SHM_STAT: u32 = 13;
pub const SHM_INFO: u32 = 14;
pub const SHM_STAT_ANY: u32 = 15;

/// Linux's limits.
pub const MSGMAX: usize = 8192;
pub const MSGMNB: usize = 16384;
pub const MSGMNI: usize = 32000;
pub const SEMMSL: usize = 32000;
pub const SEMMNI: usize = 32000;
pub const SEMOPM: usize = 500;
pub const SEMVMX: i32 = 32767;
/// Linux's derived semaphore limits, which IPC_INFO reports.
pub const SEMMNS: usize = SEMMNI * SEMMSL;
pub const SEMUSZ: usize = 20;
/// Linux's message pool figures, which IPC_INFO reports.
pub const MSGPOOL: usize = MSGMNI * MSGMNB / 1024;
pub const MSGSSZ: usize = 16;
pub const MSGSEG: u16 = 0xffff;
pub const SHMMNI: usize = 4096;
pub const SHMMIN: u64 = 1;
pub const SHMMAX: u64 = u64::MAX - (1 << 24);
pub const SHMLBA: u64 = 4096;
/// Linux's limit on the pages of all segments, which IPC_INFO reports.
pub const SHMALL: u64 = u64::MAX - (1 << 24);

/// Permission bits, as in a mode's low three.
pub const READ: u32 = 4;
pub const WRITE: u32 = 2;

/// An identifier is `seq * SEQ_MULTIPLIER + index`, as on Linux.
const SEQ_MULTIPLIER: i32 = 32768;

/// A vector of `len` copies of `value`, or ENOMEM.
pub fn try_vec<T: Clone>(len: usize, value: T) -> Result<Vec<T>, i32> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    v.resize(len, value);
    Ok(v)
}

/// A copy of `bytes`, or ENOMEM.
pub fn try_copy(bytes: &[u8]) -> Result<Vec<u8>, i32> {
    let mut v = Vec::new();
    v.try_reserve_exact(bytes.len()).map_err(|_| ENOMEM)?;
    v.extend_from_slice(bytes);
    Ok(v)
}

/// Push `item` onto `v`, or ENOMEM leaving `v` as it was.
pub fn try_push<T>(v: &mut Vec<T>, item: T) -> Result<(), i32> {
    v.try_reserve(1).map_err(|_| ENOMEM)?;
    v.push(item);
    Ok(())
}

/// A map kept as a vector sorted by key, so that adding to it can fail with
/// ENOMEM where a BTreeMap would panic.
pub struct VecMap<K, V> {
    items: Vec<(K, V)>,
}

impl<K: Ord + Copy, V> VecMap<K, V> {
    pub const fn new() -> Self {
        VecMap { items: Vec::new() }
    }

    fn find(&self, key: K) -> Result<usize, usize> {
        self.items.binary_search_by(|(k, _)| k.cmp(&key))
    }

    pub fn get(&self, key: K) -> Option<&V> {
        self.find(key).ok().map(|at| &self.items[at].1)
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.find(key).ok().map(|at| &mut self.items[at].1)
    }

    /// Map `key` to `value`, replacing any value it had; ENOMEM changes
    /// nothing.
    pub fn insert(&mut self, key: K, value: V) -> Result<(), i32> {
        match self.find(key) {
            Ok(at) => self.items[at].1 = value,
            Err(at) => {
                self.items.try_reserve(1).map_err(|_| ENOMEM)?;
                self.items.insert(at, (key, value));
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        self.find(key).ok().map(|at| self.items.remove(at).1)
    }

    /// Remove and return the first entry whose key `pick` accepts.
    pub fn take_first(&mut self, pick: impl Fn(&K) -> bool) -> Option<(K, V)> {
        let at = self.items.iter().position(|(k, _)| pick(k))?;
        Some(self.items.remove(at))
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        self.items.retain_mut(|(k, v)| keep(k, v));
    }

    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (K, V)> {
        self.items.iter_mut()
    }
}

impl<K: Ord + Copy, V> Default for VecMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// struct ipc_perm, less its layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcPerm {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u32,
    pub seq: u16,
}

impl IpcPerm {
    /// The permission record of an object a process with effective ids
    /// `euid` and `egid` creates with the low nine bits of `flags`.
    pub fn new(key: i32, euid: u32, egid: u32, flags: u32) -> Self {
        IpcPerm { key, uid: euid, gid: egid, cuid: euid, cgid: egid, mode: flags & 0o777, seq: 0 }
    }

    /// Whether a process with effective user `euid`, and group membership
    /// `in_group`, is granted every bit of `wanted` (READ, WRITE). The owner
    /// class applies to the owner and the creator, the group class to the
    /// owner's and the creator's groups; an effective user of 0 is always
    /// granted, as Linux grants CAP_IPC_OWNER.
    pub fn permits(&self, euid: u32, in_group: impl Fn(u32) -> bool, wanted: u32) -> bool {
        if euid == 0 {
            return true;
        }
        let granted = if euid == self.uid || euid == self.cuid {
            self.mode >> 6
        } else if in_group(self.gid) || in_group(self.cgid) {
            self.mode >> 3
        } else {
            self.mode
        } & 7;
        granted & wanted == wanted
    }

    /// Whether `euid` may change or remove the object: the owner, the
    /// creator, or an effective user of 0.
    pub fn owned_by(&self, euid: u32) -> bool {
        euid == 0 || euid == self.uid || euid == self.cuid
    }

    /// IPC_SET: the new owner, group and the low nine mode bits. EINVAL,
    /// changing nothing, for a uid or gid of -1, which names no one, as on
    /// Linux.
    pub fn set(&mut self, uid: u32, gid: u32, mode: u32) -> Result<(), i32> {
        if uid == u32::MAX || gid == u32::MAX {
            return Err(EINVAL);
        }
        self.uid = uid;
        self.gid = gid;
        self.mode = (self.mode & !0o777) | (mode & 0o777);
        Ok(())
    }
}

/// The access an xxxget of an existing key asks for: every class's bits of
/// `flags` folded together, as Linux checks them.
pub fn requested(flags: u32) -> u32 {
    ((flags >> 6) | (flags >> 3) | flags) & 7
}

/// One object of a class and its permission record.
pub struct Entry<T> {
    pub perm: IpcPerm,
    pub obj: T,
}

/// One class's objects by identifier, and the keys that name them.
pub struct IpcIds<T> {
    slots: Vec<Option<Entry<T>>>,
    keys: VecMap<i32, usize>,
    seq: u16,
    limit: usize,
}

impl<T> IpcIds<T> {
    pub const fn new(limit: usize) -> Self {
        IpcIds { slots: Vec::new(), keys: VecMap::new(), seq: 0, limit }
    }

    /// What an xxxget of `key` with `flags` finds: Some(id) of an existing
    /// object, None when it is to create one, or the errno.
    pub fn resolve(&self, key: i32, flags: u32) -> Result<Option<i32>, i32> {
        if key == IPC_PRIVATE {
            return Ok(None);
        }
        match self.keys.get(key) {
            None if flags & IPC_CREAT != 0 => Ok(None),
            None => Err(ENOENT),
            Some(_) if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 => Err(EEXIST),
            Some(&index) => Ok(Some(self.id_of(index))),
        }
    }

    /// Add an object: its new identifier, ENOSPC when the class is full or
    /// ENOMEM. A key other than IPC_PRIVATE then names it.
    pub fn insert(&mut self, mut perm: IpcPerm, obj: T) -> Result<i32, i32> {
        let index = match self.slots.iter().position(Option::is_none) {
            Some(index) => index,
            None if self.slots.len() < self.limit => {
                try_push(&mut self.slots, None)?;
                self.slots.len() - 1
            }
            None => return Err(ENOSPC),
        };
        if perm.key != IPC_PRIVATE {
            self.keys.insert(perm.key, index)?;
        }
        perm.seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.slots[index] = Some(Entry { perm, obj });
        Ok(self.id_of(index))
    }

    pub fn get(&self, id: i32) -> Option<&Entry<T>> {
        let index = self.index(id)?;
        self.slots[index].as_ref()
    }

    pub fn get_mut(&mut self, id: i32) -> Option<&mut Entry<T>> {
        let index = self.index(id)?;
        self.slots[index].as_mut()
    }

    /// Remove the object and its key.
    pub fn remove(&mut self, id: i32) -> Option<Entry<T>> {
        let index = self.index(id)?;
        let entry = self.slots[index].take()?;
        if self.keys.get(entry.perm.key) == Some(&index) {
            self.keys.remove(entry.perm.key);
        }
        Some(entry)
    }

    /// Stop the key naming the object, which its identifier still reaches;
    /// its key then reads as IPC_PRIVATE.
    pub fn forget_key(&mut self, id: i32) {
        let Some(index) = self.index(id) else { return };
        let Some(entry) = self.slots[index].as_mut() else { return };
        if self.keys.get(entry.perm.key) == Some(&index) {
            self.keys.remove(entry.perm.key);
        }
        entry.perm.key = IPC_PRIVATE;
    }

    /// Every object, with its identifier.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (i32, &mut Entry<T>)> {
        self.slots.iter_mut().enumerate().filter_map(|(index, slot)| {
            let entry = slot.as_mut()?;
            Some((i32::from(entry.perm.seq) * SEQ_MULTIPLIER + index as i32, entry))
        })
    }

    /// Every object, with its identifier.
    pub fn iter(&self) -> impl Iterator<Item = (i32, &Entry<T>)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let entry = slot.as_ref()?;
            Some((i32::from(entry.perm.seq) * SEQ_MULTIPLIER + index as i32, entry))
        })
    }

    /// How many objects the class holds.
    pub fn in_use(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// The highest index an object holds, or 0 when there is none: what
    /// Linux's IPC_INFO and xxx_INFO commands return.
    pub fn highest_index(&self) -> usize {
        self.slots.iter().rposition(Option::is_some).unwrap_or(0)
    }

    /// The highest index an object `keep` accepts holds, or 0.
    pub fn highest_index_where(&self, keep: impl Fn(&Entry<T>) -> bool) -> usize {
        self.slots.iter().rposition(|slot| slot.as_ref().is_some_and(&keep)).unwrap_or(0)
    }

    /// The object at index `index`, as Linux's xxx_STAT commands take it
    /// (the index bits of an identifier), with its identifier.
    pub fn at_index(&self, index: i32) -> Option<(i32, &Entry<T>)> {
        if index < 0 {
            return None;
        }
        let index = (index % SEQ_MULTIPLIER) as usize;
        let entry = self.slots.get(index)?.as_ref()?;
        Some((self.id_of(index), entry))
    }

    fn id_of(&self, index: usize) -> i32 {
        let seq = self.slots[index].as_ref().map_or(0, |entry| entry.perm.seq);
        i32::from(seq) * SEQ_MULTIPLIER + index as i32
    }

    fn index(&self, id: i32) -> Option<usize> {
        if id < 0 {
            return None;
        }
        let index = (id % SEQ_MULTIPLIER) as usize;
        let seq = id / SEQ_MULTIPLIER;
        let entry = self.slots.get(index)?.as_ref()?;
        (i32::from(entry.perm.seq) == seq).then_some(index)
    }
}

// ---------------------------------------------------------------------------
// Message queues.

pub struct Message {
    pub mtype: i64,
    pub text: Vec<u8>,
}

/// A message queue's messages and the counts struct msqid_ds reports.
pub struct MsgQueue {
    msgs: VecDeque<Message>,
    pub cbytes: usize,
    pub qbytes: usize,
    pub lspid: i32,
    pub lrpid: i32,
    pub stime: i64,
    pub rtime: i64,
    pub ctime: i64,
}

impl MsgQueue {
    pub fn new(now: i64) -> Self {
        MsgQueue { msgs: VecDeque::new(), cbytes: 0, qbytes: MSGMNB, lspid: 0, lrpid: 0, stime: 0, rtime: 0, ctime: now }
    }

    pub fn qnum(&self) -> usize {
        self.msgs.len()
    }

    /// Whether a message of `len` bytes fits: within msg_qbytes, and, as
    /// Linux also requires, with no more messages than msg_qbytes.
    pub fn has_room(&self, len: usize) -> bool {
        self.cbytes + len <= self.qbytes && self.msgs.len() < self.qbytes
    }

    /// Queue `msg` as sent by `pid`; ENOMEM, queueing nothing, when the
    /// queue cannot grow.
    pub fn push(&mut self, msg: Message, pid: i32, now: i64) -> Result<(), i32> {
        self.msgs.try_reserve(1).map_err(|_| ENOMEM)?;
        self.cbytes += msg.text.len();
        self.msgs.push_back(msg);
        self.lspid = pid;
        self.stime = now;
        Ok(())
    }

    /// The message `select` picks.
    pub fn find(&self, select: Select) -> Option<usize> {
        match select {
            Select::Type(0) => (!self.msgs.is_empty()).then_some(0),
            Select::Type(t) if t > 0 => self.msgs.iter().position(|m| m.mtype == t),
            Select::Type(t) => {
                let limit = t.unsigned_abs();
                let mut best: Option<(usize, i64)> = None;
                for (i, m) in self.msgs.iter().enumerate() {
                    if m.mtype.unsigned_abs() <= limit && best.is_none_or(|(_, low)| m.mtype < low) {
                        best = Some((i, m.mtype));
                    }
                }
                best.map(|(i, _)| i)
            }
            Select::Except(t) => self.msgs.iter().position(|m| m.mtype != t),
            Select::Copy(n) => usize::try_from(n).ok().filter(|&n| n < self.msgs.len()),
        }
    }

    /// msgrcv into `size` bytes: the message `select` picks, cut to `size`
    /// when `noerror`; None when no message matches; E2BIG, leaving it
    /// queued, when it is longer than `size` without `noerror`. Select::Copy
    /// leaves the message queued and the queue's times and pids unchanged,
    /// can fail with ENOMEM, and never truncates: a message longer than
    /// `size` is EINVAL even with `noerror`, as Linux's copy_msg gives it.
    pub fn receive(&mut self, select: Select, size: usize, noerror: bool, pid: i32, now: i64) -> Result<Option<Message>, i32> {
        let Some(index) = self.find(select) else { return Ok(None) };
        let len = self.msgs[index].text.len();
        if len > size && !noerror {
            return Err(E2BIG);
        }
        if let Select::Copy(_) = select {
            if len > size {
                return Err(EINVAL);
            }
            let m = &self.msgs[index];
            let text = try_copy(&m.text)?;
            return Ok(Some(Message { mtype: m.mtype, text }));
        }
        let Some(mut msg) = self.msgs.remove(index) else { return Ok(None) };
        self.cbytes -= msg.text.len();
        msg.text.truncate(size);
        self.lrpid = pid;
        self.rtime = now;
        Ok(Some(msg))
    }
}

/// Which message msgrcv takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// msgtyp's meaning: for 0 the first message; for a positive type the
    /// first of that type; for a negative one the first of the lowest type
    /// no greater than its absolute value.
    Type(i64),
    /// MSG_EXCEPT with a positive type: the first message of another type.
    Except(i64),
    /// MSG_COPY: a copy of the message at this position, counting from 0.
    Copy(i64),
}

/// What msgrcv's `msgtyp` and `flags` select, as Linux reads them:
/// MSG_COPY takes a position and needs IPC_NOWAIT and no MSG_EXCEPT (EINVAL
/// otherwise); MSG_EXCEPT applies to a positive type only.
pub fn select(msgtyp: i64, flags: u32) -> Result<Select, i32> {
    if flags & MSG_COPY != 0 {
        if flags & MSG_EXCEPT != 0 || flags & IPC_NOWAIT == 0 {
            return Err(EINVAL);
        }
        return Ok(Select::Copy(msgtyp));
    }
    if msgtyp > 0 && flags & MSG_EXCEPT != 0 {
        return Ok(Select::Except(msgtyp));
    }
    Ok(Select::Type(msgtyp))
}

// ---------------------------------------------------------------------------
// Semaphore sets.

/// struct sembuf.
#[derive(Clone, Copy, Debug)]
pub struct SemOp {
    pub num: u16,
    pub op: i16,
    pub flg: i16,
}

impl SemOp {
    pub fn from_bytes(b: [u8; 6]) -> Self {
        SemOp {
            num: u16::from_ne_bytes([b[0], b[1]]),
            op: i16::from_ne_bytes([b[2], b[3]]),
            flg: i16::from_ne_bytes([b[4], b[5]]),
        }
    }
}

/// A process waiting in semop: the thread, and the semaphore and kind of
/// operation (a decrement, or a wait for zero) that made it wait.
#[derive(Clone, Copy, Debug)]
pub struct SemWaiter {
    pub tid: u64,
    pub num: u16,
    pub zero: bool,
}

/// What a semop attempt did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// Every operation was applied.
    Done,
    /// None was applied: operation `num` would wait, for zero or for a
    /// value to take from; `nowait` when that operation has IPC_NOWAIT.
    Block { num: u16, zero: bool, nowait: bool },
}

pub struct SemSet {
    pub vals: Vec<u16>,
    pub pids: Vec<i32>,
    pub otime: i64,
    pub ctime: i64,
    pub waiters: Vec<SemWaiter>,
}

impl SemSet {
    /// A set of `nsems` semaphores, all 0, or ENOMEM.
    pub fn try_new(nsems: usize, now: i64) -> Result<Self, i32> {
        Ok(SemSet { vals: try_vec(nsems, 0)?, pids: try_vec(nsems, 0)?, otime: 0, ctime: now, waiters: Vec::new() })
    }

    pub fn nsems(&self) -> usize {
        self.vals.len()
    }

    /// Apply all of `ops` or none of them, in order, as process `pid`.
    /// `adj` is the caller's undo adjustments for this set (empty when no
    /// operation has SEM_UNDO), changed only when every operation applies.
    /// ERANGE when a value would pass SEMVMX or an adjustment leave i16's
    /// range; ENOMEM. Every `num` is below nsems.
    pub fn attempt(&mut self, ops: &[SemOp], adj: &mut [i16], pid: i32, now: i64) -> Result<Attempt, i32> {
        // The semaphores the operations touch: number, value and adjustment
        // as the operations so far leave them.
        let mut touched: Vec<(usize, i32, i32)> = Vec::new();
        touched.try_reserve_exact(ops.len()).map_err(|_| ENOMEM)?;
        for op in ops {
            let num = usize::from(op.num);
            let at = match touched.iter().position(|t| t.0 == num) {
                Some(at) => at,
                None => {
                    let undo = adj.get(num).map_or(0, |&a| i32::from(a));
                    touched.push((num, i32::from(self.vals[num]), undo));
                    touched.len() - 1
                }
            };
            let (_, val, undo) = &mut touched[at];
            let wait = if op.op == 0 { *val != 0 } else { *val + i32::from(op.op) < 0 };
            if wait {
                let nowait = op.flg as u16 as u32 & IPC_NOWAIT != 0;
                return Ok(Attempt::Block { num: op.num, zero: op.op == 0, nowait });
            }
            let next = *val + i32::from(op.op);
            if next > SEMVMX {
                return Err(ERANGE);
            }
            if op.flg & SEM_UNDO != 0 && op.op != 0 {
                let next_undo = *undo - i32::from(op.op);
                if next_undo < i32::from(i16::MIN) || next_undo > i32::from(i16::MAX) {
                    return Err(ERANGE);
                }
                *undo = next_undo;
            }
            *val = next;
        }
        for &(num, val, undo) in &touched {
            self.vals[num] = val as u16;
            if let Some(slot) = adj.get_mut(num) {
                *slot = undo as i16;
            }
        }
        for op in ops {
            self.pids[usize::from(op.num)] = pid;
        }
        self.otime = now;
        Ok(Attempt::Done)
    }

    /// GETNCNT and GETZCNT of semaphore `num`.
    pub fn counts(&self, num: usize) -> (usize, usize) {
        let mine = || self.waiters.iter().filter(move |w| usize::from(w.num) == num);
        (mine().filter(|w| !w.zero).count(), mine().filter(|w| w.zero).count())
    }

    /// SETVAL of a semaphore in the set; `in_range(val)` was checked first.
    pub fn set_val(&mut self, num: usize, val: i32, pid: i32, now: i64) {
        self.vals[num] = val as u16;
        self.pids[num] = pid;
        self.ctime = now;
    }

    /// SETALL: ERANGE, changing nothing, if any value passes SEMVMX.
    pub fn set_all(&mut self, vals: &[u16], pid: i32, now: i64) -> Result<(), i32> {
        if vals.len() != self.vals.len() {
            return Err(EINVAL);
        }
        if vals.iter().any(|&v| i32::from(v) > SEMVMX) {
            return Err(ERANGE);
        }
        self.vals.copy_from_slice(vals);
        for p in self.pids.iter_mut() {
            *p = pid;
        }
        self.ctime = now;
        Ok(())
    }

    /// An exiting process's undo adjustments: each value moves by its
    /// adjustment, held to 0..=SEMVMX, and sem_otime becomes `now`, as on
    /// Linux. Whether any value changed.
    pub fn apply_undo(&mut self, adj: &[i16], pid: i32, now: i64) -> bool {
        let mut changed = false;
        let nsems = self.vals.len();
        for (num, &a) in adj.iter().enumerate().filter(|&(num, &a)| a != 0 && num < nsems) {
            let next = (i32::from(self.vals[num]) + i32::from(a)).clamp(0, SEMVMX);
            self.vals[num] = next as u16;
            self.pids[num] = pid;
            changed = true;
        }
        self.otime = now;
        changed
    }
}

/// Whether SETVAL accepts `val`: 0..=SEMVMX.
pub fn in_range(val: i32) -> bool {
    (0..=SEMVMX).contains(&val)
}

/// The SEM_UNDO adjustments, as Linux keeps them: each task (a row, one
/// thread) refers to at most one undo list, which tasks made by clone with
/// CLONE_SYSVSEM share and fork and exec leave as they are (a child of fork
/// starts with none). A list holds an adjustment per semaphore of each set
/// it has touched; the task that drops the last reference to it applies it.
pub struct UndoLists {
    /// The list each task refers to, by row.
    tasks: VecMap<u64, u64>,
    /// The adjustments by list and set.
    adj: VecMap<(u64, i32), Vec<i16>>,
    next: u64,
}

impl UndoLists {
    pub const fn new() -> Self {
        UndoLists { tasks: VecMap::new(), adj: VecMap::new(), next: 1 }
    }

    /// The list task `row` refers to, made for it if it has none.
    pub fn join(&mut self, row: u64) -> Result<u64, i32> {
        if let Some(&list) = self.tasks.get(row) {
            return Ok(list);
        }
        let list = self.next;
        self.tasks.insert(row, list)?;
        self.next += 1;
        Ok(list)
    }

    /// clone with CLONE_SYSVSEM: task `child` shares `parent`'s list, made
    /// for the parent if it has none.
    pub fn share(&mut self, parent: u64, child: u64) -> Result<(), i32> {
        let list = self.join(parent)?;
        self.tasks.insert(child, list)
    }

    /// The adjustments list `list` holds for set `semid` of `nsems`
    /// semaphores, made all zero if it has none.
    pub fn adjustments(&mut self, list: u64, semid: i32, nsems: usize) -> Result<&mut Vec<i16>, i32> {
        if self.adj.get((list, semid)).is_none() {
            self.adj.insert((list, semid), try_vec(nsems, 0)?)?;
        }
        self.adj.get_mut((list, semid)).ok_or(ENOMEM)
    }

    /// Task `row` drops its reference: the list, when no other task refers
    /// to it, whose adjustments are now to be applied (`take`).
    pub fn leave(&mut self, row: u64) -> Option<u64> {
        let list = self.tasks.remove(row)?;
        (!self.tasks.iter().any(|&(_, l)| l == list)).then_some(list)
    }

    /// One set's adjustments of list `list`, which no task refers to any
    /// more, taken from it; None once there are none left.
    pub fn take(&mut self, list: u64) -> Option<(i32, Vec<i16>)> {
        self.adj.take_first(|&(l, _)| l == list).map(|((_, semid), adj)| (semid, adj))
    }

    /// IPC_RMID: drop every list's adjustments for `semid`.
    pub fn drop_set(&mut self, semid: i32) {
        self.adj.retain(|&(_, id), _| id != semid);
    }

    /// SETVAL and SETALL: clear every list's adjustment of semaphore `num`
    /// of `semid`, or of all its semaphores.
    pub fn clear(&mut self, semid: i32, num: Option<usize>) {
        for (_, adj) in self.adj.iter_mut().filter(|((_, id), _)| *id == semid) {
            match num {
                Some(n) => {
                    if let Some(a) = adj.get_mut(n) {
                        *a = 0;
                    }
                }
                None => adj.iter_mut().for_each(|a| *a = 0),
            }
        }
    }
}

impl Default for UndoLists {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// The Linux ABI's structures, 64-bit layouts.

pub const IPC_PERM_SIZE: usize = 48;
pub const MSQID_DS_SIZE: usize = 120;
pub const SHMID_DS_SIZE: usize = 112;
pub const MSGINFO_SIZE: usize = 32;
pub const SEMINFO_SIZE: usize = 40;
pub const SHMINFO_SIZE: usize = 72;
pub const SHM_INFO_SIZE: usize = 48;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// struct msginfo. IPC_INFO reports the limits (`usage` None); MSG_INFO
/// reports in msgpool, msgmap and msgtql the queues, messages and bytes in
/// use, as Linux does.
pub fn msginfo(usage: Option<(usize, usize, usize)>) -> [u8; MSGINFO_SIZE] {
    let mut buf = [0u8; MSGINFO_SIZE];
    let (pool, map, tql) = usage.unwrap_or((MSGPOOL, MSGMNB, MSGMNB));
    for (at, v) in [(0, pool), (4, map), (8, MSGMAX), (12, MSGMNB), (16, MSGMNI), (20, MSGSSZ), (24, tql)] {
        put(&mut buf, at, &int(v).to_ne_bytes());
    }
    put(&mut buf, 28, &MSGSEG.to_ne_bytes());
    buf
}

/// struct seminfo. IPC_INFO reports the limits (`usage` None); SEM_INFO
/// reports in semusz and semaem the sets and semaphores in use, as Linux
/// does.
pub fn seminfo(usage: Option<(usize, usize)>) -> [u8; SEMINFO_SIZE] {
    let mut buf = [0u8; SEMINFO_SIZE];
    let (usz, aem) = usage.unwrap_or((SEMUSZ, SEMVMX as usize));
    let fields = [SEMMNS, SEMMNI, SEMMNS, SEMMNS, SEMMSL, SEMOPM, SEMOPM, usz, SEMVMX as usize, aem];
    for (i, v) in fields.into_iter().enumerate() {
        put(&mut buf, i * 4, &int(v).to_ne_bytes());
    }
    buf
}

/// struct semid64_ds differs by architecture: x86-64 follows each time with
/// a padding word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemLayout {
    X86_64,
    Aarch64,
}

impl SemLayout {
    #[cfg(target_arch = "x86_64")]
    pub const NATIVE: SemLayout = SemLayout::X86_64;
    #[cfg(not(target_arch = "x86_64"))]
    pub const NATIVE: SemLayout = SemLayout::Aarch64;

    pub fn size(self) -> usize {
        match self {
            SemLayout::X86_64 => 104,
            SemLayout::Aarch64 => 88,
        }
    }
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn put_perm(buf: &mut [u8], perm: &IpcPerm) {
    put(buf, 0, &perm.key.to_ne_bytes());
    put(buf, 4, &perm.uid.to_ne_bytes());
    put(buf, 8, &perm.gid.to_ne_bytes());
    put(buf, 12, &perm.cuid.to_ne_bytes());
    put(buf, 16, &perm.cgid.to_ne_bytes());
    put(buf, 20, &perm.mode.to_ne_bytes());
    put(buf, 24, &perm.seq.to_ne_bytes());
}

/// What IPC_SET takes from a struct's leading ipc64_perm: uid, gid, mode.
pub fn perm_set_fields(buf: &[u8]) -> (u32, u32, u32) {
    (get_u32(buf, 4), get_u32(buf, 8), get_u32(buf, 20))
}

/// msg_qbytes of a struct msqid64_ds.
pub fn msqid_qbytes(buf: &[u8; MSQID_DS_SIZE]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[88..96]);
    u64::from_ne_bytes(b)
}

pub fn msqid_ds(perm: &IpcPerm, q: &MsgQueue) -> [u8; MSQID_DS_SIZE] {
    let mut buf = [0u8; MSQID_DS_SIZE];
    put_perm(&mut buf, perm);
    put(&mut buf, 48, &q.stime.to_ne_bytes());
    put(&mut buf, 56, &q.rtime.to_ne_bytes());
    put(&mut buf, 64, &q.ctime.to_ne_bytes());
    put(&mut buf, 72, &(q.cbytes as u64).to_ne_bytes());
    put(&mut buf, 80, &(q.qnum() as u64).to_ne_bytes());
    put(&mut buf, 88, &(q.qbytes as u64).to_ne_bytes());
    put(&mut buf, 96, &q.lspid.to_ne_bytes());
    put(&mut buf, 100, &q.lrpid.to_ne_bytes());
    buf
}

/// struct semid64_ds in `layout`, in the first `layout.size()` bytes.
pub fn semid_ds(perm: &IpcPerm, set: &SemSet, layout: SemLayout) -> [u8; 104] {
    let mut buf = [0u8; 104];
    put_perm(&mut buf, perm);
    let (ctime_at, nsems_at) = match layout {
        SemLayout::X86_64 => (64, 80),
        SemLayout::Aarch64 => (56, 64),
    };
    put(&mut buf, 48, &set.otime.to_ne_bytes());
    put(&mut buf, ctime_at, &set.ctime.to_ne_bytes());
    put(&mut buf, nsems_at, &(set.nsems() as u64).to_ne_bytes());
    buf
}

/// What struct shmid64_ds reports of a segment.
pub struct ShmStat {
    pub segsz: u64,
    pub atime: i64,
    pub dtime: i64,
    pub ctime: i64,
    pub cpid: i32,
    pub lpid: i32,
    pub nattch: u64,
}

/// struct shminfo64, IPC_INFO's limits for shared memory.
pub fn shminfo() -> [u8; SHMINFO_SIZE] {
    let mut buf = [0u8; SHMINFO_SIZE];
    for (at, v) in [(0, SHMMAX), (8, SHMMIN), (16, SHMMNI as u64), (24, SHMMNI as u64), (32, SHMALL)] {
        put(&mut buf, at, &v.to_ne_bytes());
    }
    buf
}

/// struct shm_info, SHM_INFO's use: the segments in use, and their pages
/// allocated and resident (the same here, where a segment's pages are made
/// with it); nothing is swapped.
pub fn shm_info(used_ids: usize, pages: u64) -> [u8; SHM_INFO_SIZE] {
    let mut buf = [0u8; SHM_INFO_SIZE];
    put(&mut buf, 0, &int(used_ids).to_ne_bytes());
    put(&mut buf, 8, &pages.to_ne_bytes());
    put(&mut buf, 16, &pages.to_ne_bytes());
    buf
}

pub fn shmid_ds(perm: &IpcPerm, s: &ShmStat) -> [u8; SHMID_DS_SIZE] {
    let mut buf = [0u8; SHMID_DS_SIZE];
    put_perm(&mut buf, perm);
    put(&mut buf, 48, &s.segsz.to_ne_bytes());
    put(&mut buf, 56, &s.atime.to_ne_bytes());
    put(&mut buf, 64, &s.dtime.to_ne_bytes());
    put(&mut buf, 72, &s.ctime.to_ne_bytes());
    put(&mut buf, 80, &s.cpid.to_ne_bytes());
    put(&mut buf, 84, &s.lpid.to_ne_bytes());
    put(&mut buf, 88, &s.nattch.to_ne_bytes());
    buf
}
