//! Clean-room POSIX.1-2024 semaphores, using the Linux futex ABI.
use core::ptr::null;
use core::sync::atomic::{AtomicU32, Ordering};
use libbreenix::{raw, syscall::nr};

const EAGAIN: i64 = 11;
const EINVAL: i64 = 22;
const EOVERFLOW: i64 = 75;
pub const SEM_VALUE_MAX: u32 = 0x7fff_ffff;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_PRIVATE_FLAG: u64 = 128;
const FUTEX_CLOCK_REALTIME: u64 = 256;

/// Linux-compatible opaque storage size; field offsets are Breenix's ABI.
#[repr(C)]
pub struct Sem {
    value: AtomicU32,
    waiters: AtomicU32,
    pshared: AtomicU32,
    ready: AtomicU32,
    padding: [u8; 16],
}

unsafe fn futex(word: &AtomicU32, op: u64, expected: u32, timeout: *const i64) -> i64 {
    raw::syscall6(
        nr::FUTEX,
        word as *const AtomicU32 as u64,
        op,
        expected as u64,
        timeout as u64,
        0,
        u32::MAX as u64,
    ) as i64
}
fn fail(errno: i64) -> i32 {
    crate::syscall_result_to_c_int(-errno)
}
unsafe fn checked<'a>(sem: *mut Sem) -> Result<&'a Sem, i64> {
    if sem.is_null() || sem as usize % core::mem::align_of::<Sem>() != 0 {
        return Err(EINVAL);
    }
    let sem = &*sem;
    if sem.ready.load(Ordering::Acquire) != 1 {
        return Err(EINVAL);
    }
    Ok(sem)
}
impl Sem {
    fn private(&self) -> u64 {
        if self.pshared.load(Ordering::Relaxed) == 0 {
            FUTEX_PRIVATE_FLAG
        } else {
            0
        }
    }
    fn take(&self) -> bool {
        let mut value = self.value.load(Ordering::SeqCst);
        while value != 0 {
            match self.value.compare_exchange_weak(
                value,
                value - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(seen) => value = seen,
            }
        }
        false
    }
}

