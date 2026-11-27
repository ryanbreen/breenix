#![feature(allocator_api)]

extern crate alloc;
extern crate self as log;
extern crate self as spin;

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::cell::{Cell, RefCell};
use std::time::Duration;

#[path = REGULAR_READ_PATH]
mod regular_read;

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }};
}
pub use crate::debug as error;

// Keep the same poison-free signature as the short kernel metadata lock.
struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}

mod fs {
    pub mod errno {
        pub struct Errno(pub i32);
        impl From<Errno> for u64 {
            fn from(value: Errno) -> Self { value.0 as u64 }
        }
    }
    pub mod vfs {
        pub mod file {
            pub struct OpenFile {
                pub fs: std::sync::Arc<crate::Backend>,
                pub node: std::sync::Arc<crate::Node>,
            }
        }
        pub enum WriteAt { Offset(u64), Append }
        pub struct WriteReq {
            pub at: WriteAt,
            pub limit: u64,
            pub clear_setid: bool,
        }
        pub struct WriteResult { pub offset: u64, pub bytes: usize }
        pub enum WriteError { FileSizeLimit }
        impl WriteError {
            pub fn errno(&self) -> super::errno::Errno { super::errno::Errno(27) }
        }
    }
}

mod ipc {
    use crate::syscall::errno;

    // SLEEP_MUTEX
    pub mod fd {
        use crate::fs::vfs::file::OpenFile;
        use super::super::{Arc, Mutex, StubFile, DirectoryFile};
        pub mod status_flags { pub const O_APPEND: u32 = 0x400; }

        #[allow(dead_code)] // The fixture only uses the open file and its cursor.
        // REGULAR_FILE

        #[derive(Clone)]
        #[allow(dead_code)]
        pub enum FdKind {
            // REGULAR_VARIANT
            Directory(Arc<Mutex<DirectoryFile>>),
            Ipc(Arc<StubFile>),
            Pipe,
        }

        pub struct OpenFileDescription(u32);
        impl OpenFileDescription {
            pub fn status_flags(&self) -> u32 { self.0 }
        }

        #[derive(Clone)]
        pub struct FileDescriptor {
            pub kind: FdKind,
            pub description: Arc<OpenFileDescription>,
        }
        impl FileDescriptor {
            pub fn new(kind: FdKind, flags: u32) -> Self {
                Self { kind, description: Arc::new(OpenFileDescription(flags)) }
            }
        }

        pub fn open(open: OpenFile, flags: u32) -> Result<FileDescriptor, crate::fs::errno::Errno> {
            use crate::fs::errno::Errno;
            let inode_num = open.node.ino;
            let mount_id = 0;
            let kind = {
                // CONSTRUCTOR
            };
            Ok(FileDescriptor::new(kind, flags))
        }
    }

    pub mod wait {
        #[allow(dead_code)]
        pub enum IpcDeadline { Never }
        #[allow(dead_code)]
        pub enum WaitEnd { Woken, TimedOut, Interrupted(u64) }

        #[allow(dead_code)]
        pub fn wait_on(
            q: &crate::task::waitqueue::WaitQueueHead,
            _: &IpcDeadline,
            blocked: impl FnOnce() -> bool,
        ) -> WaitEnd {
            crate::assert_unlocked();
            let lock = q.state.lock();
            if blocked() {
                crate::send(crate::Event::Waiting);
                drop(q.wake.wait(lock).unwrap());
            }
            WaitEnd::Woken
        }
    }
}

mod task {
    pub mod scheduler {
        pub fn current_thread_id() -> Option<u64> { Some(1) }
    }
    pub mod waitqueue {
        #[allow(dead_code)]
        pub struct WaitQueueHead {
            pub state: crate::Mutex<()>,
            pub wake: std::sync::Condvar,
        }
        #[allow(dead_code)]
        impl WaitQueueHead {
            pub fn new() -> Self {
                Self { state: crate::Mutex::new(()), wake: std::sync::Condvar::new() }
            }
            pub fn wake_up(&self) {
                let _lock = self.state.lock();
                self.wake.notify_all();
            }
        }
    }
}

use ipc::fd::{FdKind, FileDescriptor};

thread_local! {
    static DESCRIPTOR: RefCell<Option<FileDescriptor>> = const { RefCell::new(None) };
    static PM_LOCKED: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Option<SyncSender<Event>>> = const { RefCell::new(None) };
}

fn assert_unlocked() { PM_LOCKED.with(|locked| assert!(!locked.get())); }

