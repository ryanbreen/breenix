//! Descriptor-relative access to one generation or one shared bind.
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(target_os = "breenix"))]
use libc as sys;

#[cfg(target_os = "breenix")]
mod sys {
    pub use libbreenix::fs::Stat as stat;
    use libbreenix::syscall::{nr, raw};
    pub const O_DIRECTORY: i32 = libbreenix::fs::O_DIRECTORY as i32;
    #[cfg(target_arch = "x86_64")]
    pub const O_NOFOLLOW: i32 = 0x20000;
    #[cfg(target_arch = "aarch64")]
    pub const O_NOFOLLOW: i32 = 0x8000;
    pub const O_CLOEXEC: i32 = 0x80000;
    pub const O_NONBLOCK: i32 = 0x800;
    pub const O_WRONLY: i32 = 1;
    pub const O_CREAT: i32 = 0x40;
    pub const O_EXCL: i32 = 0x80;
    pub const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
    pub const AT_REMOVEDIR: i32 = 0x200;
    pub const UTIME_OMIT: i64 = 0x3fff_fffe;

    #[repr(C)]
    pub struct timespec {
        pub tv_sec: i64,
        pub tv_nsec: i64,
    }

    extern "C" {
        fn __errno_location() -> *mut i32;
        pub fn openat(fd: i32, path: *const i8, flags: i32, ...) -> i32;
        pub fn renameat(from: i32, old: *const i8, to: i32, new: *const i8) -> i32;
        pub fn unlinkat(fd: i32, path: *const i8, flags: i32) -> i32;
        pub fn linkat(from: i32, old: *const i8, to: i32, new: *const i8, flags: i32) -> i32;
        pub fn fchown(fd: i32, uid: u32, gid: u32) -> i32;
        pub fn fchownat(fd: i32, path: *const i8, uid: u32, gid: u32, flags: i32) -> i32;
        pub fn utimensat(fd: i32, path: *const i8, times: *const timespec, flags: i32) -> i32;
    }

    // The static archive omits these hosted C symbols, but both kernels
    // expose the descriptor-relative syscalls.
    unsafe fn posix(value: u64) -> i64 {
        let value = value as i64;
        if value < 0 {
            unsafe {
                *__errno_location() = -value as i32;
            }
            -1
        } else {
            value
        }
    }

    pub unsafe fn fstatat(fd: i32, path: *const i8, out: *mut stat, flags: i32) -> i32 {
        unsafe {
            posix(raw::syscall4(
                nr::NEWFSTATAT,
                fd as u64,
                path as u64,
                out as u64,
                flags as u64,
            )) as i32
        }
    }

    pub unsafe fn mkdirat(fd: i32, path: *const i8, mode: u32) -> i32 {
        unsafe {
            posix(raw::syscall3(
                nr::MKDIRAT,
                fd as u64,
                path as u64,
                mode as u64,
            )) as i32
        }
    }

    pub unsafe fn symlinkat(target: *const i8, fd: i32, path: *const i8) -> i32 {
        unsafe {
            posix(raw::syscall3(
                nr::SYMLINKAT,
                target as u64,
                fd as u64,
                path as u64,
            )) as i32
        }
    }

