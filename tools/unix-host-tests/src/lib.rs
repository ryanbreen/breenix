//! Host execution of the production Unix message queue with counted-descriptor
//! shims. Filesystem and scheduler behaviour still require guest validation.
#![cfg(test)]
extern crate alloc;

#[path = "../../../kernel/src/socket/addr.rs"]
pub mod addr;
mod namespace_support;
#[path = "../../../kernel/src/socket/unix_msg.rs"]
pub mod unix_msg;
#[path = "../../../kernel/src/socket/unix_ns.rs"]
pub mod unix_ns;
pub use namespace_support::{fs, irq_safe_mutex};
pub use socket::object;

pub mod socket {
    pub use crate::addr;
    pub mod object {
        #[derive(Debug, PartialEq)]
        pub struct UCred {
            pub pid: u32,
            pub uid: u32,
            pub gid: u32,
        }
        pub struct Socket;
    }
    pub use crate::unix_msg;
    pub use crate::unix_ns;
}
pub mod ipc {
    pub mod fd {
        #[derive(Debug)]
        pub struct FileDescriptor(pub std::sync::Arc<()>);
    }
}
pub mod syscall {
    pub enum SyscallResult {
        Ok(u64),
        Err(u64),
    }
    pub use crate::namespace_support::{at_path, fifo, metadata};
    pub mod fs {
        pub const S_IFSOCK: u32 = 0o140000;
    }
    pub mod userptr {
        pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, u64> {
            if ptr.is_null() {
                return Err(14);
            }
            Ok(unsafe { ptr.read_unaligned() })
        }
        pub fn copy_to_user<T: Copy>(ptr: *mut T, val: &T) -> Result<(), u64> {
            if ptr.is_null() {
                return Err(14);
            }
            unsafe {
                ptr.write_unaligned(*val);
            }
            Ok(())
        }
    }
    pub mod errno {
        pub const EAGAIN: i32 = 11;
        pub const EMSGSIZE: i32 = 90;
        pub const ENOMEM: i32 = 12;
        pub const EAFNOSUPPORT: i32 = 97;
        pub const EINVAL: i32 = 22;
        pub const EFAULT: i32 = 14;
        pub const EIO: i32 = 5;
        pub const ENOENT: i32 = 2;
        pub const EACCES: i32 = 13;
        pub const ECONNREFUSED: i32 = 111;
        pub const EEXIST: i32 = 17;
        pub const EADDRINUSE: i32 = 98;
        pub const ENOBUFS: i32 = 105;
    }
}
pub mod task {
    pub mod process_task {
        pub fn close_extracted_fds(entries: Vec<(usize, crate::ipc::fd::FileDescriptor)>) {
            drop(entries);
        }
    }
}

use socket::{
    addr::UnixName,
    object::UCred,
    unix_msg::{UnixMsg, UnixQueue, MAX_RIGHTS},
};
fn message(data: &[u8]) -> UnixMsg {
    UnixMsg::new(
        data,
        Vec::new(),
        UCred {
            pid: 7,
            uid: 8,
            gid: 9,
        },
        UnixName::Unnamed,
    )
    .unwrap()
}

#[test]
fn records_keep_boundaries_and_zero_length_messages() {
    let mut q = UnixQueue::new();
    for bytes in [&b"one"[..], &b"seven!!"[..], &b""[..]] {
        assert!(q.push(message(bytes), 64).is_ok());
    }
    for bytes in [&b"one"[..], &b"seven!!"[..], &b""[..]] {
        let msg = q.pop().unwrap();
        assert_eq!(msg.bytes(), bytes);
        assert_eq!(msg.cred.pid, 7);
        assert_eq!(msg.from, UnixName::Unnamed);
    }
    assert!(q.is_empty());
    assert_eq!(q.room(64), 64);
}

