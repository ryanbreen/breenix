//! The layout of every struct std passes to the kernel or to libbreenix-libc, pinned
//! per architecture to the definition on the other side of the call: a size and the
//! offset of each field, checked when the crate compiles for that architecture.
//! tests/libc_crate_layout_test.py compiles it for both.
//!
//! Where each layout comes from:
//! - stat, stat64: the kernel's `Stat` (kernel/src/syscall/fs.rs), Linux's per-arch
//!   uapi stat (x86_64 asm/stat.h, aarch64 asm-generic/stat.h).
//! - epoll_event: the kernel's `EpollEvent` (kernel/src/syscall/epoll.rs) and
//!   libbreenix-libc's, packed on x86_64.
//! - dirent: libbreenix-libc's `struct dirent` (include/dirent.h), which readdir fills.
//! - sigaction: the C `struct sigaction` that libbreenix-libc's sigaction converts
//!   (src/lib.rs); stack_t: include/signal.h and the kernel's sigaltstack.
//! - msghdr, cmsghdr: the kernel's sendmsg and recvmsg (kernel/src/syscall/socket.rs:
//!   msg_namelen at 8, msg_controllen at 40, msg_flags at 48, a 16-byte cmsghdr).
//! - sockaddr, sockaddr_in, sockaddr_un: kernel/src/socket/types.rs; sockaddr_in6 and
//!   sockaddr_storage: Linux's.
//! - timeval, linger: the kernel's SO_RCVTIMEO/SO_SNDTIMEO and SO_LINGER
//!   (kernel/src/socket/opts.rs); timespec, iovec, pollfd: the kernel's syscalls.
//! - cpu_set_t: 128-byte public mask; libc zero-fills past the kernel's 8 bytes.

use super::*;
use crate::{iovec, linger, pollfd, timespec, timeval};

macro_rules! pin {
    ($ty:ty, $size:expr $(, $field:ident @ $offset:expr)* $(,)?) => {
        const _: () = core::assert!(core::mem::size_of::<$ty>() == $size);
        $(const _: () = core::assert!(core::mem::offset_of!($ty, $field) == $offset);)*
    };
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::*;

    pin!(stat, 144, st_dev @ 0, st_ino @ 8, st_nlink @ 16, st_mode @ 24, st_uid @ 28,
         st_gid @ 32, st_rdev @ 40, st_size @ 48, st_blksize @ 56, st_blocks @ 64,
         st_atime @ 72, st_atime_nsec @ 80, st_mtime @ 88, st_mtime_nsec @ 96,
         st_ctime @ 104, st_ctime_nsec @ 112);
    pin!(stat64, 144, st_dev @ 0, st_ino @ 8, st_nlink @ 16, st_mode @ 24, st_uid @ 28,
         st_gid @ 32, st_rdev @ 40, st_size @ 48, st_blksize @ 56, st_blocks @ 64,
         st_atime @ 72, st_atime_nsec @ 80, st_mtime @ 88, st_mtime_nsec @ 96,
         st_ctime @ 104, st_ctime_nsec @ 112);
    pin!(epoll_event, 12, events @ 0, u64 @ 4);
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::*;

    pin!(stat, 128, st_dev @ 0, st_ino @ 8, st_mode @ 16, st_nlink @ 20, st_uid @ 24,
         st_gid @ 28, st_rdev @ 32, st_size @ 48, st_blksize @ 56, st_blocks @ 64,
         st_atime @ 72, st_atime_nsec @ 80, st_mtime @ 88, st_mtime_nsec @ 96,
         st_ctime @ 104, st_ctime_nsec @ 112);
    pin!(stat64, 128, st_dev @ 0, st_ino @ 8, st_mode @ 16, st_nlink @ 20, st_uid @ 24,
         st_gid @ 28, st_rdev @ 32, st_size @ 48, st_blksize @ 56, st_blocks @ 64,
         st_atime @ 72, st_atime_nsec @ 80, st_mtime @ 88, st_mtime_nsec @ 96,
         st_ctime @ 104, st_ctime_nsec @ 112);
    pin!(epoll_event, 16, events @ 0, u64 @ 8);
}

// The same on both architectures.
pin!(dirent, 280, d_ino @ 0, d_off @ 8, d_reclen @ 16, d_type @ 18, d_name @ 19);
pin!(sigaction, 32, sa_sigaction @ 0, sa_mask @ 8, sa_flags @ 16, sa_restorer @ 24);
pin!(stack_t, 24, ss_sp @ 0, ss_flags @ 8, ss_size @ 16);
pin!(msghdr, 56, msg_name @ 0, msg_namelen @ 8, msg_iov @ 16, msg_iovlen @ 24,
     msg_control @ 32, msg_controllen @ 40, msg_flags @ 48);
pin!(cmsghdr, 16, cmsg_len @ 0, cmsg_level @ 8, cmsg_type @ 12);
pin!(sockaddr, 16, sa_family @ 0, sa_data @ 2);
pin!(sockaddr_in, 16, sin_family @ 0, sin_port @ 2, sin_addr @ 4, sin_zero @ 8);
pin!(sockaddr_in6, 28, sin6_family @ 0, sin6_port @ 2, sin6_flowinfo @ 4, sin6_addr @ 8,
     sin6_scope_id @ 24);
pin!(sockaddr_un, 110, sun_family @ 0, sun_path @ 2);
pin!(sockaddr_storage, 128, ss_family @ 0);
pin!(timespec, 16, tv_sec @ 0, tv_nsec @ 8);
pin!(timeval, 16, tv_sec @ 0, tv_usec @ 8);
pin!(linger, 8, l_onoff @ 0, l_linger @ 4);
pin!(iovec, 16, iov_base @ 0, iov_len @ 8);
pin!(pollfd, 8, fd @ 0, events @ 4, revents @ 6);
pin!(cpu_set_t, 128);