#[no_mangle]
pub unsafe extern "C" fn sem_init(sem: *mut Sem, pshared: i32, value: u32) -> i32 {
    if value > SEM_VALUE_MAX || sem.is_null() || sem as usize % core::mem::align_of::<Sem>() != 0 {
        return fail(EINVAL);
    }
    sem.write(Sem {
        value: AtomicU32::new(value),
        waiters: AtomicU32::new(0),
        pshared: AtomicU32::new(u32::from(pshared != 0)),
        ready: AtomicU32::new(1),
        padding: [0; 16],
    });
    0
}
#[no_mangle]
pub unsafe extern "C" fn sem_destroy(sem: *mut Sem) -> i32 {
    let sem = match checked(sem) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    sem.ready.store(0, Ordering::Release);
    0
}
#[no_mangle]
pub unsafe extern "C" fn sem_trywait(sem: *mut Sem) -> i32 {
    let sem = match checked(sem) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if sem.take() {
        0
    } else {
        fail(EAGAIN)
    }
}
#[no_mangle]
pub unsafe extern "C" fn sem_post(sem: *mut Sem) -> i32 {
    let sem = match checked(sem) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let mut value = sem.value.load(Ordering::SeqCst);
    loop {
        if value == SEM_VALUE_MAX {
            return fail(EOVERFLOW);
        }
        match sem
            .value
            .compare_exchange_weak(value, value + 1, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => break,
            Err(seen) => value = seen,
        }
    }
    // SeqCst pairs registration and the slow-path recheck with publication:
    // either the recheck sees the token or post sees a registered waiter.
    if sem.waiters.load(Ordering::SeqCst) != 0 {
        futex(&sem.value, FUTEX_WAKE | sem.private(), 1, null());
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn sem_getvalue(sem: *mut Sem, value: *mut i32) -> i32 {
    let sem = match checked(sem) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if value.is_null() {
        return fail(EINVAL);
    }
    value.write(sem.value.load(Ordering::Relaxed) as i32);
    0
}
struct Waiting<'a>(&'a Sem);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.waiters.fetch_sub(1, Ordering::SeqCst);
    }
}
unsafe fn wait(sem: *mut Sem, timeout: *const i64, timed: bool) -> i32 {
    let sem = match checked(sem) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if sem.take() {
        return 0;
    }
    if timed && (timeout.is_null() || !(0..1_000_000_000).contains(&*timeout.add(1))) {
        return fail(EINVAL);
    }
    loop {
        // Short critical sections normally release on another CPU before a
        // futex sleep can return. Bound this work, then use the same SeqCst
        // registration/recheck protocol; an idle semaphore still parks.
        for _ in 0..128 {
            if sem.take() {
                return 0;
            }
            core::hint::spin_loop();
        }
        sem.waiters.fetch_add(1, Ordering::SeqCst);
        let waiting = Waiting(sem);
        if sem.take() {
            return 0;
        }
        let op = if timed {
            FUTEX_WAIT_BITSET | FUTEX_CLOCK_REALTIME
        } else {
            FUTEX_WAIT
        };
        let result = futex(&sem.value, op | sem.private(), 0, timeout);
        drop(waiting);
        if result < 0 && result != -EAGAIN {
            return crate::syscall_result_to_c_int(result);
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn sem_wait(sem: *mut Sem) -> i32 {
    wait(sem, null(), false)
}
#[no_mangle]
pub unsafe extern "C" fn sem_timedwait(sem: *mut Sem, abstime: *const i64) -> i32 {
    wait(sem, abstime, true)
}

const O_RDWR: i32 = 2;
const O_CREAT: i32 = 0x40;
const O_EXCL: i32 = 0x80;
const EEXIST: i64 = 17;
const ENOMEM: i64 = 12;
const SEM_MAP_LEN: usize = 4096;

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;

// Published Linux ABI: inode at 8, size at 48 on both 64-bit arches. Opaque
// metadata surrounds the fields used here; storage covers either stat layout.
#[repr(C)]
struct NamedStat {
    dev: u64,
    ino: u64,
    metadata: [u8; 32],
    size: i64,
    rest: [u8; 88],
}
impl NamedStat {
    fn zeroed() -> Self {
        Self {
            dev: 0,
            ino: 0,
            metadata: [0; 32],
            size: 0,
            rest: [0; 88],
        }
    }
}
const _: () = assert!(core::mem::offset_of!(NamedStat, ino) == 8);
const _: () = assert!(core::mem::offset_of!(NamedStat, size) == 48);
const _: () = assert!(core::mem::size_of::<NamedStat>() == 144);

#[derive(Clone, Copy)]
struct NamedEntry {
    ino: u64,
    addr: *mut Sem,
    refs: usize,
}
struct NamedTable {
    entries: *mut NamedEntry,
    len: usize,
    capacity: usize,
}
struct TableCell(core::cell::UnsafeCell<NamedTable>);
// Access is serialized by TABLE_LOCK; fork copies the private table.
unsafe impl Sync for TableCell {}
static TABLE: TableCell = TableCell(core::cell::UnsafeCell::new(NamedTable {
    entries: core::ptr::null_mut(),
    len: 0,
    capacity: 0,
}));
// The holder's PID doubles as a fork marker: the child can take an inherited
// lock held by a vanished parent thread without waiting for that thread.
static TABLE_LOCK: AtomicU32 = AtomicU32::new(0);
struct TableGuard;
impl TableGuard {
    unsafe fn lock() -> Self {
        let pid = raw::syscall0(nr::GETPID) as u32;
        loop {
            let owner = TABLE_LOCK.load(Ordering::Relaxed);
            if owner != pid {
                if TABLE_LOCK
                    .compare_exchange(owner, pid, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return Self;
                }
            } else {
                futex(&TABLE_LOCK, FUTEX_WAIT | FUTEX_PRIVATE_FLAG, owner, null());
            }
        }
    }
}
impl Drop for TableGuard {
    fn drop(&mut self) {
        TABLE_LOCK.store(0, Ordering::Release);
        unsafe {
            futex(&TABLE_LOCK, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1, null());
        }
    }
}
impl NamedTable {
    unsafe fn retain(&mut self, ino: u64, addr: *mut Sem) -> Result<*mut Sem, i64> {
        for index in 0..self.len {
            let entry = &mut *self.entries.add(index);
            if entry.ino == ino {
                entry.refs = entry.refs.checked_add(1).ok_or(ENOMEM)?;
                return Ok(entry.addr);
            }
        }
        if self.len == self.capacity {
            let capacity = self.capacity.max(4).checked_mul(2).ok_or(ENOMEM)?;
            let bytes = capacity
                .checked_mul(core::mem::size_of::<NamedEntry>())
                .ok_or(ENOMEM)?;
            let entries = crate::realloc(self.entries as *mut u8, bytes) as *mut NamedEntry;
            if entries.is_null() {
                return Err(ENOMEM);
            }
            self.entries = entries;
            self.capacity = capacity;
        }
        self.entries
            .add(self.len)
            .write(NamedEntry { ino, addr, refs: 1 });
        self.len += 1;
        Ok(addr)
    }
    unsafe fn release(&mut self, addr: *mut Sem) -> Result<(), i64> {
        for index in 0..self.len {
            let entry = &mut *self.entries.add(index);
            if entry.addr == addr {
                if entry.refs > 1 {
                    entry.refs -= 1;
                    return Ok(());
                }
                if crate::munmap(addr as *mut u8, SEM_MAP_LEN) != 0 {
                    return Err(i64::from(*crate::__errno_location()));
                }
                self.len -= 1;
                if index != self.len {
                    self.entries.add(index).write(*self.entries.add(self.len));
                }
                return Ok(());
            }
        }
        Err(EINVAL)
    }
}

unsafe fn named_open(name: *const u8, oflag: i32, mode: u32, value: u32) -> Result<*mut Sem, i64> {
    use core::ptr::null_mut;
    let mut path = [0u8; 265];
    crate::shm::name_with_prefix(name, b"/dev/shm/sem.", &mut path)?;
    loop {
        // First use an existing initialized object, except for O_EXCL creators.
        let existing = if oflag & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
            -1
        } else {
            crate::open(path.as_ptr(), O_RDWR | 0x80000, 0)
        };
        let created = existing < 0;
        if created && oflag & O_CREAT == 0 {
            return Err(i64::from(*crate::__errno_location()));
        }
        if created && oflag & O_EXCL == 0 && *crate::__errno_location() != 2 {
            return Err(i64::from(*crate::__errno_location()));
        }
        if created && value > SEM_VALUE_MAX {
            return Err(EINVAL);
        }
        #[cfg(target_arch = "x86_64")]
        const O_TMPFILE: i32 = 0x410000;
        #[cfg(target_arch = "aarch64")]
        const O_TMPFILE: i32 = 0x404000;
        let fd = if created {
            crate::open(b"/dev/shm\0".as_ptr(), O_RDWR | O_TMPFILE | 0x80000, mode)
        } else {
            existing
        };
        if fd < 0 {
            return Err(i64::from(*crate::__errno_location()));
        }
        let result = (|| {
            if created && crate::ftruncate(fd, core::mem::size_of::<Sem>() as i64) != 0 {
                return Err(i64::from(*crate::__errno_location()));
            }
            let mut stat = NamedStat::zeroed();
            if crate::fstat(fd, &mut stat as *mut NamedStat as *mut u8) != 0 {
                return Err(i64::from(*crate::__errno_location()));
            }
            if stat.size < core::mem::size_of::<Sem>() as i64 {
                return Err(EINVAL);
            }
            let addr = crate::mmap(
                null_mut(),
                SEM_MAP_LEN,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            ) as *mut Sem;
            if addr as usize == usize::MAX {
                return Err(i64::from(*crate::__errno_location()));
            }
            let initialize = (|| {
                if created {
                    sem_init(addr, 1, value);
                    // Atomically publish the fully initialized unnamed file.
                    // Closing a failed/dead creator releases it without an
                    // unlink by name, so no replacement can be removed.
                    if crate::linkat(fd, b"\0".as_ptr(), -100, path.as_ptr(), 0x1000) != 0 {
                        return Err(i64::from(*crate::__errno_location()));
                    }
                } else if (*addr).ready.load(Ordering::Acquire) != 1 {
                    return Err(EINVAL);
                }
                Ok(())
            })();
            if let Err(e) = initialize {
                crate::munmap(addr as *mut u8, SEM_MAP_LEN);
                return Err(e);
            }
            let _guard = TableGuard::lock();
            match (&mut *TABLE.0.get()).retain(stat.ino, addr) {
                Ok(existing) => {
                    if existing != addr {
                        crate::munmap(addr as *mut u8, SEM_MAP_LEN);
                    }
                    Ok(existing)
                }
                Err(e) => {
                    crate::munmap(addr as *mut u8, SEM_MAP_LEN);
                    Err(e)
                }
            }
        })();
        crate::close(fd);
        if result == Err(EEXIST) && oflag & O_EXCL == 0 {
            continue;
        }
        return result;
    }
}

#[no_mangle]
pub unsafe extern "C" fn sem_open(name: *const u8, oflag: i32, mut args: ...) -> *mut Sem {
    let (mode, value) = if oflag & O_CREAT != 0 {
        (args.arg::<u32>(), args.arg::<u32>())
    } else {
        (0, 0)
    };
    match named_open(name, oflag, mode, value) {
        Ok(addr) => addr,
        Err(e) => {
            fail(e);
            core::ptr::null_mut()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn sem_close(sem: *mut Sem) -> i32 {
    let _guard = TableGuard::lock();
    match (&mut *TABLE.0.get()).release(sem) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}
#[no_mangle]
pub unsafe extern "C" fn sem_unlink(name: *const u8) -> i32 {
    let mut shm_name = [0u8; 257];
    if let Err(e) = crate::shm::name_with_prefix(name, b"/sem.", &mut shm_name) {
        return fail(e);
    }
    crate::shm::shm_unlink(shm_name.as_ptr())
}