#[test]
fn partial_stream_reads_release_only_consumed_capacity() {
    let mut q = UnixQueue::new();
    assert!(q.push(message(b"abcdef"), 8).is_ok());
    q.consume(2);
    assert_eq!(q.front().unwrap().bytes(), b"cdef");
    assert_eq!(q.room(8), 4);
    assert_eq!(q.charged_bytes(), 4);
    assert!(q.push(message(b"ghij"), 8).is_ok());
    assert_eq!(q.room(8), 0);
    assert_eq!(q.pop().unwrap().bytes(), b"cdef");
    assert_eq!(q.front_mut().unwrap().bytes(), b"ghij");
    q.consume(100);
    assert_eq!(q.pop().unwrap().bytes(), b"");
    assert_eq!(q.room(8), 8);
}

#[test]
fn stream_writes_coalesce_and_reclaim_consumed_prefixes() {
    let mut q = UnixQueue::new();
    for _ in 0..2048 {
        assert!(q.push_stream(message(b"a"), 2048).is_ok());
    }
    assert_eq!(q.stream_room(2048), 0);
    assert_eq!(q.iter().count(), 1);
    q.consume(1024);
    assert_eq!(q.stream_room(2048), 1024);
    for _ in 0..1024 {
        assert!(q.push_stream(message(b"b"), 2048).is_ok());
    }
    assert_eq!(q.charged_bytes(), 2048);
    let msg = q.pop().unwrap();
    assert_eq!(msg.bytes().len(), 2048);
    assert!(msg.bytes()[..1024].iter().all(|b| *b == b'a'));
    assert!(msg.bytes()[1024..].iter().all(|b| *b == b'b'));
}

#[test]
fn full_queue_refuses_without_discarding_existing_records() {
    let mut q = UnixQueue::new();
    assert!(q.push(message(b"1234"), 4).is_ok());
    assert_eq!(
        q.push(message(b"x"), 4).err().unwrap().0,
        syscall::errno::EAGAIN
    );
    assert_eq!(
        q.push(message(b"12345"), 4).err().unwrap().0,
        syscall::errno::EMSGSIZE
    );
    assert_eq!(q.pop().unwrap().bytes(), b"1234");
    for _ in 0..1024 {
        assert!(q.push(message(b""), 2048).is_ok());
    }
    assert_eq!(q.room(2048), 0);
    assert_eq!(
        q.push(message(b""), 2048).err().unwrap().0,
        syscall::errno::EAGAIN
    );
}

#[test]
fn rights_live_in_flight_and_transfer_once_to_receiver() {
    let file = std::sync::Arc::new(());
    let mut q = UnixQueue::new();
    let msg = UnixMsg::new(
        b"F",
        vec![ipc::fd::FileDescriptor(file.clone())],
        UCred {
            pid: 1,
            uid: 2,
            gid: 3,
        },
        UnixName::Path(b"/tmp/sock".to_vec()),
    )
    .unwrap();
    assert!(q.push(msg, 64).is_ok());
    assert_eq!(std::sync::Arc::strong_count(&file), 2);
    let before = q.room(64);
    let received = q.take_rights();
    assert_eq!(received.len(), 1);
    assert!(q.take_rights().is_empty());
    assert!(q.room(64) > before);
    drop(q);
    assert_eq!(std::sync::Arc::strong_count(&file), 2);
    drop(received);
    assert_eq!(std::sync::Arc::strong_count(&file), 1);
}

#[test]
fn discarded_messages_close_rights_including_rejected_sends() {
    let file = std::sync::Arc::new(());
    let mut q = UnixQueue::new();
    let msg = UnixMsg::new(
        b"F",
        vec![ipc::fd::FileDescriptor(file.clone())],
        UCred {
            pid: 1,
            uid: 2,
            gid: 3,
        },
        UnixName::Abstract(b"a\0b".to_vec()),
    )
    .unwrap();
    assert!(q.push(msg, 64).is_ok());
    drop(q);
    assert_eq!(std::sync::Arc::strong_count(&file), 1);
    let rights = (0..MAX_RIGHTS + 1)
        .map(|_| ipc::fd::FileDescriptor(file.clone()))
        .collect();
    assert!(matches!(
        UnixMsg::new(
            b"F",
            rights,
            UCred {
                pid: 1,
                uid: 2,
                gid: 3
            },
            UnixName::Unnamed
        ),
        Err(syscall::errno::EMSGSIZE)
    ));
    assert_eq!(std::sync::Arc::strong_count(&file), 1);
}