    pub unsafe fn readlinkat(fd: i32, path: *const i8, out: *mut i8, size: usize) -> isize {
        unsafe {
            posix(raw::syscall4(
                nr::READLINKAT,
                fd as u64,
                path as u64,
                out as u64,
                size as u64,
            )) as isize
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Info {
    pub ino: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u64,
    pub size: u64,
}

impl Info {
    pub fn kind(self) -> u32 {
        self.mode & 0o170000
    }
}

pub(super) const REG: u32 = 0o100000;
pub(super) const DIR: u32 = 0o040000;
pub(super) const LINK: u32 = 0o120000;
const OPEN_DIR: i32 = sys::O_DIRECTORY | sys::O_NOFOLLOW | sys::O_CLOEXEC;

pub(super) struct Dir {
    pub file: File,
    dev: u64,
    #[cfg(feature = "paired-host-test")]
    _held: held::Held,
}

/// The host tests' model of BreenFS's held-directory rule: removing a directory that an
/// open descriptor still holds fails with EBUSY (the held-directory check in BreenFS's
/// `remove`, kernel/src/fs/breenfs/vfs.rs). POSIX allows that refusal, but Linux and APFS
/// remove such a directory, so without the model a host run passes code that the box
/// refuses. Each `Dir` counts as one hold on its directory for as long as it lives.
#[cfg(feature = "paired-host-test")]
mod held {
    use std::{collections::BTreeMap, sync::Mutex};

    static OPEN: Mutex<BTreeMap<(u64, u64), usize>> = Mutex::new(BTreeMap::new());

    pub(super) struct Held((u64, u64));

    impl Held {
        pub fn new(dev: u64, ino: u64) -> Self {
            *OPEN.lock().unwrap().entry((dev, ino)).or_default() += 1;
            Self((dev, ino))
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            let mut open = OPEN.lock().unwrap();
            if let Some(count) = open.get_mut(&self.0) {
                *count -= 1;
                if *count == 0 {
                    open.remove(&self.0);
                }
            }
        }
    }

    pub(super) fn is_held(dev: u64, ino: u64) -> bool {
        OPEN.lock().unwrap().contains_key(&(dev, ino))
    }
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
}

fn result(rc: i32) -> io::Result<()> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn owned(fd: i32) -> io::Result<File> {
    result(fd)?;
    // The successful open transferred descriptor ownership to the caller.
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Dir {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_DIR)
            .open(path)?;
        let dev = file.metadata()?.dev();
        Self::new(file, dev)
    }

    fn new(file: File, dev: u64) -> io::Result<Self> {
        #[cfg(feature = "paired-host-test")]
        let _held = held::Held::new(dev, file.metadata()?.ino());
        Ok(Self {
            file,
            dev,
            #[cfg(feature = "paired-host-test")]
            _held,
        })
    }

    fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    fn no_crossing(&self, dev: u64) -> io::Result<()> {
        if dev == self.dev {
            Ok(())
        } else {
            Err(io::Error::other("path crosses into another mount"))
        }
    }

    pub fn dir(&self, name: &OsStr, create: Option<u32>) -> io::Result<Self> {
        let name = cstr(name)?;
        let open = || owned(unsafe { sys::openat(self.fd(), name.as_ptr().cast(), OPEN_DIR) });
        let file = match open() {
            Err(err) if err.kind() == io::ErrorKind::NotFound && create.is_some() => {
                let status =
                    unsafe { sys::mkdirat(self.fd(), name.as_ptr().cast(), create.unwrap() as _) };
                if let Err(err) = result(status) {
                    if err.kind() != io::ErrorKind::AlreadyExists {
                        return Err(err);
                    }
                }
                open()?
            }
            other => other?,
        };
        self.no_crossing(file.metadata()?.dev())?;
        Self::new(file, self.dev)
    }

    pub fn parent<'a>(&self, path: &'a Path, create: bool) -> io::Result<(Self, &'a OsStr)> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let leaf = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let mut directory = Self::new(self.file.try_clone()?, self.dev)?;
        for part in parent.components() {
            let Component::Normal(name) = part else {
                return Err(io::ErrorKind::InvalidInput.into());
            };
            directory = directory
                .dir(name, create.then_some(0o755))
                .map_err(|err| io::Error::new(err.kind(), format!("{path:?}: {name:?}: {err}")))?;
        }
        Ok((directory, leaf))
    }

    pub fn info(&self, name: &OsStr) -> io::Result<Option<Info>> {
        let name = cstr(name)?;
        let mut data = std::mem::MaybeUninit::<sys::stat>::zeroed();
        let rc = unsafe {
            sys::fstatat(
                self.fd(),
                name.as_ptr().cast(),
                data.as_mut_ptr(),
                sys::AT_SYMLINK_NOFOLLOW,
            )
        };
        if let Err(err) = result(rc) {
            return if err.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(err)
            };
        }
        let data = unsafe { data.assume_init() };
        self.no_crossing(data.st_dev as u64)?;
        Ok(Some(Info {
            ino: data.st_ino as u64,
            mode: data.st_mode as u32,
            uid: data.st_uid,
            gid: data.st_gid,
            links: data.st_nlink as u64,
            size: data.st_size as u64,
        }))
    }

