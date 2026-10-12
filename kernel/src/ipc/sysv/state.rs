//! System V IPC state: keys, identifiers and permissions, message queues,
//! semaphore sets and their undo adjustments, and the Linux ABI's structures.
//!
//! Nothing in this file locks, sleeps, reads a clock or touches user memory;
//! `msg`, `sem` and `shm` do that around it. The host tests in
//! tests/sysv_ipc_state.rs build this file on its own.

use crate::syscall::errno::{E2BIG, EEXIST, EINVAL, ENOENT, ENOSPC, ERANGE};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;

pub const IPC_PRIVATE: i32 = 0;
pub const IPC_CREAT: u32 = 0o1000;
pub const IPC_EXCL: u32 = 0o2000;
pub const IPC_NOWAIT: u32 = 0o4000;

pub const IPC_RMID: u32 = 0;
pub const IPC_SET: u32 = 1;
pub const IPC_STAT: u32 = 2;
/// Linux's request for the 64-bit structures, the only ones Breenix has.
pub const IPC_64: u32 = 0x100;

pub const MSG_NOERROR: u32 = 0o10000;

pub const GETPID: u32 = 11;
pub const GETVAL: u32 = 12;
pub const GETALL: u32 = 13;
pub const GETNCNT: u32 = 14;
pub const GETZCNT: u32 = 15;
pub const SETVAL: u32 = 16;
pub const SETALL: u32 = 17;
pub const SEM_UNDO: i16 = 0x1000;

pub const SHM_RDONLY: u32 = 0o10000;
pub const SHM_RND: u32 = 0o20000;
pub const SHM_EXEC: u32 = 0o100000;
/// The mode bit that marks a segment removed while still attached.
pub const SHM_DEST: u32 = 0o1000;

/// Linux's limits.
pub const MSGMAX: usize = 8192;
pub const MSGMNB: usize = 16384;
pub const MSGMNI: usize = 32000;
pub const SEMMSL: usize = 32000;
pub const SEMMNI: usize = 32000;
pub const SEMOPM: usize = 500;
pub const SEMVMX: i32 = 32767;
pub const SHMMNI: usize = 4096;
pub const SHMMIN: u64 = 1;
pub const SHMMAX: u64 = u64::MAX - (1 << 24);
pub const SHMLBA: u64 = 4096;

/// Permission bits, as in a mode's low three.
pub const READ: u32 = 4;
pub const WRITE: u32 = 2;

/// An identifier is `seq * SEQ_MULTIPLIER + index`, as on Linux.
const SEQ_MULTIPLIER: i32 = 32768;

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

    /// IPC_SET: the new owner, group and the low nine mode bits.
    pub fn set(&mut self, uid: u32, gid: u32, mode: u32) {
        self.uid = uid;
        self.gid = gid;
        self.mode = (self.mode & !0o777) | (mode & 0o777);
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
    keys: BTreeMap<i32, usize>,
    seq: u16,
    limit: usize,
}

impl<T> IpcIds<T> {
    pub const fn new(limit: usize) -> Self {
        IpcIds { slots: Vec::new(), keys: BTreeMap::new(), seq: 0, limit }
    }

    /// What an xxxget of `key` with `flags` finds: Some(id) of an existing
    /// object, None when it is to create one, or the errno.
    pub fn resolve(&self, key: i32, flags: u32) -> Result<Option<i32>, i32> {
        if key == IPC_PRIVATE {
            return Ok(None);
        }
        match self.keys.get(&key) {
            None if flags & IPC_CREAT != 0 => Ok(None),
            None => Err(ENOENT),
            Some(_) if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 => Err(EEXIST),
            Some(&index) => Ok(Some(self.id_of(index))),
        }
    }

    /// Add an object: its new identifier, or ENOSPC when the class is full.
    /// A key other than IPC_PRIVATE then names it.
    pub fn insert(&mut self, mut perm: IpcPerm, obj: T) -> Result<i32, i32> {
        let index = match self.slots.iter().position(Option::is_none) {
            Some(index) => index,
            None if self.slots.len() < self.limit => {
                self.slots.push(None);
                self.slots.len() - 1
            }
            None => return Err(ENOSPC),
        };
        perm.seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        if perm.key != IPC_PRIVATE {
            self.keys.insert(perm.key, index);
        }
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
        if self.keys.get(&entry.perm.key) == Some(&index) {
            self.keys.remove(&entry.perm.key);
        }
        Some(entry)
    }

