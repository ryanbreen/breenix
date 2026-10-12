//! A message queue's messages and the checks on its name and attributes:
//! the parts of POSIX message queues that need no process, lock or user
//! memory.

use crate::syscall::errno::{EACCES, EINVAL, EMSGSIZE, ENAMETOOLONG};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

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
/// The only flag mq_setattr may change.
pub const O_NONBLOCK: i64 = 0o4000;

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
/// the C library strips: longer than NAME_MAX is ENAMETOOLONG; empty, `.`,
/// `..` or containing a slash is EACCES (Linux).
pub fn check_name(name: &[u8]) -> Result<(), u64> {
    if name.len() > NAME_MAX {
        return Err(ENAMETOOLONG as u64);
    }
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
        return Err(EACCES as u64);
    }
    Ok(())
}

/// The (maxmsg, msgsize) a new queue gets from `attr`: the defaults for
/// None; EINVAL for a value of 0 or less, or above the caller's cap.
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

/// mq_setattr's new flags: only O_NONBLOCK may be set; any other bit is
/// EINVAL (Linux). mq_maxmsg, mq_msgsize and mq_curmsgs are ignored.
pub fn check_setattr(attr: &MqAttr) -> Result<bool, u64> {
    if attr.mq_flags & !O_NONBLOCK != 0 {
        return Err(EINVAL as u64);
    }
    Ok(attr.mq_flags & O_NONBLOCK != 0)
}

/// A queue's messages: at most `maxmsg`, taken highest priority first and
/// oldest first within a priority.
#[derive(Debug)]
pub struct MessageStore {
    maxmsg: usize,
    count: usize,
    by_prio: BTreeMap<u32, VecDeque<Vec<u8>>>,
}

impl MessageStore {
    pub const fn new(maxmsg: usize) -> Self {
        Self { maxmsg, count: 0, by_prio: BTreeMap::new() }
    }

    /// The messages queued.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn is_full(&self) -> bool {
        self.count >= self.maxmsg
    }

    /// Queue `msg` at `prio` behind any of the same priority. A full queue
    /// refuses it and hands it back.
    pub fn push(&mut self, prio: u32, msg: Vec<u8>) -> Result<(), Vec<u8>> {
        if self.is_full() {
            return Err(msg);
        }
        self.by_prio.entry(prio).or_default().push_back(msg);
        self.count += 1;
        Ok(())
    }

    /// Take the oldest message of the highest priority, with its priority.
    pub fn pop(&mut self) -> Option<(Vec<u8>, u32)> {
        let mut entry = self.by_prio.last_entry()?;
        let prio = *entry.key();
        let msg = entry.get_mut().pop_front()?;
        if entry.get().is_empty() {
            entry.remove();
        }
        self.count -= 1;
        Some((msg, prio))
    }
}
