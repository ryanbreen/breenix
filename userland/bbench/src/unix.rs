//! POSIX operations shared by the guest and the native reference probe.
use std::ffi::{c_void, CString};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{self, NonNull};

extern "C" {
    fn fork() -> i32;
    fn execv(path: *const i8, argv: *const *const i8) -> i32;
    fn _exit(status: i32) -> !;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn getpagesize() -> i32;
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
        -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}

pub fn fork_exec_wait(path: &Path, count: usize) -> io::Result<()> {
    let binary = CString::new(path.as_os_str().as_bytes())?;
    let argv = [binary.as_ptr(), ptr::null()];
    for _ in 0..count {
        // SAFETY: The probe is single-threaded. The child uses only execv and
        // _exit, with the pathname and argv allocated before fork.
        let pid = unsafe { fork() };
        if pid == 0 {
            unsafe {
                execv(binary.as_ptr(), argv.as_ptr());
                _exit(127);
            }
        }
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut status = 0;
        loop {
            // SAFETY: pid is this iteration's child and status is writable.
            if unsafe { waitpid(pid, &mut status, 0) } == pid {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        if status != 0 {
            return Err(io::Error::other(format!(
                "tiny child wait status {status:#x}"
            )));
        }
    }
    Ok(())
}

pub fn page_size() -> usize {
    // SAFETY: getpagesize takes no arguments.
    unsafe { getpagesize() as usize }
}

pub struct Mapping {
    address: NonNull<u8>,
    len: usize,
}

impl Mapping {
    pub fn new(len: usize, file: Option<&File>) -> io::Result<Self> {
        const PRIVATE: i32 = 2;
        const READ: i32 = 1;
        const WRITE: i32 = 2;
        #[cfg(target_os = "macos")]
        const ANONYMOUS: i32 = 0x1000;
        #[cfg(not(target_os = "macos"))]
        const ANONYMOUS: i32 = 0x20;
        let (prot, flags, fd) = match file {
            Some(file) => (READ, PRIVATE, file.as_raw_fd()),
            None => (READ | WRITE, PRIVATE | ANONYMOUS, -1),
        };
        // SAFETY: The kernel chooses a private address range. The file, when
        // supplied, stays open for this call.
        let address = unsafe { mmap(ptr::null_mut(), len, prot, flags, fd, 0) };
        if address as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            address: NonNull::new(address.cast()).ok_or_else(|| io::Error::other("null mmap"))?,
            len,
        })
    }

    pub fn touch_read(&self, stride: usize) -> u64 {
        (0..self.len)
            .step_by(stride)
            // SAFETY: Each offset is inside this live readable mapping.
            .map(|offset| unsafe { self.address.as_ptr().add(offset).read_volatile() as u64 })
            .sum()
    }
}

pub fn touch_anonymous(len: usize, stride: usize) -> io::Result<()> {
    let mapping = Mapping::new(len, None)?;
    for offset in (0..len).step_by(stride) {
        // SAFETY: This is a writable anonymous mapping and offset < len.
        unsafe { mapping.address.as_ptr().add(offset).write_volatile(0xa5) };
    }
    Ok(())
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: This owns the indicated mmap range.
        unsafe { munmap(self.address.as_ptr().cast(), self.len) };
    }
}