mod process {
    use super::*;
    pub struct ManagerGuard(Option<Process>);
    pub struct Process { pub fd_table: FdTable }
    pub struct FdTable(FileDescriptor);
    impl FdTable {
        pub fn get(&self, _: i32) -> Option<&FileDescriptor> { Some(&self.0) }
    }
    impl Process {
        pub fn find_process_by_thread(&self, _: u64) -> Option<(u64, &Self)> { Some((1, self)) }
        #[allow(dead_code)]
        pub fn find_process_by_thread_mut(&mut self, _: u64) -> Option<(u64, &mut Self)> { Some((1, self)) }
    }
    impl std::ops::Deref for ManagerGuard {
        type Target = Option<Process>;
        fn deref(&self) -> &Self::Target { &self.0 }
    }
    impl std::ops::DerefMut for ManagerGuard {
        fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
    }
    impl Drop for ManagerGuard {
        fn drop(&mut self) { PM_LOCKED.with(|locked| locked.set(false)); }
    }
    pub fn manager() -> ManagerGuard {
        PM_LOCKED.with(|locked| assert!(!locked.replace(true)));
        ManagerGuard(Some(Process {
            fd_table: FdTable(DESCRIPTOR.with(|entry| entry.borrow().as_ref().unwrap().clone())),
        }))
    }
}

mod syscall {
    pub mod errno {
        pub const ENOMEM: i32 = 12;
        #[allow(dead_code)]
        pub const EINVAL: i32 = 22;
    }
    pub mod resource {
        pub fn signal_fsize() { crate::assert_unlocked(); }
    }
    pub mod calls {
        use super::super::*;

        pub fn read(buf_ptr: u64, count: u64) -> SyscallResult {
            let manager_guard = crate::process::manager();
            match &manager_guard.as_ref().unwrap().fd_table.get(3).unwrap().kind {
                FdKind::RegularFile(file_ref) => {
                    // READ_ARM
                _ => unreachable!(),
            }
        }

        pub fn write(entry: &FileDescriptor, buffer: &[u8]) -> SyscallResult {
            let FdKind::RegularFile(file) = &entry.kind else { unreachable!() };
            let file = file.clone();
            // WRITE_BINDING
            let unprivileged = true;
            let size_limit = u64::MAX;
            let signal_limit = true;
            // WRITE_ARM
        }

        // LSEEK
    }
}

fn copy_to_user(dst: u64, src: u64, len: usize) -> Result<(), ()> {
    assert_unlocked();
    // The caller passes its live output slice, and regular_read owns the source.
    unsafe { std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, len) };
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum SyscallResult { Ok(u64), Err(u64) }

const SEEK_SET: i32 = 0;
const SEEK_CUR: i32 = 1;
const SEEK_END: i32 = 2;
struct StubFile;
impl StubFile {
    fn seek(&self, _: i64, _: i32) -> Result<u64, u64> { unreachable!() }
}
struct DirectoryFile { position: u64 }

struct Node { ino: u64 }
struct Attrs { size: u64 }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pause { Read, Append }

struct Backend {
    bytes: Mutex<Vec<u8>>,
    pause: Mutex<Option<Pause>>,
    entered: SyncSender<()>,
    resume: Mutex<Receiver<()>>,
}

impl Backend {
    fn stop_after(&self, operation: Pause) {
        let mut pause = self.pause.lock();
        if *pause == Some(operation) {
            *pause = None;
            drop(pause);
            self.entered.send(()).unwrap();
            receive(&self.resume.lock());
        }
    }

    fn getattr(&self, _: u64) -> Result<Attrs, fs::errno::Errno> {
        assert_unlocked();
        Ok(Attrs { size: self.bytes.lock().len() as u64 })
    }

    fn read(&self, _: &Node, offset: u64, out: &mut [u8]) -> Result<usize, fs::errno::Errno> {
        assert_unlocked();
        let bytes = self.bytes.lock();
        let start = (offset as usize).min(bytes.len());
        let count = out.len().min(bytes.len() - start);
        out[..count].copy_from_slice(&bytes[start..start + count]);
        drop(bytes);
        self.stop_after(Pause::Read);
        Ok(count)
    }

