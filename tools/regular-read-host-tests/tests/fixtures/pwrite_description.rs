// The production pwrite64 arm (`sys_pwrite64`, `write_positional_file`) over a
// host descriptor table. The backend runs another thread's close of the last
// descriptor while the write is pending, then records whether the open
// description's write access, which counts in the file's writers (ETXTBSY,
// docs/plans/e3-page-cache.md 10.3), was released before the write finished.
use std::cell::{Cell, RefCell};
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub enum SyscallResult { Ok(u64), Err(u64) }

thread_local! {
    static TABLE: RefCell<Option<ipc::fd::FileDescriptor>> = const { RefCell::new(None) };
    static WRITER_RELEASED: Cell<bool> = const { Cell::new(false) };
    static RELEASED_DURING_WRITE: Cell<Option<bool>> = const { Cell::new(None) };
}

mod fs {
    pub mod errno {
        pub struct Errno(pub i32);
        impl From<Errno> for u64 {
            fn from(value: Errno) -> Self { value.0 as u64 }
        }
    }
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
                pub fn pwrite(&self, _: u64, _: usize, _: u64, _: bool) -> Result<u64, u64> { unreachable!() }
            }
        }
    }
    pub mod vfs {
        pub struct VNode;
        pub trait FileSystem {
            fn write(&self, node: &VNode, req: WriteReq, input: &[u8]) -> Result<WriteResult, WriteError>;
        }
        pub mod file {
            pub struct OpenFile {
                pub fs: std::sync::Arc<dyn super::FileSystem>,
                pub node: std::sync::Arc<super::VNode>,
            }
        }
        pub enum WriteAt { Offset(u64) }
        pub struct WriteReq {
            pub at: WriteAt,
            pub limit: u64,
            pub clear_setid: bool,
        }
        pub struct WriteResult { pub bytes: usize }
        // Constructed only by a backend's size check; the arm matches it.
        #[allow(dead_code)]
        pub enum WriteError { FileSizeLimit }
        impl WriteError {
            pub fn errno(&self) -> super::errno::Errno { super::errno::Errno(27) }
        }
    }
}

mod ipc {
    pub use fd::FdKind;
    pub mod fd {
        use std::sync::Arc;
        pub mod status_flags {
            pub const O_APPEND: u32 = 0x400;
        }
        pub struct RegularFile { pub open: crate::fs::vfs::file::OpenFile }
        pub struct UserFile;
        impl UserFile {
            pub fn as_any(&self) -> &dyn std::any::Any { self }
        }
        #[derive(Clone)]
        pub enum FdKind {
            RegularFile(Arc<RegularFile>),
            // Constructed only by the kernel; the arm matches them.
            #[allow(dead_code)]
            Ipc(Arc<UserFile>),
            #[allow(dead_code)]
            PipeRead(Arc<()>),
            #[allow(dead_code)]
            PipeWrite(Arc<()>),
        }
        /// The write access an open with O_WRONLY or O_RDWR holds
        /// (`OpenFileDescription::writer` in the kernel).
        pub struct WriteOpen;
        impl Drop for WriteOpen {
            fn drop(&mut self) { crate::WRITER_RELEASED.with(|released| released.set(true)); }
        }
        pub struct OpenFileDescription { _writer: WriteOpen }
        #[derive(Clone)]
        pub struct FileDescriptor {
            pub kind: FdKind,
            _description: Arc<OpenFileDescription>,
        }
        impl FileDescriptor {
            pub fn new(kind: FdKind) -> Self {
                Self { kind, _description: Arc::new(OpenFileDescription { _writer: WriteOpen }) }
            }
            pub fn writable(&self) -> bool { true }
            pub fn status_flags(&self) -> u32 { 0 }
        }
    }
}

mod syscall {
    pub use crate::SyscallResult;
    pub mod errno {
        pub const EBADF: i32 = 9;
        pub const EFAULT: i32 = 14;
        pub const EINVAL: i32 = 22;
        pub const ESPIPE: i32 = 29;
    }
    pub mod metadata {
        pub fn descriptor_entry(_: i32) -> Result<crate::ipc::fd::FileDescriptor, u64> {
            crate::TABLE.with(|table| table.borrow().clone().ok_or(super::errno::EBADF as u64))
        }
    }
    pub mod resource {
        pub fn current_fsize() -> u64 { u64::MAX }
        pub fn write_length(_: u64, count: usize, _: u64) -> Result<usize, u64> { Ok(count) }
        pub fn signal_fsize() {}
    }
    pub mod handlers {
        use super::SyscallResult;
        fn copy_from_user(user_ptr: u64, len: usize) -> Result<Vec<u8>, &'static str> {
            // SAFETY: the test passes a live buffer of `len` bytes.
            Ok(unsafe { std::slice::from_raw_parts(user_ptr as *const u8, len) }.to_vec())
        }
        fn validate_fd_for_degenerate_transfer(_: i32, _: bool) -> Result<(), u64> { Ok(()) }
        // PWRITE64
    }
}

/// A filesystem whose write first runs another thread's close(fd) of the last
/// descriptor that shares the description.
struct ClosingBackend;
impl fs::vfs::FileSystem for ClosingBackend {
    fn write(
        &self, _: &fs::vfs::VNode, req: fs::vfs::WriteReq, input: &[u8],
    ) -> Result<fs::vfs::WriteResult, fs::vfs::WriteError> {
        let fs::vfs::WriteAt::Offset(offset) = req.at;
        assert_eq!((offset, req.limit, req.clear_setid), (64, u64::MAX, true));
        drop(TABLE.with(|table| table.borrow_mut().take()));
        RELEASED_DURING_WRITE.with(|seen| seen.set(Some(WRITER_RELEASED.with(Cell::get))));
        Ok(fs::vfs::WriteResult { bytes: input.len() })
    }
}

#[test]
fn a_close_during_pwrite_keeps_write_access_until_the_write_returns() {
    let open = fs::vfs::file::OpenFile { fs: Arc::new(ClosingBackend), node: Arc::new(fs::vfs::VNode) };
    let entry = ipc::fd::FileDescriptor::new(ipc::FdKind::RegularFile(Arc::new(ipc::fd::RegularFile { open })));
    TABLE.with(|table| *table.borrow_mut() = Some(entry));
    let buffer = [0x5a_u8; 16];
    let result = syscall::handlers::sys_pwrite64(3, buffer.as_ptr() as u64, buffer.len() as u64, 64);
    assert_eq!(result, SyscallResult::Ok(16));
    assert_eq!(
        RELEASED_DURING_WRITE.with(Cell::get),
        Some(false),
        "the description's write access ended before its pwrite64 reached the file, so an exec could share the file's pages in between",
    );
    assert!(WRITER_RELEASED.with(Cell::get), "the last reference did not release write access");
}
