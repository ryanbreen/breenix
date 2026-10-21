//! lckpwdf/ulckpwdf share the account tools' /etc/.pwd.lock record lock.
use core::sync::atomic::{AtomicI32, Ordering};
static LOCK_FD: AtomicI32 = AtomicI32::new(-1);
#[no_mangle]
pub unsafe extern "C" fn lckpwdf() -> i32 {
    // Reserve the per-process slot before opening: POSIX close of *any* fd
    // for the inode would otherwise release a lock already held by this process.
    if LOCK_FD
        .compare_exchange(-1, -2, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        crate::pthread::set_errno(16);
        return -1;
    }
    #[cfg(target_arch = "x86_64")]
    const NOFOLLOW: i32 = 0o400000;
    #[cfg(target_arch = "aarch64")]
    const NOFOLLOW: i32 = 0o100000;
    let fd = crate::open(
        b"/etc/.pwd.lock\0".as_ptr(),
        2 | 0o100 | NOFOLLOW | 0o2000000,
        0o600,
    );
    if fd < 0 {
        LOCK_FD.store(-1, Ordering::Release);
        return -1;
    }
    let mut stat = libbreenix::fs::Stat::default();
    if crate::fstat(fd, &mut stat as *mut _ as *mut u8) != 0
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode != 0o100600
        || stat.st_nlink != 1
    {
        crate::close(fd);
        LOCK_FD.store(-1, Ordering::Release);
        crate::pthread::set_errno(13);
        return -1;
    }
    let mut record = libbreenix::accounts::PasswordLock::write();
    // F_SETLK polling bounds the wait to 15 seconds (the lckpwdf interface).
    let mut start = [0i64; 2];
    if crate::clock_gettime(1, start.as_mut_ptr() as *mut u8) != 0 {
        crate::close(fd);
        LOCK_FD.store(-1, Ordering::Release);
        return -1;
    }
    loop {
        if crate::fcntl(fd, 6, &mut record as *mut _ as u64) == 0 {
            LOCK_FD.store(fd, Ordering::Release);
            return 0;
        }
        let error = crate::pthread::errno();
        if error != 11 && error != 13 {
            crate::close(fd);
            LOCK_FD.store(-1, Ordering::Release);
            crate::pthread::set_errno(error);
            return -1;
        }
        let mut now = [0i64; 2];
        if crate::clock_gettime(1, now.as_mut_ptr() as *mut u8) != 0 || now[0] - start[0] >= 15 {
            crate::close(fd);
            LOCK_FD.store(-1, Ordering::Release);
            crate::pthread::set_errno(11);
            return -1;
        }
        let delay = [0i64, 10_000_000];
        crate::nanosleep(delay.as_ptr() as *const u8, core::ptr::null_mut());
    }
}
#[no_mangle]
pub unsafe extern "C" fn ulckpwdf() -> i32 {
    let fd = LOCK_FD.load(Ordering::Acquire);
    if fd < 0 {
        crate::pthread::set_errno(22);
        return -1;
    }
    // Keep the slot reserved until close has released the inode's record lock.
    if LOCK_FD
        .compare_exchange(fd, -2, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        crate::pthread::set_errno(16);
        return -1;
    }
    let result = crate::close(fd);
    LOCK_FD.store(-1, Ordering::Release);
    result
}