    fn write(&self, _: &Node, req: fs::vfs::WriteReq, input: &[u8]) -> Result<fs::vfs::WriteResult, fs::vfs::WriteError> {
        use fs::vfs::{WriteAt, WriteError, WriteResult};

        assert_unlocked();
        assert!(req.clear_setid);
        let mut bytes = self.bytes.lock();
        let offset = match req.at {
            WriteAt::Offset(at) => at,
            WriteAt::Append => bytes.len() as u64,
        };
        if offset >= req.limit {
            return Err(WriteError::FileSizeLimit);
        }
        let end = offset as usize + input.len();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        bytes[offset as usize..end].copy_from_slice(input);
        drop(bytes);
        self.stop_after(Pause::Append);
        Ok(WriteResult { offset, bytes: input.len() })
    }
}

fn receive<T>(channel: &Receiver<T>) -> T {
    channel.recv_timeout(Duration::from_secs(5)).expect("interleaving stalled")
}

enum Event { Waiting, Done }
fn send(event: Event) {
    EVENTS.with(|slot| {
        if let Some(sender) = slot.borrow().as_ref() {
            sender.send(event).unwrap();
        }
    });
}

fn interleave<A: Send, B: Send>(
    bytes: &[u8],
    pause: Pause,
    first: impl FnOnce(&FileDescriptor) -> A + Send,
    second: impl FnOnce(&FileDescriptor) -> B + Send,
) -> (A, B, Vec<u8>, SyscallResult) {
    let (entered, paused) = mpsc::sync_channel(1);
    let (resume, ready) = mpsc::sync_channel(1);
    let fs = Arc::new(Backend {
        bytes: Mutex::new(bytes.to_vec()),
        pause: Mutex::new(Some(pause)),
        entered,
        resume: Mutex::new(ready),
    });
    let entry = ipc::fd::open(
        fs::vfs::file::OpenFile { fs: fs.clone(), node: Arc::new(Node { ino: 1 }) },
        ipc::fd::status_flags::O_APPEND,
    ).ok().unwrap();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            DESCRIPTOR.with(|slot| slot.replace(Some(entry.clone())));
            first(&entry)
        });
        receive(&paused);
        let (events, contender) = mpsc::sync_channel(4);
        let entry = &entry;
        let b = scope.spawn(move || {
            DESCRIPTOR.with(|slot| slot.replace(Some(entry.clone())));
            EVENTS.with(|slot| slot.replace(Some(events)));
            let result = second(entry);
            send(Event::Done);
            result
        });
        // The old operation completes here. The gated one has to park first.
        let _ = receive(&contender);
        resume.send(()).unwrap();
        (a.join().unwrap(), b.join().unwrap())
    });
    DESCRIPTOR.with(|slot| slot.replace(Some(entry)));
    let position = syscall::calls::sys_lseek(3, 0, SEEK_CUR);
    let bytes = fs.bytes.lock().clone();
    (a, b, bytes, position)
}

fn read(_: &FileDescriptor) -> (SyscallResult, [u8; 4]) {
    let mut bytes = [0; 4];
    let result = syscall::calls::read(bytes.as_mut_ptr() as u64, bytes.len() as u64);
    (result, bytes)
}

#[test]
fn two_readers_get_disjoint_ranges() {
    let (a, b, _, position) = interleave(b"abcdefgh", Pause::Read, read, read);
    assert_eq!(a, (SyscallResult::Ok(4), *b"abcd"));
    assert_eq!(b, (SyscallResult::Ok(4), *b"efgh"));
    assert_eq!(position, SyscallResult::Ok(8));
}

#[test]
fn appenders_never_overlap_and_the_shared_cursor_ends_at_eof() {
    let (a, b, bytes, position) = interleave(
        b"start", Pause::Append,
        |entry| syscall::calls::write(entry, b"AA"),
        |entry| syscall::calls::write(entry, b"bbb"),
    );
    assert_eq!(a, SyscallResult::Ok(2));
    assert_eq!(b, SyscallResult::Ok(3));
    assert_eq!(bytes, b"startAAbbb");
    assert_eq!(position, SyscallResult::Ok(10));
}

#[test]
fn relative_seek_during_read_has_a_serial_result() {
    let (read, seek, _, position) = interleave(
        b"abcdefgh", Pause::Read, read, |_| syscall::calls::sys_lseek(3, 2, SEEK_CUR),
    );
    // This read already transferred abcd. If the relative seek had happened
    // first, the read would have started at c. Therefore it must see offset 4.
    assert_eq!(read, (SyscallResult::Ok(4), *b"abcd"));
    assert_eq!(seek, SyscallResult::Ok(6));
    assert_eq!(position, SyscallResult::Ok(6));
}
