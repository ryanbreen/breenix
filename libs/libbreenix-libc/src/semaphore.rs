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
    pshared: u32,
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
        if self.pshared == 0 {
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
        pshared: u32::from(pshared != 0),
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
    sem.waiters.fetch_add(1, Ordering::SeqCst);
    let _waiting = Waiting(sem);
    loop {
        if sem.take() {
            return 0;
        }
        let op = if timed {
            FUTEX_WAIT_BITSET | FUTEX_CLOCK_REALTIME
        } else {
            FUTEX_WAIT
        };
        let result = futex(&sem.value, op | sem.private(), 0, timeout);
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