    pub fn lookup(&self, path: &Path) -> io::Result<Option<(Self, Info)>> {
        match self.parent(path, false) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
            Ok((dir, leaf)) => Ok(dir.info(leaf)?.map(|info| (dir, info))),
        }
    }

    pub fn read_file(&self, name: &OsStr) -> io::Result<File> {
        let name = cstr(name)?;
        // NONBLOCK prevents a substituted FIFO from parking the installer.
        let fd = unsafe {
            sys::openat(
                self.fd(),
                name.as_ptr().cast(),
                sys::O_NOFOLLOW | sys::O_CLOEXEC | sys::O_NONBLOCK,
            )
        };
        let file = owned(fd)?;
        let meta = file.metadata()?;
        self.no_crossing(meta.dev())?;
        if !meta.is_file() {
            return Err(io::Error::other("expected a regular file"));
        }
        Ok(file)
    }

    pub fn read(&self, name: &OsStr) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_file(name)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn read_link(&self, name: &OsStr) -> io::Result<Vec<u8>> {
        let name = cstr(name)?;
        let mut bytes = vec![0; 4096];
        let count = unsafe {
            sys::readlinkat(
                self.fd(),
                name.as_ptr().cast(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count as usize == bytes.len() {
            return Err(io::Error::other("symlink target is too long"));
        }
        bytes.truncate(count as usize);
        Ok(bytes)
    }

    pub fn rename(&self, from: &OsStr, to: &Self, name: &OsStr) -> io::Result<()> {
        let from = cstr(from)?;
        let name = cstr(name)?;
        result(unsafe {
            sys::renameat(
                self.fd(),
                from.as_ptr().cast(),
                to.fd(),
                name.as_ptr().cast(),
            )
        })
    }

    pub fn remove(&self, name: &OsStr, dir: bool) -> io::Result<()> {
        #[cfg(feature = "paired-host-test")]
        if dir
            && self
                .info(name)?
                .is_some_and(|info| held::is_held(self.dev, info.ino))
        {
            return Err(io::Error::from_raw_os_error(sys::EBUSY));
        }
        let name = cstr(name)?;
        result(unsafe {
            sys::unlinkat(
                self.fd(),
                name.as_ptr().cast(),
                if dir { sys::AT_REMOVEDIR } else { 0 },
            )
        })
    }

    pub fn remove_tree(&self, name: &OsStr) -> io::Result<()> {
        let Some(info) = self.info(name)? else {
            return Ok(());
        };
        if info.kind() == DIR {
            let child = self.dir(name, None)?;
            for entry in child.names()? {
                child.remove_tree(&entry)?;
            }
        }
        self.remove(name, info.kind() == DIR)
    }

    pub fn link(&self, from: &OsStr, to: &Self, name: &OsStr) -> io::Result<()> {
        let from = cstr(from)?;
        let name = cstr(name)?;
        result(unsafe {
            sys::linkat(
                self.fd(),
                from.as_ptr().cast(),
                to.fd(),
                name.as_ptr().cast(),
                0,
            )
        })
    }

    pub fn symlink(&self, target: &[u8], name: &OsStr) -> io::Result<()> {
        let target = CString::new(target).map_err(|_| io::ErrorKind::InvalidInput)?;
        let name = cstr(name)?;
        result(unsafe { sys::symlinkat(target.as_ptr().cast(), self.fd(), name.as_ptr().cast()) })
    }

    pub fn symlink_owner(&self, name: &OsStr, uid: u32, gid: u32) -> io::Result<()> {
        let name = cstr(name)?;
        result(unsafe {
            sys::fchownat(
                self.fd(),
                name.as_ptr().cast(),
                uid,
                gid,
                sys::AT_SYMLINK_NOFOLLOW,
            )
        })
    }

    pub fn mtime(&self, name: &OsStr, sec: i64, ns: i64) -> io::Result<()> {
        let name = cstr(name)?;
        let times = [
            sys::timespec {
                tv_sec: 0,
                tv_nsec: sys::UTIME_OMIT as _,
            },
            sys::timespec {
                tv_sec: sec as _,
                tv_nsec: ns as _,
            },
        ];
        result(unsafe {
            sys::utimensat(
                self.fd(),
                name.as_ptr().cast(),
                times.as_ptr(),
                sys::AT_SYMLINK_NOFOLLOW,
            )
        })
    }

    pub fn atomic(
        &self,
        name: &OsStr,
        build: impl FnOnce(&OsStr) -> io::Result<()>,
    ) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = format!(
            ".paired-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let tmp = OsStr::new(&tmp);
        build(tmp)?;
        if let Err(err) = self.rename(tmp, self, name) {
            let _ = self.remove(tmp, false);
            return Err(err);
        }
        self.file.sync_all()
    }

    pub fn create(&self, name: &OsStr) -> io::Result<File> {
        let name = cstr(name)?;
        owned(unsafe {
            sys::openat(
                self.fd(),
                name.as_ptr().cast(),
                sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_NOFOLLOW | sys::O_CLOEXEC,
                0o600_i32,
            )
        })
    }

    pub fn write(&self, name: &OsStr, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.atomic(name, |tmp| {
            let mut file = self.create(tmp)?;
            let result = file
                .write_all(bytes)
                .and_then(|()| file.set_permissions(std::fs::Permissions::from_mode(mode)))
                .and_then(|()| file.sync_all());
            if result.is_err() {
                let _ = self.remove(tmp, false);
            }
            result
        })
    }

    // Open a separate directory description so enumeration does not advance the caller's offset.
    pub fn names(&self) -> io::Result<Vec<OsString>> {
        let directory = self.dir(OsStr::new("."), None)?;
        names(directory.file)
    }
}

pub(super) fn owner_mode(file: &File, uid: u32, gid: u32, mode: u32) -> io::Result<()> {
    let meta = file.metadata()?;
    if (meta.uid(), meta.gid()) != (uid, gid) {
        result(unsafe { sys::fchown(file.as_raw_fd(), uid, gid) })?;
    }
    file.set_permissions(std::fs::Permissions::from_mode(mode))
}

#[cfg(target_os = "breenix")]
fn names(file: File) -> io::Result<Vec<OsString>> {
    use libbreenix::{
        error::Error,
        syscall::{nr, raw},
    };
    let mut buffer = [0u8; 8192];
    let mut names = Vec::new();
    loop {
        let rc = unsafe {
            raw::syscall3(
                nr::GETDENTS64,
                file.as_raw_fd() as u64,
                buffer.as_mut_ptr() as u64,
                buffer.len() as u64,
            )
        };
        let count = Error::from_syscall(rc as i64)
            .map_err(|err| io::Error::other(err.to_string()))? as usize;
        if count == 0 {
            return Ok(names);
        }
        let mut data = &buffer[..count];
        while !data.is_empty() {
            if data.len() < 20 {
                return Err(io::Error::other("invalid directory entry"));
            }
            let size = u16::from_ne_bytes([data[16], data[17]]) as usize;
            if !(20..=data.len()).contains(&size) {
                return Err(io::Error::other("invalid directory entry size"));
            }
            let name = bounded_name(&data[19..size]);
            collect_name(name, &mut names);
            data = &data[size..];
        }
    }
}

#[cfg(not(target_os = "breenix"))]
fn names(file: File) -> io::Result<Vec<OsString>> {
    use std::os::fd::IntoRawFd;
    let stream = unsafe { sys::fdopendir(file.as_raw_fd()) };
    if stream.is_null() {
        return Err(io::Error::last_os_error());
    }
    let _ = file.into_raw_fd(); // closedir owns it now.
    let mut names = Vec::new();
    loop {
        let entry = unsafe { sys::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let raw = unsafe { &(*entry).d_name };
        let bytes = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), raw.len()) };
        collect_name(bounded_name(bytes), &mut names);
    }
    result(unsafe { sys::closedir(stream) })?;
    Ok(names)
}

fn bounded_name(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

fn collect_name(bytes: &[u8], names: &mut Vec<OsString>) {
    if !matches!(bytes, b"" | b"." | b"..") {
        names.push(OsString::from_vec(bytes.to_vec()));
    }
}

#[cfg(all(test, feature = "paired-host-test"))]
mod held_tests {
    use super::Dir;
    use std::{ffi::OsStr, fs, io};

    /// The model the host tests rely on: while a `Dir` holds a directory its removal
    /// fails with EBUSY, as on BreenFS, and once the handle is dropped it succeeds.
    #[test]
    fn a_held_directory_is_removed_only_after_its_handle_closes() {
        let base = std::env::temp_dir().join(format!("paired-held-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("victim")).unwrap();
        let parent = Dir::open(&base).unwrap();
        let held = parent.dir(OsStr::new("victim"), None).unwrap();
        let refused = parent.remove(OsStr::new("victim"), true).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::EBUSY));
        drop(held);
        parent.remove(OsStr::new("victim"), true).unwrap();
        assert_eq!(
            fs::metadata(base.join("victim")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        drop(parent);
        fs::remove_dir(&base).unwrap();
    }
}