    /// Stop the key naming the object, which its identifier still reaches;
    /// its key then reads as IPC_PRIVATE.
    pub fn forget_key(&mut self, id: i32) {
        let Some(index) = self.index(id) else { return };
        let Some(entry) = self.slots[index].as_mut() else { return };
        if self.keys.get(&entry.perm.key) == Some(&index) {
            self.keys.remove(&entry.perm.key);
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

    pub fn push(&mut self, msg: Message, pid: i32, now: i64) {
        self.cbytes += msg.text.len();
        self.msgs.push_back(msg);
        self.lspid = pid;
        self.stime = now;
    }

    /// The message msgrcv with `msgtyp` takes: for 0 the first; for a
    /// positive type the first of that type; for a negative one the first of
    /// the lowest type no greater than its absolute value.
    pub fn find(&self, msgtyp: i64) -> Option<usize> {
        match msgtyp {
            0 => (!self.msgs.is_empty()).then_some(0),
            t if t > 0 => self.msgs.iter().position(|m| m.mtype == t),
            t => {
                let limit = t.unsigned_abs();
                let mut best: Option<(usize, i64)> = None;
                for (i, m) in self.msgs.iter().enumerate() {
                    if m.mtype.unsigned_abs() <= limit && best.is_none_or(|(_, low)| m.mtype < low) {
                        best = Some((i, m.mtype));
                    }
                }
                best.map(|(i, _)| i)
            }
        }
    }

    /// msgrcv into `size` bytes: the message, cut to `size` when `noerror`;
    /// None when no message matches; E2BIG, leaving it queued, when it is
    /// longer than `size` without `noerror`.
    pub fn receive(&mut self, msgtyp: i64, size: usize, noerror: bool, pid: i32, now: i64) -> Result<Option<Message>, i32> {
        let Some(index) = self.find(msgtyp) else { return Ok(None) };
        if self.msgs[index].text.len() > size && !noerror {
            return Err(E2BIG);
        }
        let Some(mut msg) = self.msgs.remove(index) else { return Ok(None) };
        self.cbytes -= msg.text.len();
        msg.text.truncate(size);
        self.lrpid = pid;
        self.rtime = now;
        Ok(Some(msg))
    }
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
    pub fn new(nsems: usize, now: i64) -> Self {
        SemSet { vals: vec![0; nsems], pids: vec![0; nsems], otime: 0, ctime: now, waiters: Vec::new() }
    }

    pub fn nsems(&self) -> usize {
        self.vals.len()
    }

    /// Apply all of `ops` or none of them, in order, as process `pid`.
    /// `adj` is the caller's undo adjustments for this set, changed only
    /// when every operation applies. ERANGE when a value would pass SEMVMX
    /// or an adjustment leave i16's range. Every `num` is below nsems.
    pub fn attempt(&mut self, ops: &[SemOp], adj: &mut [i16], pid: i32, now: i64) -> Result<Attempt, i32> {
        let mut vals: Vec<i32> = self.vals.iter().map(|&v| i32::from(v)).collect();
        let mut new_adj: Vec<i32> = adj.iter().map(|&a| i32::from(a)).collect();
        for op in ops {
            let num = usize::from(op.num);
            let val = vals[num];
            let wait = if op.op == 0 { val != 0 } else { val + i32::from(op.op) < 0 };
            if wait {
                let nowait = op.flg as u16 as u32 & IPC_NOWAIT != 0;
                return Ok(Attempt::Block { num: op.num, zero: op.op == 0, nowait });
            }
            let next = val + i32::from(op.op);
            if next > SEMVMX {
                return Err(ERANGE);
            }
            if op.flg & SEM_UNDO != 0 && op.op != 0 {
                let undo = new_adj[num] - i32::from(op.op);
                if undo < i32::from(i16::MIN) || undo > i32::from(i16::MAX) {
                    return Err(ERANGE);
                }
                new_adj[num] = undo;
            }
            vals[num] = next;
        }
        for (slot, v) in self.vals.iter_mut().zip(&vals) {
            *slot = *v as u16;
        }
        for (slot, a) in adj.iter_mut().zip(&new_adj) {
            *slot = *a as i16;
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

    /// SETVAL: ERANGE outside 0..=SEMVMX.
    pub fn set_val(&mut self, num: usize, val: i32, pid: i32, now: i64) -> Result<(), i32> {
        if !(0..=SEMVMX).contains(&val) {
            return Err(ERANGE);
        }
        self.vals[num] = val as u16;
        self.pids[num] = pid;
        self.ctime = now;
        Ok(())
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
    /// adjustment, held to 0..=SEMVMX. Whether any value changed.
    pub fn apply_undo(&mut self, adj: &[i16], pid: i32) -> bool {
        let mut changed = false;
        let nsems = self.vals.len();
        for (num, &a) in adj.iter().enumerate().filter(|&(num, &a)| a != 0 && num < nsems) {
            let next = (i32::from(self.vals[num]) + i32::from(a)).clamp(0, SEMVMX);
            self.vals[num] = next as u16;
            self.pids[num] = pid;
            changed = true;
        }
        changed
    }
}

/// The SEM_UNDO adjustments of every process, by thread group and set, and
/// the thread groups whose rows are ending.
pub struct UndoLists {
    lists: BTreeMap<(u64, i32), Vec<i16>>,
    exiting: BTreeMap<u64, Vec<u64>>,
}

impl UndoLists {
    pub const fn new() -> Self {
        UndoLists { lists: BTreeMap::new(), exiting: BTreeMap::new() }
    }

    /// The adjustments of thread group `tgid` for set `semid` of `nsems`
    /// semaphores, made empty if it has none.
    pub fn list(&mut self, tgid: u64, semid: i32, nsems: usize) -> &mut Vec<i16> {
        self.lists.entry((tgid, semid)).or_insert_with(|| vec![0; nsems])
    }

    /// A copy of those adjustments, all zero if there are none, made
    /// without adding a list.
    pub fn current(&self, tgid: u64, semid: i32, nsems: usize) -> Vec<i16> {
        self.lists.get(&(tgid, semid)).cloned().unwrap_or_else(|| vec![0; nsems])
    }

    /// Take every list of thread group `tgid`.
    pub fn take_group(&mut self, tgid: u64) -> Vec<(i32, Vec<i16>)> {
        let keys: Vec<(u64, i32)> = self.lists.range((tgid, i32::MIN)..=(tgid, i32::MAX)).map(|(&k, _)| k).collect();
        keys.into_iter().filter_map(|k| self.lists.remove(&k).map(|adj| (k.1, adj))).collect()
    }

    /// IPC_RMID: drop every process's list for `semid`.
    pub fn drop_set(&mut self, semid: i32) {
        self.lists.retain(|&(_, id), _| id != semid);
    }

    /// SETVAL and SETALL: clear every process's adjustment of semaphore
    /// `num` of `semid`, or of all its semaphores.
    pub fn clear(&mut self, semid: i32, num: Option<usize>) {
        for (_, adj) in self.lists.iter_mut().filter(|((_, id), _)| *id == semid) {
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

    /// Row `row` of thread group `tgid` is ending, and `live` lists the
    /// group's rows that had not ended a moment before (`row` among them).
    /// Whether the group ends with it: every live row has now reached here.
    /// Two last rows ending at once each count the other as live, and the
    /// second to arrive finds both recorded.
    pub fn row_ending(&mut self, tgid: u64, row: u64, live: &[u64]) -> bool {
        let ending = self.exiting.entry(tgid).or_default();
        if !ending.contains(&row) {
            ending.push(row);
        }
        let ends = live.iter().all(|r| ending.contains(r));
        if ends {
            self.exiting.remove(&tgid);
        }
        ends
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
