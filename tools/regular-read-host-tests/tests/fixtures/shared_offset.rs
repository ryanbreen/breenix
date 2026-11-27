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
    pub mod permissions {
        pub struct Credentials;
        impl Credentials {
            pub fn current(_: bool) -> Result<Self, u64> { Ok(Self) }
            pub fn privileged(&self) -> bool { false }
        }
    }
    pub mod ufs {
        pub mod file {
            pub struct UfsFile;
            impl UfsFile {
                pub fn preadv_once(&self, _: u64, _: usize) -> Result<Vec<u8>, u64> { unreachable!() }
                pub fn pwritev_once(
                    &self, _: u64, _: usize, _: impl FnOnce(usize) -> Result<Vec<u8>, u64>,
                ) -> Result<u64, u64> { unreachable!() }
            }
        }
    }
    pub mod errno {
        pub struct Errno(pub i32);
        impl From<Errno> for u64 {
            fn from(value: Errno) -> Self { value.0 as u64 }
        }
    }
    pub mod vfs {
        pub use crate::Node as VNode;
        pub trait FileSystem {
            fn getattr(&self, ino: u64) -> Result<crate::Attrs, super::errno::Errno>;
            fn read(&self, node: &VNode, offset: u64, out: &mut [u8]) -> Result<usize, super::errno::Errno>;
            fn write(&self, node: &VNode, req: WriteReq, input: &[u8]) -> Result<WriteResult, WriteError>;
        }
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
    pub use fd::FdKind;
    pub mod pipe { pub const PIPE_BUF: usize = 4096; }

    // SLEEP_MUTEX
    pub mod fd {
        use crate::fs::vfs::file::OpenFile;
        use super::super::{Arc, Mutex, StubFile, DirectoryFile};
        pub mod status_flags {
            pub const O_APPEND: u32 = 0x400;
            pub const O_NONBLOCK: u32 = 0x800;
        }

        #[allow(dead_code)] // The fixture only uses the open file and its cursor.
        // REGULAR_FILE

        #[derive(Clone)]
        #[allow(dead_code)]
        pub enum FdKind {
            // REGULAR_VARIANT
            Directory(Arc<Mutex<DirectoryFile>>),
            Ipc(Arc<StubFile>),
            Pipe,
            PipeWrite(Arc<()>),
            FifoWrite((), Arc<()>, Arc<()>),
            FifoReadWrite((), Arc<()>, Arc<()>),
            ProcfsDirectory {},
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
            pub fn readable(&self) -> bool { true }
            pub fn writable(&self) -> bool { true }
            pub fn status_flags(&self) -> u32 { self.description.status_flags() }
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
    static REPLACEMENT: RefCell<Option<FileDescriptor>> = const { RefCell::new(None) };
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
    use super::*;
    #[path = IOVEC_RANGE_PATH]
    mod iovec_range;
    #[allow(dead_code)] // Exercise the implicit-offset entry points.
    #[path = IOVEC_PATH]
    pub mod iovec;
    pub mod metadata {
        use super::*;
        pub fn descriptor_entry(_: i32) -> Result<FileDescriptor, u64> {
            assert_unlocked();
            DESCRIPTOR.with(|entry| entry.borrow().clone().ok_or(errno::EBADF as u64))
        }
    }
    pub mod fs {
        pub fn touch_fifo(_: &(), _: bool) { unreachable!() }
    }
    pub mod blocking_io {
        use super::*;
        pub fn write_pipe(_: &(), _: &[u8], _: bool) -> SyscallResult { unreachable!() }
    }
    pub mod userptr {
        use super::*;
        pub fn copy_from_user<T: Copy>(source: *const T) -> Result<T, u64> {
            assert_unlocked();
            Ok(unsafe { source.read_unaligned() })
        }
        pub fn read_user_bytes(dst: *mut u8, src: u64, len: usize) -> Result<(), u64> {
            assert_unlocked();
            unsafe { std::ptr::copy_nonoverlapping(src as *const u8, dst, len) };
            Ok(())
        }
        pub fn write_user_prefix(dst: u64, src: *const u8, len: usize) -> usize {
            copy_to_user(dst, src as u64, len).unwrap();
            len
        }
    }
    pub mod errno {
        pub const EBADF: i32 = 9;
        pub const ENOMEM: i32 = 12;
        pub const EFAULT: i32 = 14;
        pub const EISDIR: i32 = 21;
        pub const EINVAL: i32 = 22;
        pub const ESPIPE: i32 = 29;
        pub const EIO: i32 = 5;
    }
    pub mod resource {
        pub fn current_fsize() -> u64 { u64::MAX }
        pub fn write_length(_: u64, count: usize, _: u64) -> Result<usize, u64> { Ok(count) }
        pub fn signal_fsize() { crate::assert_unlocked(); }
    }
    pub mod handlers {
        use super::*;
        pub fn sys_read(_: u64, buf: u64, count: u64) -> SyscallResult { calls::read(buf, count) }
        pub fn write_vector(fd: u64, buf: u64, count: u64, _: bool) -> SyscallResult {
            let entry = metadata::descriptor_entry(fd as i32).unwrap();
            let data = unsafe { std::slice::from_raw_parts(buf as *const u8, count as usize) };
            calls::write(&entry, data)
        }
        // POSITIONED_HANDLERS
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
    // Reuse the fd as soon as the first destination receives its bytes.
    if let Some(entry) = REPLACEMENT.with(|slot| slot.take()) {
        DESCRIPTOR.with(|slot| slot.replace(Some(entry)));
    }
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
    fn as_any(&self) -> &dyn std::any::Any { self }
}
struct DirectoryFile { position: u64 }

pub struct Node { ino: u64 }
pub struct Attrs { size: u64 }
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

impl fs::vfs::FileSystem for Backend {
    fn getattr(&self, ino: u64) -> Result<Attrs, fs::errno::Errno> {
        self.getattr(ino)
    }
    fn read(&self, node: &Node, offset: u64, out: &mut [u8]) -> Result<usize, fs::errno::Errno> {
        self.read(node, offset, out)
    }
    fn write(&self, node: &Node, req: fs::vfs::WriteReq, input: &[u8]) -> Result<fs::vfs::WriteResult, fs::vfs::WriteError> {
        self.write(node, req, input)
    }
}

fn independent_open(entry: &FileDescriptor) -> FileDescriptor {
    let FdKind::RegularFile(file) = &entry.kind else { unreachable!() };
    ipc::fd::open(
        fs::vfs::file::OpenFile { fs: file.open.fs.clone(), node: file.open.node.clone() },
        entry.status_flags(),
    ).ok().unwrap()
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

#[test]
fn vector_read_keeps_one_file_and_advances_only_its_cursor() {
    let open = |bytes: &[u8], ino| {
        let fs = Arc::new(Backend {
            bytes: Mutex::new(bytes.to_vec()),
            pause: Mutex::new(None),
            entered: mpsc::sync_channel(0).0,
            resume: Mutex::new(mpsc::sync_channel(0).1),
        });
        ipc::fd::open(fs::vfs::file::OpenFile { fs, node: Arc::new(Node { ino }) }, 0).ok().unwrap()
    };
    let original = open(b"abcd", 1);
    DESCRIPTOR.with(|slot| slot.replace(Some(original.clone())));
    REPLACEMENT.with(|slot| slot.replace(Some(open(b"WXYZ", 2))));

    let mut a = [0; 2];
    let mut b = [0; 2];
    let vectors = [
        UserVector { base: a.as_mut_ptr() as u64, len: 2 },
        UserVector { base: b.as_mut_ptr() as u64, len: 2 },
    ];
    assert_eq!(syscall::iovec::sys_readv(3, vectors.as_ptr() as u64, 2), SyscallResult::Ok(4));
    assert_eq!((a, b), (*b"ab", *b"cd"));
    assert_eq!(syscall::calls::sys_lseek(3, 0, SEEK_CUR), SyscallResult::Ok(0));
    DESCRIPTOR.with(|slot| slot.replace(Some(original)));
    assert_eq!(syscall::calls::sys_lseek(3, 0, SEEK_CUR), SyscallResult::Ok(4));
}

#[test]
fn vector_append_is_one_record_across_independent_opens() {
    let (a, b, bytes, position) = interleave(
        b"start", Pause::Append,
        |_| {
            let vectors = [
                UserVector { base: b"A".as_ptr() as u64, len: 1 },
                UserVector { base: b"B".as_ptr() as u64, len: 1 },
            ];
            syscall::iovec::sys_writev(3, vectors.as_ptr() as u64, 2)
        },
        |entry| syscall::calls::write(&independent_open(entry), b"bbb"),
    );
    assert_eq!(a, SyscallResult::Ok(2));
    assert_eq!(b, SyscallResult::Ok(3));
    assert_eq!(bytes, b"startABbbb");
    assert_eq!(position, SyscallResult::Ok(7));
}

#[repr(C)]
struct UserVector { base: u64, len: u64 }
