//! System V IPC: ftok and the msg, sem and shm calls.
//!
//! Written from POSIX.1-2024 (sys/ipc.h, sys/msg.h, sys/sem.h, sys/shm.h)
//! and Linux's system call ABI: each call but ftok is one system call, and
//! the kernel's structures are Linux's 64-bit ones.

/// ftok: a key from the file `path` names and the low eight bits of `id`.
/// The same file and id always give the same key: the id in bits 24-31, the
/// low eight bits of the file's device in bits 16-23 and the low sixteen
/// bits of its inode number below them. A path stat cannot resolve returns
/// -1 with stat's errno.
#[no_mangle]
pub unsafe extern "C" fn ftok(path: *const u8, id: i32) -> i32 {
    // struct stat begins with st_dev and st_ino, 64 bits each, on both
    // architectures.
    let mut st = [0u64; 32];
    if crate::stat(path, st.as_mut_ptr() as *mut u8) != 0 {
        return -1;
    }
    let (dev, ino) = (st[0] as u32, st[1] as u32);
    (((id as u32 & 0xff) << 24) | ((dev & 0xff) << 16) | (ino & 0xffff)) as i32
}
