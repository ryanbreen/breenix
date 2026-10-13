//! Networking & sockets: UDP, TCP and Unix-domain sockets, socket options and shutdown,
//! poll, select and epoll on sockets, and name resolution, as POSIX.1-2024 specifies them
//! (sys/socket.h, netinet/in.h, sys/un.h, netdb.h, poll.h, sys/select.h) and as Linux's
//! ABI fixes them where it is stricter or POSIX leaves them open: errno values, the SO_*
//! and TCP_* options, the MSG_* flags, SOCK_NONBLOCK and SOCK_CLOEXEC, and abstract Unix
//! addresses. Where Linux deliberately departs from POSIX a case follows Linux and its
//! comment says so.
//!
//! Sockets are measured through the interface a portable C program uses. A function
//! Breenix's own C library (libs/libbreenix-libc) defines is called there: build.rs reads
//! libc.a's symbol index and sets `libc_has = "<name>"` for each one it has. A function
//! the library lacks that Linux implements as a system call (socket, accept4, sendmsg,
//! the epoll calls and the rest) is made by its Linux number, as a C library makes it,
//! so a kernel that lacks it fails the case with ENOSYS. A function a C library builds
//! from other calls (getaddrinfo, getnameinfo, gai_strerror, inet_pton, inet_ntop) fails
//! each case that needs it with `the C library has no <name>` while the library lacks it.
//! The suite uses raw system calls otherwise only to arrange a case: forking and reaping,
//! clocks and process identity.
//!
//! Cases use the loopback address 127.0.0.1, Unix paths under /tmp and abstract Unix
//! names, so they need no outside network. The two resolve cases whose ids begin
//! `gateway-` reach the nameserver in /etc/resolv.conf and require outside networking;
//! failures are bounded to a few seconds. Each case process takes its
//! TCP and UDP ports from 20000-29999, below Linux's ephemeral range, numbered from its
//! process ID so no two cases share one.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Waits for data poll the socket with MSG_DONTWAIT and nap between looks, and every wait
//! stops 1.5 seconds before the case's deadline, so on a kernel that honours MSG_DONTWAIT
//! a case whose data never comes fails saying what is missing; on one that ignores it the
//! receive blocks and the runner fails the case at its deadline. A call whose point is that it blocks (a receive timeout,
//! a refused blocking connect) runs in a process of its own the case waits on and kills.
//! Every Unix path a case binds is unlinked when the case ends, and a case that edits
//! /etc/hosts puts it back; the next resolve case restores it if a killed case could not.
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self as signal, Sigaction};
use libbreenix::suite::{case, case_ms_left, category, check, fail, suite, value, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU8, Ordering::Relaxed, Ordering::SeqCst};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const MMAP: u64 = 9;
    pub const NANOSLEEP: u64 = 35;
    pub const GETPID: u64 = 39;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const GETUID: u64 = 102;
    pub const GETGID: u64 = 104;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    #[cfg(not(libc_has = "socket"))]
    pub const SOCKET: u64 = 41;
    #[cfg(not(libc_has = "connect"))]
    pub const CONNECT: u64 = 42;
    #[cfg(not(libc_has = "accept"))]
    pub const ACCEPT: u64 = 43;
    #[cfg(not(all(libc_has = "sendto", libc_has = "send")))]
    pub const SENDTO: u64 = 44;
    #[cfg(not(all(libc_has = "recvfrom", libc_has = "recv")))]
    pub const RECVFROM: u64 = 45;
    #[cfg(not(libc_has = "sendmsg"))]
    pub const SENDMSG: u64 = 46;
    #[cfg(not(libc_has = "recvmsg"))]
    pub const RECVMSG: u64 = 47;
    #[cfg(not(libc_has = "shutdown"))]
    pub const SHUTDOWN: u64 = 48;
    #[cfg(not(libc_has = "bind"))]
    pub const BIND: u64 = 49;
    #[cfg(not(libc_has = "listen"))]
    pub const LISTEN: u64 = 50;
    #[cfg(not(libc_has = "getsockname"))]
    pub const GETSOCKNAME: u64 = 51;
    #[cfg(not(libc_has = "getpeername"))]
    pub const GETPEERNAME: u64 = 52;
    #[cfg(not(libc_has = "socketpair"))]
    pub const SOCKETPAIR: u64 = 53;
    #[cfg(not(libc_has = "setsockopt"))]
    pub const SETSOCKOPT: u64 = 54;
    #[cfg(not(libc_has = "getsockopt"))]
    pub const GETSOCKOPT: u64 = 55;
    #[cfg(not(libc_has = "fcntl"))]
    pub const FCNTL: u64 = 72;
    #[cfg(not(libc_has = "epoll_ctl"))]
    pub const EPOLL_CTL: u64 = 233;
    #[cfg(not(libc_has = "epoll_wait"))]
    pub const EPOLL_PWAIT: u64 = 281;
    #[cfg(not(libc_has = "accept4"))]
    pub const ACCEPT4: u64 = 288;
    #[cfg(not(libc_has = "epoll_create1"))]
    pub const EPOLL_CREATE1: u64 = 291;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const MMAP: u64 = 222;
    pub const NANOSLEEP: u64 = 101;
    pub const GETPID: u64 = 172;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const GETUID: u64 = 174;
    pub const GETGID: u64 = 176;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const EXIT_GROUP: u64 = 94;
    #[cfg(not(libc_has = "socket"))]
    pub const SOCKET: u64 = 198;
    #[cfg(not(libc_has = "connect"))]
    pub const CONNECT: u64 = 203;
    #[cfg(not(libc_has = "accept"))]
    pub const ACCEPT: u64 = 202;
    #[cfg(not(all(libc_has = "sendto", libc_has = "send")))]
    pub const SENDTO: u64 = 206;
    #[cfg(not(all(libc_has = "recvfrom", libc_has = "recv")))]
    pub const RECVFROM: u64 = 207;
    #[cfg(not(libc_has = "sendmsg"))]
    pub const SENDMSG: u64 = 211;
    #[cfg(not(libc_has = "recvmsg"))]
    pub const RECVMSG: u64 = 212;
    #[cfg(not(libc_has = "shutdown"))]
    pub const SHUTDOWN: u64 = 210;
    #[cfg(not(libc_has = "bind"))]
    pub const BIND: u64 = 200;
    #[cfg(not(libc_has = "listen"))]
    pub const LISTEN: u64 = 201;
    #[cfg(not(libc_has = "getsockname"))]
    pub const GETSOCKNAME: u64 = 204;
    #[cfg(not(libc_has = "getpeername"))]
    pub const GETPEERNAME: u64 = 205;
    #[cfg(not(libc_has = "socketpair"))]
    pub const SOCKETPAIR: u64 = 199;
    #[cfg(not(libc_has = "setsockopt"))]
    pub const SETSOCKOPT: u64 = 208;
    #[cfg(not(libc_has = "getsockopt"))]
    pub const GETSOCKOPT: u64 = 209;
    #[cfg(not(libc_has = "fcntl"))]
    pub const FCNTL: u64 = 25;
    #[cfg(not(libc_has = "epoll_ctl"))]
    pub const EPOLL_CTL: u64 = 21;
    #[cfg(not(libc_has = "epoll_wait"))]
    pub const EPOLL_PWAIT: u64 = 22;
    #[cfg(not(libc_has = "accept4"))]
    pub const ACCEPT4: u64 = 242;
    #[cfg(not(libc_has = "epoll_create1"))]
    pub const EPOLL_CREATE1: u64 = 20;
}

const EINTR: i64 = 4;
const EAGAIN: i64 = 11;
const EACCES: i64 = 13;
const EINVAL: i64 = 22;
const ENOSPC: i64 = 28;
const EPIPE: i64 = 32;
const EDOM: i64 = 33;
const ENOENT: i64 = 2;
const ENOTSOCK: i64 = 88;
const EDESTADDRREQ: i64 = 89;
const EMSGSIZE: i64 = 90;
const EPROTOTYPE: i64 = 91;
const ENOPROTOOPT: i64 = 92;
const EPROTONOSUPPORT: i64 = 93;
const EOPNOTSUPP: i64 = 95;
const EAFNOSUPPORT: i64 = 97;
const EADDRINUSE: i64 = 98;
const EADDRNOTAVAIL: i64 = 99;
const ECONNRESET: i64 = 104;
const EISCONN: i64 = 106;
const ENOTCONN: i64 = 107;
const ECONNREFUSED: i64 = 111;
const EINPROGRESS: i64 = 115;

const AF_UNSPEC: i32 = 0;
const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_SEQPACKET: i32 = 5;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;

const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_BROADCAST: i32 = 6;
const SO_SNDBUF: i32 = 7;
const SO_RCVBUF: i32 = 8;
const SO_KEEPALIVE: i32 = 9;
const SO_LINGER: i32 = 13;
const SO_PEERCRED: i32 = 17;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;
const SO_ACCEPTCONN: i32 = 30;
const SO_PROTOCOL: i32 = 38;
const SO_DOMAIN: i32 = 39;
const TCP_NODELAY: i32 = 1;
const SCM_RIGHTS: i32 = 1;

const MSG_PEEK: i32 = 2;
const MSG_TRUNC: i32 = 0x20;
const MSG_DONTWAIT: i32 = 0x40;
const MSG_WAITALL: i32 = 0x100;
const MSG_NOSIGNAL: i32 = 0x4000;

const SHUT_RD: i32 = 0;
const SHUT_WR: i32 = 1;
const SHUT_RDWR: i32 = 2;

const F_GETFD: i32 = 1;
const F_GETFL: i32 = 3;
const FD_CLOEXEC: i64 = 1;
const O_NONBLOCK: i64 = 0o4000;
const O_WRONLY: i32 = 1;
const O_CREAT: i32 = 0o100;
const O_TRUNC: i32 = 0o1000;

const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 0x10;
const POLLRDHUP: i16 = 0x2000;

const EPOLLIN: u32 = 1;
const EPOLLHUP: u32 = 0x10;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_MOD: i32 = 3;

const S_IFMT: u32 = 0o170000;
const S_IFSOCK: u32 = 0o140000;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const MAP_SHARED_ANON: u64 = 0x21;
const SIGKILL: i32 = 9;
const SIGPIPE: i32 = 13;
const WNOHANG: u64 = 1;

/// The loopback address, a documentation address no interface has (RFC 5737), and the
/// limited broadcast address, in host order.
const LOOPBACK: u32 = 0x7f00_0001;
const TEST_NET: u32 = 0xc000_0201;
const BROADCAST: u32 = 0xffff_ffff;
/// The QEMU user network's DNS server.

const CLOCK_MONOTONIC: i32 = 1;
const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// The timer tick: 200 Hz on x86-64 and 1000 Hz on ARM64. A timed wait may end late by
/// at most two ticks and 20 ms, as the time suite holds its sleeps to.
#[cfg(target_arch = "x86_64")]
const TICK_MS: i64 = 5;
#[cfg(target_arch = "aarch64")]
const TICK_MS: i64 = 1;
const LATE_MS: i64 = 2 * TICK_MS + 20;
/// How soon a process blocked on a socket must come back once it is released. This is a
/// liveness bound, not a performance target.
const WAKE_MS: i64 = 100;
/// How long a case waits for data, a connection or an error it expects to arrive.
const ARRIVE_MS: u64 = 3000;
/// Time a case keeps to report and clean up after its last bounded wait.
const CLEANUP_MS: u64 = 1500;

/// Where struct stat keeps st_mode on each architecture; st_ino is at 8 on both.
#[cfg(target_arch = "x86_64")]
const ST_MODE: usize = 24;
#[cfg(target_arch = "aarch64")]
const ST_MODE: usize = 16;

// ---------------------------------------------------------------------------
// System calls made directly.

/// A system call made as a C library makes it: on x86-64 the SYSCALL instruction, and on
/// ARM64 svc.
fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    #[cfg(target_arch = "x86_64")]
    {
        let ret: i64;
        // SAFETY: every caller keeps the buffers its arguments point to alive through the
        // call; SYSCALL writes only rax, rcx and r11.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n as i64 => ret,
                in("rdi") a(0), in("rsi") a(1), in("rdx") a(2), in("r10") a(3), in("r8") a(4), in("r9") a(5),
                lateout("rcx") _, lateout("r11") _,
                options(nostack),
            );
        }
        ret
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: every caller keeps the buffers its arguments point to alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

// ---------------------------------------------------------------------------
// The C library's functions.

/// errno as the C library left it.
#[cfg(libc_has = "__errno_location")]
fn errno() -> i64 {
    extern "C" {
        fn __errno_location() -> *mut i32;
    }
    // SAFETY: the library returns the calling thread's errno, which lives as long as it.
    i64::from(unsafe { *__errno_location() })
}
#[cfg(not(libc_has = "__errno_location"))]
fn errno() -> i64 { 4095 }

/// Declare C library functions the cases need: each is a Rust function of the same name
/// that calls the library's function when libc.a defines it, returning its result, or
/// `-errno` when it returns -1. When the library does not define it, the case fails with
/// "the C library has no <name>".
macro_rules! c_fn {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type.
            let r = unsafe { f($($arg),*) } as i64;
            Ok(if r == -1 { -errno() } else { r })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            let _ = ($($arg,)*);
            Err(CaseError::Fail(concat!("the C library has no ", $sym).into()))
        }
    )* };
}

/// Declare C library functions whose result is not -1 and errno (getaddrinfo's EAI_*
/// codes, pointers): each returns the library function's own result, and fails the case
/// with "the C library has no <name>" when libc.a does not define it.
macro_rules! c_raw {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<$ret, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type.
            Ok(unsafe { f($($arg),*) })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<$ret, CaseError> {
            let _ = ($($arg,)*);
            Err(CaseError::Fail(concat!("the C library has no ", $sym).into()))
        }
    )* };
}

/// Declare C library functions that Linux implements as system calls: each calls the
/// library's function when libc.a defines it, and otherwise makes the system call by its
/// Linux number with the arguments given, as a C library makes it. Either way the result
/// is the value, or `-errno`.
macro_rules! c_or_sys {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $nr:expr, [$($sa:expr),*]; )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type.
            let r = unsafe { f($($arg),*) } as i64;
            Ok(if r == -1 { -errno() } else { r })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            Ok(sc($nr, &[$($sa as u64),*]))
        }
    )* };
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd { fd: i32, events: i16, revents: i16 }

/// struct timeval.
type Tv = [i64; 2];

/// struct iovec.
#[repr(C)]
struct IoVec { base: *mut u8, len: usize }

/// The Linux ABI's struct msghdr.
#[repr(C)]
struct MsgHdr { name: *mut u8, namelen: u32, iov: *mut IoVec, iovlen: usize, control: *mut u8, controllen: usize, flags: i32 }

/// A control message buffer: struct cmsghdr (a size_t length, the level and the type)
/// and room for its data.
#[repr(C, align(8))]
struct Cmsg { len: usize, level: i32, kind: i32, data: [u8; 48] }
/// CMSG_LEN and CMSG_SPACE of one descriptor.
const CMSG_LEN_FD: usize = 16 + 4;
const CMSG_SPACE_FD: usize = 16 + 8;

/// struct epoll_event: packed on x86-64, naturally aligned on ARM64.
#[cfg(target_arch = "x86_64")]
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct EpollEvent { events: u32, data: u64 }
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EpollEvent { events: u32, data: u64 }

/// struct addrinfo.
#[repr(C)]
struct AddrInfo {
    flags: i32,
    family: i32,
    socktype: i32,
    protocol: i32,
    addrlen: u32,
    addr: *mut u8,
    canonname: *mut u8,
    next: *mut AddrInfo,
}

c_fn! {
    "close" fn close(fd: i32) -> i32;
    "read" fn read(fd: i32, buf: *mut u8, len: usize) -> isize;
    "write" fn write(fd: i32, buf: *const u8, len: usize) -> isize;
    "open" fn open(path: *const u8, flags: i32, mode: u32) -> i32;
    "unlink" fn unlink(path: *const u8) -> i32;
    "stat" fn stat(path: *const u8, buf: *mut u8) -> i32;
    "fstat" fn fstat(fd: i32, buf: *mut u8) -> i32;
    "pipe" fn pipe(fds: *mut i32) -> i32;
    "poll" fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    "select" fn select(nfds: i32, r: *mut u64, w: *mut u64, e: *mut u64, timeout: *mut Tv) -> i32;
}

c_or_sys! {
    "socket" fn socket(domain: i32, kind: i32, protocol: i32) -> i32 = nr::SOCKET, [domain, kind, protocol];
    "socketpair" fn socketpair(domain: i32, kind: i32, protocol: i32, sv: *mut i32) -> i32 = nr::SOCKETPAIR, [domain, kind, protocol, sv];
    "bind" fn bind(fd: i32, addr: *const u8, len: u32) -> i32 = nr::BIND, [fd, addr, len];
    "listen" fn listen(fd: i32, backlog: i32) -> i32 = nr::LISTEN, [fd, backlog];
    "accept" fn accept(fd: i32, addr: *mut u8, len: *mut u32) -> i32 = nr::ACCEPT, [fd, addr, len];
    "accept4" fn accept4(fd: i32, addr: *mut u8, len: *mut u32, flags: i32) -> i32 = nr::ACCEPT4, [fd, addr, len, flags];
    "connect" fn connect(fd: i32, addr: *const u8, len: u32) -> i32 = nr::CONNECT, [fd, addr, len];
    "send" fn send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize = nr::SENDTO, [fd, buf, len, flags, 0, 0];
    "recv" fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize = nr::RECVFROM, [fd, buf, len, flags, 0, 0];
    "sendto" fn sendto(fd: i32, buf: *const u8, len: usize, flags: i32, addr: *const u8, alen: u32) -> isize = nr::SENDTO, [fd, buf, len, flags, addr, alen];
    "recvfrom" fn recvfrom(fd: i32, buf: *mut u8, len: usize, flags: i32, addr: *mut u8, alen: *mut u32) -> isize = nr::RECVFROM, [fd, buf, len, flags, addr, alen];
    "sendmsg" fn sendmsg(fd: i32, msg: *const MsgHdr, flags: i32) -> isize = nr::SENDMSG, [fd, msg, flags];
    "recvmsg" fn recvmsg(fd: i32, msg: *mut MsgHdr, flags: i32) -> isize = nr::RECVMSG, [fd, msg, flags];
    "shutdown" fn shutdown(fd: i32, how: i32) -> i32 = nr::SHUTDOWN, [fd, how];
    "setsockopt" fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32 = nr::SETSOCKOPT, [fd, level, name, val, len];
    "getsockopt" fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, len: *mut u32) -> i32 = nr::GETSOCKOPT, [fd, level, name, val, len];
    "getsockname" fn getsockname(fd: i32, addr: *mut u8, len: *mut u32) -> i32 = nr::GETSOCKNAME, [fd, addr, len];
    "getpeername" fn getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32 = nr::GETPEERNAME, [fd, addr, len];
    "fcntl" fn fcntl(fd: i32, cmd: i32, arg: u64) -> i32 = nr::FCNTL, [fd, cmd, arg];
    "epoll_create1" fn epoll_create1(flags: i32) -> i32 = nr::EPOLL_CREATE1, [flags];
    "epoll_ctl" fn epoll_ctl(epfd: i32, op: i32, fd: i32, ev: *mut EpollEvent) -> i32 = nr::EPOLL_CTL, [epfd, op, fd, ev];
    "epoll_wait" fn epoll_wait(epfd: i32, ev: *mut EpollEvent, max: i32, timeout: i32) -> i32 = nr::EPOLL_PWAIT, [epfd, ev, max, timeout, 0, 8];
}

c_raw! {
    "getaddrinfo" fn getaddrinfo(node: *const u8, service: *const u8, hints: *const AddrInfo, res: *mut *mut AddrInfo) -> i32;
    "freeaddrinfo" fn freeaddrinfo(res: *mut AddrInfo) -> ();
    "getnameinfo" fn getnameinfo(sa: *const u8, salen: u32, host: *mut u8, hostlen: u32, serv: *mut u8, servlen: u32, flags: i32) -> i32;
    "gai_strerror" fn gai_strerror(code: i32) -> *const u8;
    "inet_pton" fn inet_pton(af: i32, src: *const u8, dst: *mut u8) -> i32;
    "inet_ntop" fn inet_ntop(af: i32, src: *const u8, dst: *mut u8, size: u32) -> *const u8;
}

// ---------------------------------------------------------------------------
// Results, time and waiting.

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 4 => "EINTR", 9 => "EBADF", 11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES",
        14 => "EFAULT", 17 => "EEXIST", 22 => "EINVAL", 24 => "EMFILE", 28 => "ENOSPC", 32 => "EPIPE", 33 => "EDOM",
        38 => "ENOSYS", 88 => "ENOTSOCK", 89 => "EDESTADDRREQ", 90 => "EMSGSIZE", 91 => "EPROTOTYPE",
        92 => "ENOPROTOOPT", 93 => "EPROTONOSUPPORT", 94 => "ESOCKTNOSUPPORT", 95 => "EOPNOTSUPP",
        97 => "EAFNOSUPPORT", 98 => "EADDRINUSE", 99 => "EADDRNOTAVAIL", 100 => "ENETDOWN", 101 => "ENETUNREACH",
        103 => "ECONNABORTED", 104 => "ECONNRESET", 105 => "ENOBUFS", 106 => "EISCONN", 107 => "ENOTCONN",
        110 => "ETIMEDOUT", 111 => "ECONNREFUSED", 113 => "EHOSTUNREACH", 114 => "EALREADY", 115 => "EINPROGRESS",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A result as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if (-4095..0).contains(&ret) { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// A call that must succeed: its value, or a failure naming the error.
fn ok(what: &str, ret: i64) -> Result<i64, CaseError> {
    if ret < 0 { err(format!("{what} failed with {}", shown(ret))) } else { Ok(ret) }
}

/// A call that must return exactly 0.
fn zero(what: &str, ret: i64) -> CaseResult {
    check(ret == 0, &format!("{what} returned {}", shown(ret)))
}

/// A call that must fail with `errno`.
fn want(what: &str, ret: i64, errno: i64) -> CaseResult {
    check(ret == -errno, &format!("{what} returned {}, expected {}", shown(ret), errname(errno)))
}

fn ts(ns: i64) -> [i64; 2] { [ns.div_euclid(NS), ns.rem_euclid(NS)] }
fn mono() -> i64 {
    let mut t = [0i64; 2];
    if sc(nr::CLOCK_GETTIME, &[CLOCK_MONOTONIC as u64, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * NS + t[1]
}
fn now_ms() -> u64 { (mono() / MS) as u64 }

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

fn nap() {
    let t = ts(MS);
    let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
}

/// Sleep `ms`, resuming after any interruption.
fn sleep_ms(ms: u64) {
    let end = mono() + ms as i64 * MS;
    while mono() < end { nap(); }
}

/// Wait up to `ms` for `cond`, napping between looks.
fn until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    loop {
        if cond() { return true; }
        if now_ms().saturating_sub(start) >= ms { return false; }
        nap();
    }
}

/// How long a timed wait of `want_ms` took, `took_ns`: reported, and checked never early
/// and late by at most LATE_MS.
fn on_time(what: &str, took_ns: i64, want_ms: i64) -> CaseResult {
    let late_us = (took_ns - want_ms * MS) / 1000;
    value("late", late_us, "us", Some((0, LATE_MS * 1000)));
    check(late_us >= 0, &format!("{what} returned {} us before its {want_ms} ms timeout", -late_us))?;
    check(late_us <= LATE_MS * 1000, &format!("{what} returned {late_us} us after its {want_ms} ms timeout, more than {LATE_MS} ms"))
}

/// How soon a blocked call came back once it was released, `woke_ns` against
/// `released_ns`: reported, and checked not before the release and within WAKE_MS.
fn woken(what: &str, woke_ns: i64, released_ns: i64) -> CaseResult {
    let us = (woke_ns - released_ns) / 1000;
    value("wake", us, "us", Some((0, WAKE_MS * 1000)));
    check(woke_ns >= released_ns, &format!("{what} returned {} us before it was released", -us))?;
    check(us <= WAKE_MS * 1000, &format!("{what} returned {us} us after it was released, more than {WAKE_MS} ms"))
}

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }

/// End the process at once, running no destructors, so a child never unlinks a path or
/// closes a socket the case still uses.
fn exit_group(status: i32) -> ! {
    sc(nr::EXIT_GROUP, &[status as u64]);
    loop { core::hint::spin_loop(); }
}

/// A NUL-terminated copy of `s`.
fn c(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// The NUL-terminated string at `p`, at most 1024 bytes of it.
fn c_text(p: *const u8) -> String {
    if p.is_null() { return String::new(); }
    let mut out = Vec::new();
    // SAFETY: p points to a NUL-terminated string the C library or the case owns; the
    // read stops at its NUL or after 1024 bytes.
    unsafe {
        while out.len() < 1024 && *p.add(out.len()) != 0 { out.push(*p.add(out.len())); }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The text a fixed buffer holds up to its first NUL.
fn buf_text(buf: &[u8]) -> String {
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The first port of this case's sixteen, fixed by the first call so processes the case
/// forks afterwards use the same ports.
static PORT_BASE: AtomicU32 = AtomicU32::new(0);

/// A TCP or UDP port for this case's process: 20000-29999, below Linux's ephemeral
/// range, sixteen to a process.
fn port(n: u16) -> u16 {
    let mut base = PORT_BASE.load(SeqCst);
    if base == 0 {
        base = 20000 + (pid() as u32 * 16) % 10000;
        PORT_BASE.store(base, SeqCst);
    }
    (base + n as u32) as u16
}

/// Bytes of a stream whose position each byte shows, so a reordered, lost or repeated
/// stretch is seen.
fn pattern(len: usize) -> Vec<u8> { (0..len).map(|i| (i ^ (i >> 8) ^ (i >> 16)).wrapping_mul(31) as u8).collect() }

/// Where two byte strings first differ, as text.
fn first_difference(got: &[u8], want: &[u8]) -> String {
    match got.iter().zip(want).position(|(a, b)| a != b) {
        Some(i) => format!("byte {i} is {:#04x}, expected {:#04x}", got[i], want[i]),
        None => format!("{} bytes arrived, expected {}", got.len(), want.len()),
    }
}

/// A `struct stat`, large and aligned enough for the Linux ABI's.
type StatBuf = [u64; 32];
fn st_mode(st: &StatBuf) -> u32 { (st[ST_MODE / 8] >> (8 * (ST_MODE % 8))) as u32 }
fn st_ino(st: &StatBuf) -> u64 { st[1] }

fn fstat_of(fd: i32) -> Result<StatBuf, CaseError> {
    let mut st: StatBuf = [0; 32];
    ok("fstat", fstat(fd, st.as_mut_ptr() as *mut u8)?)?;
    Ok(st)
}

// ---------------------------------------------------------------------------
// Descriptors and addresses.

/// A descriptor the case owns, closed when it is dropped.
struct Fd(i32);

impl Drop for Fd {
    fn drop(&mut self) {
        if self.0 >= 0 { let _ = close(self.0); }
    }
}

/// A descriptor a call returned, owned so it is closed: None when the call failed.
fn adopt(ret: i64) -> Option<Fd> { (ret >= 0).then(|| Fd(ret as i32)) }

/// struct sockaddr_in, its port and address in network order.
#[repr(C)]
#[derive(Clone, Copy)]
struct SockIn { family: u16, port: u16, addr: u32, zero: [u8; 8] }

fn sin(addr: u32, port: u16) -> SockIn { SockIn { family: AF_INET as u16, port: port.to_be(), addr: addr.to_be(), zero: [0; 8] } }
const SIN_LEN: u32 = 16;

/// struct sockaddr_un.
#[repr(C)]
#[derive(Clone, Copy)]
struct SockUn { family: u16, path: [u8; 108] }
const SUN_LEN: u32 = 110;

fn sa<T>(x: &T) -> *const u8 { x as *const T as *const u8 }

fn ip(a: u32) -> String { format!("{}.{}.{}.{}", a >> 24, (a >> 16) & 0xff, (a >> 8) & 0xff, a & 0xff) }

/// Storage for an address a call returns, and how long the call said it is.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct Store { b: [u8; 128], len: u32 }

impl Store {
    fn new() -> Store { Store { b: [0; 128], len: 128 } }
    fn ptr(&mut self) -> *mut u8 { self.b.as_mut_ptr() }
    fn family(&self) -> i32 { i32::from(u16::from_ne_bytes([self.b[0], self.b[1]])) }
    fn port(&self) -> u16 { u16::from_be_bytes([self.b[2], self.b[3]]) }
    fn addr(&self) -> u32 { u32::from_be_bytes([self.b[4], self.b[5], self.b[6], self.b[7]]) }
    /// The sun_path bytes the length covers.
    fn path(&self) -> Vec<u8> { self.b[2..(self.len as usize).clamp(2, 110)].to_vec() }
    fn text(&self) -> String {
        match self.family() {
            AF_INET => format!("{}:{} (length {})", ip(self.addr()), self.port(), self.len),
            AF_UNIX => format!("AF_UNIX {:?} (length {})", String::from_utf8_lossy(&self.path()), self.len),
            f => format!("family {f} (length {})", self.len),
        }
    }
}

/// getsockname, or getpeername when `peer`: the address, or a failure naming the error.
fn name_of(fd: &Fd, peer: bool) -> Result<Store, CaseError> {
    let mut st = Store::new();
    let what = if peer { "getpeername" } else { "getsockname" };
    let r = if peer { getpeername(fd.0, st.ptr(), &mut st.len)? } else { getsockname(fd.0, st.ptr(), &mut st.len)? };
    ok(what, r)?;
    Ok(st)
}

/// Check an IPv4 address a call returned: AF_INET, length 16, `addr`, and `port` when it
/// is given or else any port but 0.
fn is_in(what: &str, st: &Store, addr: u32, port: Option<u16>) -> CaseResult {
    let port_ok = match port { Some(p) => st.port() == p, None => st.port() != 0 };
    let expected = match port { Some(p) => format!("{}:{p}", ip(addr)), None => format!("{}:<nonzero>", ip(addr)) };
    check(st.family() == AF_INET && st.len == SIN_LEN && st.addr() == addr && port_ok,
        &format!("{what} reported {}, expected AF_INET {expected} (length 16)", st.text()))
}

fn sock(domain: i32, kind: i32, what: &str) -> Result<Fd, CaseError> {
    Ok(Fd(ok(&format!("socket({what})"), socket(domain, kind, 0)?)? as i32))
}
fn udp() -> Result<Fd, CaseError> { sock(AF_INET, SOCK_DGRAM, "AF_INET, SOCK_DGRAM") }
fn tcp() -> Result<Fd, CaseError> { sock(AF_INET, SOCK_STREAM, "AF_INET, SOCK_STREAM") }

fn bind_in(fd: &Fd, addr: u32, port: u16) -> Result<i64, CaseError> { let a = sin(addr, port); bind(fd.0, sa(&a), SIN_LEN) }
fn connect_in(fd: &Fd, addr: u32, port: u16) -> Result<i64, CaseError> { let a = sin(addr, port); connect(fd.0, sa(&a), SIN_LEN) }
fn sendto_in(fd: &Fd, bytes: &[u8], addr: u32, port: u16) -> Result<i64, CaseError> {
    let a = sin(addr, port);
    sendto(fd.0, bytes.as_ptr(), bytes.len(), MSG_NOSIGNAL, sa(&a), SIN_LEN)
}

/// A UDP socket bound to 127.0.0.1:`port`.
fn udp_at(port: u16) -> Result<Fd, CaseError> {
    let s = udp()?;
    zero(&format!("bind to 127.0.0.1:{port}"), bind_in(&s, LOOPBACK, port)?)?;
    Ok(s)
}

/// Send one datagram to 127.0.0.1:`port`, which must take all of it.
fn send_dgram(fd: &Fd, bytes: &[u8], port: u16) -> CaseResult {
    let n = sendto_in(fd, bytes, LOOPBACK, port)?;
    check(n == bytes.len() as i64, &format!("sendto of {} bytes to 127.0.0.1:{port} returned {}", bytes.len(), shown(n)))
}

/// A TCP socket of `kind` (SOCK_STREAM with any flags) bound to 127.0.0.1:`port` and
/// listening with `backlog`.
fn listener_of(kind: i32, port: u16, backlog: i32) -> Result<Fd, CaseError> {
    let s = sock(AF_INET, kind, "AF_INET, SOCK_STREAM")?;
    zero(&format!("bind to 127.0.0.1:{port}"), bind_in(&s, LOOPBACK, port)?)?;
    zero(&format!("listen({backlog})"), listen(s.0, backlog)?)?;
    Ok(s)
}
fn listener(port: u16) -> Result<Fd, CaseError> { listener_of(SOCK_STREAM, port, 8) }

/// A TCP socket connected to 127.0.0.1:`port`.
fn connected(port: u16) -> Result<Fd, CaseError> {
    let s = tcp()?;
    zero(&format!("connect to 127.0.0.1:{port}"), connect_in(&s, LOOPBACK, port)?)?;
    Ok(s)
}

fn accepted(l: &Fd) -> Result<Fd, CaseError> { Ok(Fd(ok("accept", accept(l.0, null_mut(), null_mut())?)? as i32)) }

/// A TCP connection over loopback: the listener, the connecting socket and the socket
/// accept returned for it.
struct Tcp { l: Fd, c: Fd, s: Fd, port: u16 }

fn tcp_pair() -> Result<Tcp, CaseError> {
    let port = port(0);
    let l = listener(port)?;
    let c = connected(port)?;
    let s = accepted(&l)?;
    Ok(Tcp { l, c, s, port })
}

/// A connected Unix-domain socket pair of `kind`.
fn pair(kind: i32) -> Result<(Fd, Fd), CaseError> {
    let mut sv = [-1i32; 2];
    ok("socketpair(AF_UNIX)", socketpair(AF_UNIX, kind, 0, sv.as_mut_ptr())?)?;
    Ok((Fd(sv[0]), Fd(sv[1])))
}

fn get_int(fd: &Fd, level: i32, name: i32, what: &str) -> Result<i32, CaseError> {
    let mut v = 0i32;
    let mut len = 4u32;
    ok(&format!("getsockopt({what})"), getsockopt(fd.0, level, name, &mut v as *mut i32 as *mut u8, &mut len)?)?;
    Ok(v)
}
fn set_int(fd: &Fd, level: i32, name: i32, v: i32) -> Result<i64, CaseError> {
    setsockopt(fd.0, level, name, &v as *const i32 as *const u8, 4)
}
fn set_on(fd: &Fd, level: i32, name: i32, what: &str) -> CaseResult {
    zero(&format!("setsockopt({what}, 1)"), set_int(fd, level, name, 1)?)
}
fn set_tv(fd: &Fd, name: i32, tv: Tv) -> Result<i64, CaseError> {
    setsockopt(fd.0, SOL_SOCKET, name, tv.as_ptr() as *const u8, 16)
}

/// The descriptor flags (F_GETFD) or status flags (F_GETFL) of `fd`.
fn fd_flags(fd: i32, cmd: i32) -> Result<i64, CaseError> {
    ok(if cmd == F_GETFD { "fcntl(F_GETFD)" } else { "fcntl(F_GETFL)" }, fcntl(fd, cmd, 0)?)
}

/// Send all of `bytes` on a connected socket, without SIGPIPE.
fn send_all(fd: &Fd, bytes: &[u8], what: &str) -> CaseResult {
    let mut at = 0;
    while at < bytes.len() {
        let n = send(fd.0, bytes[at..].as_ptr(), bytes.len() - at, MSG_NOSIGNAL)?;
        if n <= 0 { return err(format!("{what}: send failed with {} after {at} of {} bytes", shown(n), bytes.len())); }
        at += n as usize;
    }
    Ok(())
}

/// recv with MSG_DONTWAIT and `flags` until it returns anything but EAGAIN, for up to
/// `ms`: its result, or None if nothing came.
fn recv_wait(fd: &Fd, buf: &mut [u8], flags: i32, ms: u64) -> Result<Option<i64>, CaseError> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    loop {
        let n = recv(fd.0, buf.as_mut_ptr(), buf.len(), flags | MSG_DONTWAIT)?;
        if n != -EAGAIN { return Ok(Some(n)); }
        if now_ms().saturating_sub(start) >= ms { return Ok(None); }
        nap();
    }
}

/// The next datagram or bytes that arrive on `fd`, received into a buffer of `cap` bytes
/// with `flags`: the count recv returned and the bytes it stored.
fn recv_some(fd: &Fd, cap: usize, flags: i32, what: &str) -> Result<(i64, Vec<u8>), CaseError> {
    let mut buf = vec![0u8; cap];
    match recv_wait(fd, &mut buf, flags, ARRIVE_MS)? {
        None => err(format!("{what}: nothing arrived within {ARRIVE_MS} ms")),
        Some(n) if n < 0 => err(format!("{what}: recv failed with {}", shown(n))),
        Some(n) => { buf.truncate((n as usize).min(cap)); Ok((n, buf)) }
    }
}

/// Receive exactly `want` bytes from a stream, within `ms`.
fn recv_exact(fd: &Fd, want: usize, ms: u64, what: &str) -> Result<Vec<u8>, CaseError> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut out = Vec::with_capacity(want);
    let mut buf = vec![0u8; 16384];
    while out.len() < want {
        let room = (want - out.len()).min(buf.len());
        let n = recv(fd.0, buf.as_mut_ptr(), room, MSG_DONTWAIT)?;
        if n == 0 { return err(format!("{what}: end of file after {} of {want} bytes", out.len())); }
        if n > 0 { out.extend_from_slice(&buf[..n as usize]); continue; }
        if n != -EAGAIN { return err(format!("{what}: recv failed with {} after {} of {want} bytes", shown(n), out.len())); }
        if now_ms().saturating_sub(start) >= ms { return err(format!("{what}: only {} of {want} bytes arrived within {ms} ms", out.len())); }
        nap();
    }
    Ok(out)
}

/// The stream must reach end of file: recv returns 0 within ARRIVE_MS.
fn expect_eof(fd: &Fd, what: &str) -> CaseResult {
    let mut buf = [0u8; 64];
    match recv_wait(fd, &mut buf, 0, ARRIVE_MS)? {
        Some(0) => Ok(()),
        None => fail(format!("{what}: no end of file within {ARRIVE_MS} ms")),
        Some(n) => fail(format!("{what}: recv returned {} instead of end of file", shown(n))),
    }
}

/// recvfrom with MSG_DONTWAIT until a datagram comes, for up to ARRIVE_MS: the count, the
/// bytes and the sender's address.
fn recvfrom_wait(fd: &Fd, cap: usize, what: &str) -> Result<(i64, Vec<u8>, Store), CaseError> {
    let ms = bounded(ARRIVE_MS, CLEANUP_MS);
    let start = now_ms();
    let mut buf = vec![0u8; cap];
    loop {
        let mut from = Store::new();
        let n = recvfrom(fd.0, buf.as_mut_ptr(), cap, MSG_DONTWAIT, from.ptr(), &mut from.len)?;
        if n >= 0 { buf.truncate((n as usize).min(cap)); return Ok((n, buf, from)); }
        if n != -EAGAIN { return err(format!("{what}: recvfrom failed with {}", shown(n))); }
        if now_ms().saturating_sub(start) >= ms { return err(format!("{what}: nothing arrived within {ms} ms")); }
        nap();
    }
}

fn poll_one(fd: &Fd, events: i16, timeout: i32) -> Result<(i64, i16), CaseError> {
    let mut pfd = PollFd { fd: fd.0, events, revents: 0 };
    let n = poll(&mut pfd, 1, timeout)?;
    Ok((n, pfd.revents))
}

/// Poll `fd` for `events` until any of `want` is reported, for up to ARRIVE_MS: the last
/// revents seen.
fn poll_until(fd: &Fd, events: i16, want: i16) -> Result<i16, CaseError> {
    let ms = bounded(ARRIVE_MS, CLEANUP_MS);
    let start = now_ms();
    loop {
        let (n, rev) = poll_one(fd, events, 10)?;
        if n < 0 { return err(format!("poll failed with {}", shown(n))); }
        if rev & want != 0 || now_ms().saturating_sub(start) >= ms { return Ok(rev); }
    }
}

/// A select fd_set as the Linux ABI lays it out, with `fd` in it.
fn fd_set(fd: i32) -> [u64; 16] {
    let mut set = [0u64; 16];
    set[fd as usize / 64] |= 1 << (fd % 64);
    set
}
fn in_set(set: &[u64; 16], fd: i32) -> bool { set[fd as usize / 64] & (1 << (fd % 64)) != 0 }

/// select on one descriptor for reading (`write` false) or writing, with a timeout in
/// milliseconds: the count and whether the descriptor is in the returned set.
fn select_one(fd: &Fd, write: bool, ms: i64) -> Result<(i64, bool), CaseError> {
    let mut set = fd_set(fd.0);
    let mut tv: Tv = [ms / 1000, (ms % 1000) * 1000];
    let (r, w) = if write { (null_mut(), set.as_mut_ptr()) } else { (set.as_mut_ptr(), null_mut()) };
    let n = select(fd.0 + 1, r, w, null_mut(), &mut tv)?;
    Ok((n, n > 0 && in_set(&set, fd.0)))
}

fn hex16(v: i16) -> String { format!("{:#06x}", v as u16) }

/// A Unix-domain address: a path under /tmp, unlinked when the case ends, or an abstract
/// name.
struct UnAddr { sa: SockUn, len: u32, file: Option<Vec<u8>>, owner: i32 }

impl UnAddr {
    fn ptr(&self) -> *const u8 { sa(&self.sa) }
    /// The sun_path bytes the address's length covers.
    fn path(&self) -> Vec<u8> { self.sa.path[..self.len as usize - 2].to_vec() }
    fn file(&self) -> *const u8 { self.file.as_ref().map_or(null(), |f| f.as_ptr()) }
}

impl Drop for UnAddr {
    fn drop(&mut self) {
        if pid() != self.owner { return; }
        if let Some(f) = &self.file { let _ = unlink(f.as_ptr()); }
    }
}

/// A path under /tmp for this case's process. Its length counts the terminating NUL, as
/// Linux reports a bound path's.
fn un_path(tag: &str) -> UnAddr {
    let text = format!("/tmp/net-{}-{tag}", pid());
    let mut addr = SockUn { family: AF_UNIX as u16, path: [0; 108] };
    addr.path[..text.len()].copy_from_slice(text.as_bytes());
    UnAddr { sa: addr, len: 2 + text.len() as u32 + 1, file: Some(c(&text)), owner: pid() }
}

/// An abstract name for this case's process: a NUL and then the name, its length exactly
/// that many bytes.
fn un_abstract(tag: &str) -> UnAddr {
    let text = format!("breenix-net-{}-{tag}", pid());
    let mut addr = SockUn { family: AF_UNIX as u16, path: [0; 108] };
    addr.path[1..1 + text.len()].copy_from_slice(text.as_bytes());
    UnAddr { sa: addr, len: 2 + 1 + text.len() as u32, file: None, owner: pid() }
}

fn usock(kind: i32) -> Result<Fd, CaseError> { sock(AF_UNIX, kind, "AF_UNIX") }

fn un_bind(fd: &Fd, a: &UnAddr) -> Result<i64, CaseError> { bind(fd.0, a.ptr(), a.len) }
fn un_connect(fd: &Fd, a: &UnAddr) -> Result<i64, CaseError> { connect(fd.0, a.ptr(), a.len) }

/// A Unix stream socket bound to `a` and listening.
fn un_listener(a: &UnAddr) -> Result<Fd, CaseError> {
    let s = usock(SOCK_STREAM)?;
    zero("bind", un_bind(&s, a)?)?;
    zero("listen(8)", listen(s.0, 8)?)?;
    Ok(s)
}

/// A Unix stream socket connected to `a`.
fn un_connected(a: &UnAddr) -> Result<Fd, CaseError> {
    let s = usock(SOCK_STREAM)?;
    zero("connect", un_connect(&s, a)?)?;
    Ok(s)
}

// ---------------------------------------------------------------------------
// Shared pages and the processes a case starts.

/// A page the case shares with the processes it starts: words for results and times,
/// and a message.
#[repr(C, align(64))]
struct Page {
    w: [AtomicI64; 8],
    len: AtomicU32,
    text: [AtomicU8; 200],
}

impl Page {
    fn get(&self, i: usize) -> i64 { self.w[i].load(SeqCst) }
    fn set(&self, i: usize, v: i64) { self.w[i].store(v, SeqCst) }
    fn say(&self, msg: &str) {
        let n = msg.len().min(self.text.len());
        for (cell, &b) in self.text.iter().zip(&msg.as_bytes()[..n]) { cell.store(b, Relaxed); }
        self.len.store(n as u32, SeqCst);
    }
    fn said(&self) -> String {
        let n = (self.len.load(SeqCst) as usize).min(self.text.len());
        let bytes: Vec<u8> = self.text[..n].iter().map(|cell| cell.load(Relaxed)).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A fresh zeroed page shared with every process the caller forks afterwards.
fn page() -> Result<&'static Page, CaseError> {
    let ret = sc(nr::MMAP, &[0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED_ANON, u64::MAX, 0]);
    if (-4095..0).contains(&ret) { return err(format!("mmap of a shared page failed with {}", errname(-ret))); }
    // SAFETY: a fresh zeroed shared page, large enough for a Page and never unmapped.
    Ok(unsafe { &*(ret as *const Page) })
}

/// A wait status in words.
fn status_text(status: i32) -> String {
    let sig = status & 0x7f;
    if sig == 0 { format!("exited with status {}", (status >> 8) & 0xff) } else { format!("was killed by signal {sig}") }
}

/// A process the case started. Dropping it kills and reaps it.
struct Child { pid: i32, note: &'static Page, status: Option<i32>, owner: i32 }

/// Start `body` in a process of its own. It ends with status 0 when `body` passes, and
/// with status 1 after putting its message on its page when it fails.
fn spawn(body: impl FnOnce() -> CaseResult) -> Result<Child, CaseError> {
    let note = page()?;
    match process::fork() {
        Ok(ForkResult::Child) => {
            let code = match body() {
                Ok(()) => 0,
                Err(CaseError::Fail(m)) | Err(CaseError::Skip(m)) => { note.say(&m); 1 }
            };
            exit_group(code)
        }
        Ok(ForkResult::Parent(p)) => Ok(Child { pid: p.raw() as i32, note, status: None, owner: pid() }),
        Err(e) => err(format!("fork failed: {e}")),
    }
}

impl Child {
    /// Reap the process if it has ended.
    fn reap(&mut self) -> Option<i32> {
        if self.status.is_none() {
            let mut status = 0i32;
            let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, WNOHANG, 0]);
            if r == self.pid as i64 { self.status = Some(status); }
        }
        self.status
    }

    /// The process's wait status once it ends, within `ms`.
    fn wait(&mut self, ms: u64, who: &str) -> Result<i32, CaseError> {
        let ms = bounded(ms, CLEANUP_MS);
        if until(ms, || self.reap().is_some()) { return Ok(self.status.unwrap_or(0)); }
        err(format!("{who} did not finish within {ms} ms"))
    }

    /// What an ended process's status says: a pass for status 0, otherwise its message.
    fn outcome(&self, status: i32, who: &str) -> CaseResult {
        if status == 0 { return Ok(()); }
        let said = self.note.said();
        if status == 1 << 8 && !said.is_empty() { return fail(format!("{who}: {said}")); }
        fail(format!("{who} {}", status_text(status)))
    }

    /// Wait up to `ms` for the process to end, passing if it passed.
    fn finish(&mut self, ms: u64, who: &str) -> CaseResult {
        let status = self.wait(ms, who)?;
        self.outcome(status, who)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.status.is_some() || pid() != self.owner { return; }
        let _ = sc(nr::KILL, &[self.pid as u64, SIGKILL as u64]);
        let start = now_ms();
        while self.reap().is_none() && now_ms().saturating_sub(start) < 1000 { nap(); }
    }
}

/// Make a call that may block in a process of its own and wait up to `ms` for it: its
/// result and how long it took in nanoseconds. A call still blocked after `ms` fails the
/// case saying so; the process is killed.
fn timed_call(ms: u64, what: &str, call: impl FnOnce() -> Result<i64, CaseError>) -> Result<(i64, i64), CaseError> {
    let out = page()?;
    let mut child = spawn(|| {
        let start = mono();
        let r = call()?;
        out.set(1, mono() - start);
        out.set(0, r);
        out.set(2, 1);
        Ok(())
    })?;
    let ms = bounded(ms, CLEANUP_MS);
    if !until(ms, || child.reap().is_some()) { return err(format!("{what} was still blocked after {ms} ms")); }
    let status = child.status.unwrap_or(0);
    child.outcome(status, what)?;
    if out.get(2) != 1 { return err(format!("{what}: the process making it did not report")); }
    Ok((out.get(0), out.get(1)))
}

/// Set SIGPIPE's action to the default, which ends the process.
fn default_sigpipe() -> CaseResult {
    let action = Sigaction { handler: signal::SIG_DFL, ..Default::default() };
    signal::sigaction(SIGPIPE, Some(&action), None).map_err(|e| CaseError::Fail(format!("sigaction(SIGPIPE) failed: {e}")))
}

// ---------------------------------------------------------------------------
// UDP.

fn udp_socket() -> CaseResult {
    let s = udp()?;
    let st = fstat_of(s.0)?;
    check(st_mode(&st) & S_IFMT == S_IFSOCK, &format!("fstat of the socket reports mode {:o}, not S_IFSOCK", st_mode(&st)))
}

fn udp_socket_protocol() -> CaseResult {
    let r = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)?;
    let _s = adopt(r);
    ok("socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)", r).map(|_| ())
}

fn udp_socket_protocol_mismatch() -> CaseResult {
    let r = socket(AF_INET, SOCK_DGRAM, IPPROTO_TCP)?;
    let _s = adopt(r);
    want("socket(AF_INET, SOCK_DGRAM, IPPROTO_TCP)", r, EPROTONOSUPPORT)
}

fn udp_socket_family() -> CaseResult {
    let r = socket(12345, SOCK_DGRAM, 0)?;
    let _s = adopt(r);
    want("socket(12345, SOCK_DGRAM, 0)", r, EAFNOSUPPORT)
}

fn udp_socket_type() -> CaseResult {
    let r = socket(AF_INET, 99, 0)?;
    let _s = adopt(r);
    want("socket(AF_INET, 99, 0)", r, EINVAL)
}

fn udp_bind() -> CaseResult {
    let s = udp()?;
    zero("bind to 127.0.0.1", bind_in(&s, LOOPBACK, port(0))?)
}

fn udp_bind_eaddrinuse() -> CaseResult {
    let _a = udp_at(port(0))?;
    let b = udp()?;
    want("a second bind to the same address and port", bind_in(&b, LOOPBACK, port(0))?, EADDRINUSE)
}

fn udp_bind_twice() -> CaseResult {
    let s = udp_at(port(0))?;
    want("bind of a socket already bound", bind_in(&s, LOOPBACK, port(1))?, EINVAL)
}

fn udp_bind_eaddrnotavail() -> CaseResult {
    let s = udp()?;
    want("bind to 192.0.2.1, an address no interface has", bind_in(&s, TEST_NET, port(0))?, EADDRNOTAVAIL)
}

fn udp_bind_short() -> CaseResult {
    let s = udp()?;
    let a = sin(LOOPBACK, port(0));
    want("bind with an address length of 8", bind(s.0, sa(&a), 8)?, EINVAL)
}

fn udp_bind_family() -> CaseResult {
    let s = udp()?;
    let mut a = sin(LOOPBACK, port(0));
    a.family = AF_UNIX as u16;
    want("bind of an AF_INET socket to an address with sin_family AF_UNIX", bind(s.0, sa(&a), SIN_LEN)?, EAFNOSUPPORT)
}

fn udp_bind_ephemeral() -> CaseResult {
    let s = udp()?;
    zero("bind to 127.0.0.1:0", bind_in(&s, LOOPBACK, 0)?)?;
    is_in("getsockname after binding port 0", &name_of(&s, false)?, LOOPBACK, None)
}

fn udp_sendto_recvfrom() -> CaseResult {
    let a = udp_at(port(0))?;
    let b = udp_at(port(1))?;
    send_dgram(&a, b"hello", port(1))?;
    let (n, got, from) = recvfrom_wait(&b, 64, "the receiver")?;
    check(n == 5 && got == b"hello", &format!("recvfrom returned {} bytes {:?}, expected \"hello\"", shown(n), String::from_utf8_lossy(&got)))?;
    is_in("recvfrom's source address", &from, LOOPBACK, Some(port(0)))
}

fn udp_sendto_autobind() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, b"auto", port(1))?;
    let (_, _, from) = recvfrom_wait(&b, 64, "the receiver")?;
    is_in("recvfrom's source address for an unbound sender", &from, LOOPBACK, None)
}

fn udp_boundaries() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    let sizes = [1usize, 100, 1000];
    for &len in &sizes { send_dgram(&a, &pattern(len), port(1))?; }
    for &len in &sizes {
        let (n, got) = recv_some(&b, 2048, 0, "the receiver")?;
        check(n == len as i64 && got == pattern(len), &format!("a {len}-byte datagram arrived as {} bytes", shown(n)))?;
    }
    Ok(())
}

fn udp_truncation() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, &pattern(100), port(1))?;
    send_dgram(&a, b"next!", port(1))?;
    let (n, got) = recv_some(&b, 10, 0, "the receiver")?;
    check(n == 10 && got == pattern(100)[..10], &format!("recv of a 100-byte datagram into 10 bytes returned {}", shown(n)))?;
    let (n, got) = recv_some(&b, 64, 0, "the receiver")?;
    check(n == 5 && got == b"next!", &format!("after the truncated datagram recv returned {} bytes {:?}, expected the next datagram: the rest was not discarded",
        shown(n), String::from_utf8_lossy(&got)))
}

/// MSG_TRUNC on input is Linux's: recv returns the datagram's real length even when the
/// buffer is shorter. POSIX defines MSG_TRUNC only as an output flag of recvmsg.
fn udp_msg_trunc() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, &pattern(100), port(1))?;
    let (n, got) = recv_some(&b, 10, MSG_TRUNC, "the receiver")?;
    check(n == 100, &format!("recv(MSG_TRUNC) of a 100-byte datagram into 10 bytes returned {}, expected 100", shown(n)))?;
    check(got == pattern(100)[..10], "recv(MSG_TRUNC) did not store the datagram's first 10 bytes")
}

fn udp_recvmsg_trunc() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, &pattern(100), port(1))?;
    let mut buf = [0u8; 10];
    let start = now_ms();
    loop {
        let mut iov = IoVec { base: buf.as_mut_ptr(), len: buf.len() };
        let mut msg = MsgHdr { name: null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: null_mut(), controllen: 0, flags: 0 };
        let n = recvmsg(b.0, &mut msg, MSG_DONTWAIT)?;
        if n == -EAGAIN && now_ms().saturating_sub(start) < ARRIVE_MS { nap(); continue; }
        check(n == 10, &format!("recvmsg of a 100-byte datagram into 10 bytes returned {}", shown(n)))?;
        return check(msg.flags & MSG_TRUNC != 0, &format!("recvmsg left msg_flags {:#x}, without MSG_TRUNC", msg.flags));
    }
}

fn udp_zero_length() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, b"", port(1))?;
    send_dgram(&a, b"abc", port(1))?;
    let (n, _) = recv_some(&b, 64, 0, "the receiver")?;
    check(n == 0, &format!("recv of a zero-length datagram returned {}", shown(n)))?;
    let (n, got) = recv_some(&b, 64, 0, "the receiver")?;
    check(n == 3 && got == b"abc", &format!("the datagram after the empty one arrived as {} bytes", shown(n)))
}

fn udp_peek() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, b"peek", port(1))?;
    let (n, got) = recv_some(&b, 64, MSG_PEEK, "the receiver")?;
    check(n == 4 && got == b"peek", &format!("recv(MSG_PEEK) returned {}", shown(n)))?;
    let (n, got) = recv_some(&b, 64, 0, "the receiver")?;
    check(n == 4 && got == b"peek", &format!("recv after MSG_PEEK returned {}: the peek removed the datagram", shown(n)))?;
    let mut buf = [0u8; 8];
    want("a third recv(MSG_DONTWAIT)", recv(b.0, buf.as_mut_ptr(), 8, MSG_DONTWAIT)?, EAGAIN)
}

fn udp_dontwait() -> CaseResult {
    let s = udp_at(port(0))?;
    let mut buf = [0u8; 8];
    want("recv(MSG_DONTWAIT) on an empty socket", recv(s.0, buf.as_mut_ptr(), 8, MSG_DONTWAIT)?, EAGAIN)
}

fn udp_nonblock() -> CaseResult {
    let s = sock(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, "AF_INET, SOCK_DGRAM|SOCK_NONBLOCK")?;
    zero("bind", bind_in(&s, LOOPBACK, port(0))?)?;
    let mut buf = [0u8; 8];
    want("recv on an empty SOCK_NONBLOCK socket", recv(s.0, buf.as_mut_ptr(), 8, 0)?, EAGAIN)
}

fn udp_connect_send() -> CaseResult {
    let a = udp_at(port(0))?;
    let b = udp_at(port(1))?;
    zero("connect of the UDP socket", connect_in(&a, LOOPBACK, port(1))?)?;
    let n = send(a.0, b"conn".as_ptr(), 4, MSG_NOSIGNAL)?;
    check(n == 4, &format!("send on the connected socket returned {}", shown(n)))?;
    let (n, got, from) = recvfrom_wait(&b, 64, "the peer")?;
    check(n == 4 && got == b"conn", &format!("the peer received {} bytes", shown(n)))?;
    is_in("the peer's recvfrom source", &from, LOOPBACK, Some(port(0)))
}

fn udp_connect_filter() -> CaseResult {
    let a = udp_at(port(0))?;
    let b = udp_at(port(1))?;
    let third = udp_at(port(2))?;
    zero("connect of the UDP socket", connect_in(&a, LOOPBACK, port(1))?)?;
    send_dgram(&third, b"third", port(0))?;
    send_dgram(&b, b"peer", port(0))?;
    let (n, got) = recv_some(&a, 64, 0, "the connected socket")?;
    check(got == b"peer", &format!("the connected socket received {:?} ({}), expected its peer's \"peer\"", String::from_utf8_lossy(&got), shown(n)))?;
    let mut buf = [0u8; 8];
    want("a second recv(MSG_DONTWAIT): the third socket's datagram must have been dropped",
        recv(a.0, buf.as_mut_ptr(), 8, MSG_DONTWAIT)?, EAGAIN)
}

fn udp_connect_getpeername() -> CaseResult {
    let a = udp_at(port(0))?;
    zero("connect of the UDP socket", connect_in(&a, LOOPBACK, port(1))?)?;
    is_in("getpeername of the connected socket", &name_of(&a, true)?, LOOPBACK, Some(port(1)))
}

fn udp_getpeername_enotconn() -> CaseResult {
    let a = udp_at(port(0))?;
    let mut st = Store::new();
    want("getpeername of an unconnected UDP socket", getpeername(a.0, st.ptr(), &mut st.len)?, ENOTCONN)
}

fn udp_send_edestaddrreq() -> CaseResult {
    let a = udp_at(port(0))?;
    want("send on an unconnected UDP socket", send(a.0, b"x".as_ptr(), 1, MSG_NOSIGNAL)?, EDESTADDRREQ)
}

fn udp_connect_unspec() -> CaseResult {
    let a = udp_at(port(0))?;
    zero("connect of the UDP socket", connect_in(&a, LOOPBACK, port(1))?)?;
    let mut un = sin(0, 0);
    un.family = AF_UNSPEC as u16;
    zero("connect to AF_UNSPEC", connect(a.0, sa(&un), SIN_LEN)?)?;
    let mut st = Store::new();
    want("getpeername after connecting to AF_UNSPEC", getpeername(a.0, st.ptr(), &mut st.len)?, ENOTCONN)
}

/// Linux reports the ICMP port unreachable a send to a closed port draws on a connected
/// UDP socket, as ECONNREFUSED from a later call.
fn udp_connect_refused() -> CaseResult {
    let a = udp_at(port(0))?;
    zero("connect to a port no socket has", connect_in(&a, LOOPBACK, port(9))?)?;
    let n = send(a.0, b"x".as_ptr(), 1, MSG_NOSIGNAL)?;
    check(n == 1, &format!("send returned {}", shown(n)))?;
    let mut buf = [0u8; 8];
    match recv_wait(&a, &mut buf, 0, ARRIVE_MS)? {
        None => fail(format!("no ECONNREFUSED within {ARRIVE_MS} ms of sending to a closed port")),
        Some(n) => want("recv after sending to a closed port", n, ECONNREFUSED),
    }
}

fn udp_broadcast_eacces() -> CaseResult {
    let s = udp()?;
    want("sendto 255.255.255.255 without SO_BROADCAST", sendto_in(&s, b"b", BROADCAST, port(1))?, EACCES)
}

fn udp_broadcast() -> CaseResult {
    let s = udp()?;
    set_on(&s, SOL_SOCKET, SO_BROADCAST, "SO_BROADCAST")?;
    let n = sendto_in(&s, b"bcst", BROADCAST, port(1))?;
    check(n == 4, &format!("sendto 255.255.255.255 with SO_BROADCAST returned {}", shown(n)))
}

/// The largest UDP payload over IPv4 is 65507 bytes: 65535 less the IP and UDP headers.
fn udp_emsgsize() -> CaseResult {
    let a = udp()?;
    let _b = udp_at(port(1))?;
    want("sendto of a 65508-byte datagram", sendto_in(&a, &vec![0u8; 65508], LOOPBACK, port(1))?, EMSGSIZE)
}

fn udp_large() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    let data = pattern(16384);
    send_dgram(&a, &data, port(1))?;
    let (n, got) = recv_some(&b, 65536, 0, "the receiver")?;
    check(n == 16384 && got == data, &format!("a 16384-byte datagram arrived as {} bytes ({})", shown(n), first_difference(&got, &data)))
}

fn udp_enotsock() -> CaseResult {
    let mut p = [-1i32; 2];
    ok("pipe", pipe(p.as_mut_ptr())?)?;
    let (_r, w) = (Fd(p[0]), Fd(p[1]));
    let a = sin(LOOPBACK, port(1));
    want("sendto on a pipe", sendto(w.0, b"x".as_ptr(), 1, MSG_NOSIGNAL, sa(&a), SIN_LEN)?, ENOTSOCK)
}

// ---------------------------------------------------------------------------
// TCP.

fn tcp_socket() -> CaseResult {
    let s = tcp()?;
    let st = fstat_of(s.0)?;
    check(st_mode(&st) & S_IFMT == S_IFSOCK, &format!("fstat of the socket reports mode {:o}, not S_IFSOCK", st_mode(&st)))
}

fn tcp_listen() -> CaseResult {
    let s = tcp()?;
    zero("bind to 127.0.0.1", bind_in(&s, LOOPBACK, port(0))?)?;
    zero("listen(8)", listen(s.0, 8)?)
}

/// Linux binds a listening socket that was never bound to an ephemeral port. POSIX lets
/// listen fail with EDESTADDRREQ there instead; Breenix follows Linux.
fn tcp_listen_autobind() -> CaseResult {
    let s = tcp()?;
    zero("listen on an unbound socket", listen(s.0, 4)?)?;
    let st = name_of(&s, false)?;
    check(st.family() == AF_INET && st.port() != 0, &format!("getsockname after listen reported {}, expected an ephemeral port", st.text()))
}

fn tcp_connect_accept() -> CaseResult {
    let t = tcp_pair()?;
    check(t.s.0 != t.l.0 && t.s.0 != t.c.0, &format!("accept returned descriptor {}, which is not new", t.s.0))
}

fn tcp_accept_address() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let mut st = Store::new();
    let s = Fd(ok("accept", accept(l.0, st.ptr(), &mut st.len)?)? as i32);
    let _ = &s;
    is_in("accept's peer address", &st, LOOPBACK, None)
}

fn tcp_getpeername_client() -> CaseResult {
    let t = tcp_pair()?;
    is_in("getpeername of the connecting socket", &name_of(&t.c, true)?, LOOPBACK, Some(t.port))
}

fn tcp_getpeername_server() -> CaseResult {
    let t = tcp_pair()?;
    let peer = name_of(&t.s, true)?;
    let client = name_of(&t.c, false)?;
    check(peer.family() == AF_INET && peer.addr() == client.addr() && peer.port() == client.port() && peer.port() != 0,
        &format!("getpeername of the accepted socket reported {}, but the client is bound to {}", peer.text(), client.text()))
}

fn tcp_getsockname_accepted() -> CaseResult {
    let t = tcp_pair()?;
    is_in("getsockname of the accepted socket", &name_of(&t.s, false)?, LOOPBACK, Some(t.port))
}

fn tcp_econnrefused() -> CaseResult {
    let s = tcp()?;
    let closed = port(9);
    let (r, _) = timed_call(ARRIVE_MS, "connect to a port nobody listens on", || connect_in(&s, LOOPBACK, closed))?;
    want("connect to a port nobody listens on", r, ECONNREFUSED)
}

/// Stream `len` bytes from `from`, in a process of its own that then closes it, to `to`,
/// which must read them in order and then end of file.
fn stream_check(from: Fd, to: &Fd, len: usize) -> CaseResult {
    let data = pattern(len);
    let mut writer = spawn(|| send_all(&from, &data, "the writer"))?;
    drop(from);
    let got = recv_exact(to, len, 8000, "the reader")?;
    if got != data { return fail(format!("the stream arrived out of order: {}", first_difference(&got, &data))); }
    writer.finish(2000, "the writer")?;
    expect_eof(to, "the reader after the writer closed")
}

fn tcp_transfer() -> CaseResult {
    let t = tcp_pair()?;
    stream_check(t.c, &t.s, 256 * 1024)
}

fn tcp_reverse() -> CaseResult {
    let t = tcp_pair()?;
    stream_check(t.s, &t.c, 64 * 1024)
}

fn tcp_eof() -> CaseResult {
    let t = tcp_pair()?;
    send_all(&t.c, b"bye", "the client")?;
    drop(t.c);
    let got = recv_exact(&t.s, 3, ARRIVE_MS, "the server")?;
    check(got == b"bye", "the server did not read the client's last bytes")?;
    expect_eof(&t.s, "the server after the client closed")
}

fn tcp_half_close() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_WR)", shutdown(t.c.0, SHUT_WR)?)?;
    expect_eof(&t.s, "the server after the client's SHUT_WR")
}

fn tcp_half_close_reverse() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_WR)", shutdown(t.c.0, SHUT_WR)?)?;
    expect_eof(&t.s, "the server after the client's SHUT_WR")?;
    send_all(&t.s, b"back", "the server")?;
    let got = recv_exact(&t.c, 4, ARRIVE_MS, "the half-closed client")?;
    check(got == b"back", "the half-closed client did not read what the server sent")
}

/// Wait until `n` bytes are queued unread on `fd`, peeking.
fn queued(fd: &Fd, n: usize, what: &str) -> CaseResult {
    let mut buf = vec![0u8; n];
    let ok_now = until(ARRIVE_MS, || matches!(recv(fd.0, buf.as_mut_ptr(), n, MSG_PEEK | MSG_DONTWAIT), Ok(r) if r == n as i64));
    check(ok_now, &format!("{what}: {n} bytes were not queued within {ARRIVE_MS} ms"))
}

fn tcp_rst_unread() -> CaseResult {
    let t = tcp_pair()?;
    send_all(&t.c, &pattern(16), "the client")?;
    queued(&t.s, 16, "the server")?;
    drop(t.s);
    let mut buf = [0u8; 8];
    match recv_wait(&t.c, &mut buf, 0, ARRIVE_MS)? {
        None => fail(format!("the client saw nothing within {ARRIVE_MS} ms of the server closing with unread data")),
        Some(n) => want("recv after the peer closed with unread data", n, ECONNRESET),
    }
}

/// After the peer has closed, a first send draws its RST and a later one fails with
/// EPIPE, as Linux reports a reset in CLOSE_WAIT.
fn tcp_epipe_after_close() -> CaseResult {
    let t = tcp_pair()?;
    drop(t.s);
    expect_eof(&t.c, "the client after the server closed")?;
    let n = send(t.c.0, b"a".as_ptr(), 1, MSG_NOSIGNAL)?;
    check(n == 1, &format!("the first send after the peer closed returned {}", shown(n)))?;
    let mut last = 1;
    until(ARRIVE_MS, || { last = send(t.c.0, b"b".as_ptr(), 1, MSG_NOSIGNAL).unwrap_or(-EINTR); last != 1 });
    want("send after the peer's reset", last, EPIPE)
}

fn tcp_sigpipe() -> CaseResult {
    let t = tcp_pair()?;
    drop(t.s);
    expect_eof(&t.c, "the client after the server closed")?;
    let c = t.c;
    let mut writer = spawn(|| {
        default_sigpipe()?;
        let start = now_ms();
        while now_ms().saturating_sub(start) < 2000 {
            let n = write(c.0, b"x".as_ptr(), 1)?;
            if n < 0 { return fail(format!("write failed with {} without raising SIGPIPE", shown(n))); }
            nap();
        }
        fail("writes to the reset connection kept succeeding for 2 s")
    })?;
    let status = writer.wait(3000, "the writer")?;
    check(status & 0x7f == SIGPIPE, &format!("the writer {}, expected to be killed by SIGPIPE{}", status_text(status),
        if status == 1 << 8 { format!(" ({})", writer.note.said()) } else { String::new() }))
}

fn tcp_nonblock_connect() -> CaseResult {
    let _l = listener(port(0))?;
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    want("a nonblocking connect", connect_in(&s, LOOPBACK, port(0))?, EINPROGRESS)
}

fn tcp_nonblock_connect_writable() -> CaseResult {
    let _l = listener(port(0))?;
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    let r = connect_in(&s, LOOPBACK, port(0))?;
    check(r == -EINPROGRESS || r == 0, &format!("the nonblocking connect returned {}", shown(r)))?;
    let rev = poll_until(&s, POLLOUT, POLLOUT | POLLERR)?;
    check(rev & POLLOUT != 0 && rev & POLLERR == 0, &format!("poll for POLLOUT reported revents {} within {ARRIVE_MS} ms", hex16(rev)))?;
    let e = get_int(&s, SOL_SOCKET, SO_ERROR, "SO_ERROR")?;
    check(e == 0, &format!("SO_ERROR after the connect completed is {}", errname(i64::from(e))))
}

fn tcp_eisconn() -> CaseResult {
    let t = tcp_pair()?;
    want("connect of a connected socket", connect_in(&t.c, LOOPBACK, t.port)?, EISCONN)
}

fn tcp_accept_eagain() -> CaseResult {
    let l = listener_of(SOCK_STREAM | SOCK_NONBLOCK, port(0), 8)?;
    want("accept on a nonblocking listener with nothing pending", accept(l.0, null_mut(), null_mut())?, EAGAIN)
}

fn tcp_accept_einval() -> CaseResult {
    let s = tcp()?;
    zero("bind", bind_in(&s, LOOPBACK, port(0))?)?;
    let r = accept(s.0, null_mut(), null_mut())?;
    let _a = adopt(r);
    want("accept on a socket that is not listening", r, EINVAL)
}

fn tcp_bind_eaddrinuse() -> CaseResult {
    let _l = listener(port(0))?;
    let b = tcp()?;
    want("bind to a listening socket's address and port", bind_in(&b, LOOPBACK, port(0))?, EADDRINUSE)
}

fn tcp_backlog() -> CaseResult {
    let l = listener_of(SOCK_STREAM, port(0), 4)?;
    let mut clients = Vec::new();
    for i in 0..4u8 {
        let c = connected(port(0)).map_err(|e| match e { CaseError::Fail(m) | CaseError::Skip(m) => CaseError::Fail(format!("connection {i}: {m}")) })?;
        send_all(&c, &[i], "a client")?;
        clients.push(c);
    }
    for i in 0..4u8 {
        let s = accepted(&l)?;
        let got = recv_exact(&s, 1, ARRIVE_MS, "an accepted connection")?;
        check(got == [i], &format!("accept {i} returned the connection that sent {}", got[0]))?;
    }
    Ok(())
}

/// Linux takes one connection on a listener whose backlog is 0; POSIX lets an
/// implementation round a backlog of 0 up to its own minimum.
fn tcp_backlog_zero() -> CaseResult {
    let l = listener_of(SOCK_STREAM, port(0), 0)?;
    let _c = connected(port(0))?;
    accepted(&l).map(|_| ())
}

fn tcp_accept4_nonblock() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let s = Fd(ok("accept4(SOCK_NONBLOCK)", accept4(l.0, null_mut(), null_mut(), SOCK_NONBLOCK)?)? as i32);
    let fl = fd_flags(s.0, F_GETFL)?;
    check(fl & O_NONBLOCK != 0, &format!("the accepted socket's status flags are {fl:#o}, without O_NONBLOCK"))
}

fn tcp_accept4_cloexec() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let s = Fd(ok("accept4(SOCK_CLOEXEC)", accept4(l.0, null_mut(), null_mut(), SOCK_CLOEXEC)?)? as i32);
    let fl = fd_flags(s.0, F_GETFD)?;
    check(fl & FD_CLOEXEC != 0, &format!("the accepted socket's descriptor flags are {fl:#x}, without FD_CLOEXEC"))
}

fn tcp_accept4_einval() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let r = accept4(l.0, null_mut(), null_mut(), 1)?;
    let _s = adopt(r);
    want("accept4 with flags 1", r, EINVAL)
}

/// On Linux the socket accept returns does not inherit the listener's O_NONBLOCK; BSD
/// systems pass it on, and POSIX leaves it open.
fn tcp_accept_no_inherit() -> CaseResult {
    let l = listener_of(SOCK_STREAM | SOCK_NONBLOCK, port(0), 8)?;
    let _c = connected(port(0))?;
    let s = accepted(&l)?;
    let fl = fd_flags(s.0, F_GETFL)?;
    check(fl & O_NONBLOCK == 0, &format!("the socket accepted from a nonblocking listener has status flags {fl:#o}, with O_NONBLOCK"))
}

fn tcp_socket_nonblock() -> CaseResult {
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    let fl = fd_flags(s.0, F_GETFL)?;
    check(fl & O_NONBLOCK != 0, &format!("the socket's status flags are {fl:#o}, without O_NONBLOCK"))
}

fn tcp_socket_cloexec() -> CaseResult {
    let s = sock(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, "AF_INET, SOCK_STREAM|SOCK_CLOEXEC")?;
    let fl = fd_flags(s.0, F_GETFD)?;
    check(fl & FD_CLOEXEC != 0, &format!("the socket's descriptor flags are {fl:#x}, without FD_CLOEXEC"))
}

fn tcp_recv_eagain() -> CaseResult {
    let t = tcp_pair()?;
    let mut buf = [0u8; 8];
    want("recv(MSG_DONTWAIT) with no data queued", recv(t.s.0, buf.as_mut_ptr(), 8, MSG_DONTWAIT)?, EAGAIN)
}

fn tcp_recv_enotconn() -> CaseResult {
    let s = tcp()?;
    let (r, _) = timed_call(ARRIVE_MS, "recv on a never-connected TCP socket", || {
        let mut buf = [0u8; 8];
        recv(s.0, buf.as_mut_ptr(), 8, 0)
    })?;
    want("recv on a never-connected TCP socket", r, ENOTCONN)
}

fn tcp_peek() -> CaseResult {
    let t = tcp_pair()?;
    send_all(&t.c, b"peekaboo", "the client")?;
    queued(&t.s, 8, "the server")?;
    let (n, got) = recv_some(&t.s, 4, MSG_PEEK, "the server")?;
    check(n == 4 && got == b"peek", &format!("recv(MSG_PEEK) of 4 bytes returned {}", shown(n)))?;
    let got = recv_exact(&t.s, 8, ARRIVE_MS, "the server")?;
    check(got == b"peekaboo", "the bytes after MSG_PEEK were not the whole stream from its start")
}

fn tcp_waitall() -> CaseResult {
    let t = tcp_pair()?;
    let c = t.c;
    let mut writer = spawn(|| {
        send_all(&c, b"abc", "the writer")?;
        sleep_ms(100);
        send_all(&c, b"defgh", "the writer")
    })?;
    let (r, _) = timed_call(ARRIVE_MS, "recv(MSG_WAITALL)", || {
        let mut buf = [0u8; 8];
        let n = recv(t.s.0, buf.as_mut_ptr(), 8, MSG_WAITALL)?;
        Ok(if n == 8 && &buf == b"abcdefgh" { 8 } else if n < 0 { n } else { 1000 + n })
    })?;
    writer.finish(2000, "the writer")?;
    check(r == 8, &format!("recv(MSG_WAITALL) of 8 bytes sent as 3 and then 5 returned {}",
        if r >= 1000 { format!("{} bytes that are not \"abcdefgh\"", r - 1000) } else { shown(r) }))
}

// ---------------------------------------------------------------------------
// Unix-domain sockets.

fn unix_stream_socket() -> CaseResult { usock(SOCK_STREAM).map(|_| ()) }
fn unix_dgram_socket() -> CaseResult { sock(AF_UNIX, SOCK_DGRAM, "AF_UNIX, SOCK_DGRAM").map(|_| ()) }
fn unix_seqpacket_socket() -> CaseResult { sock(AF_UNIX, SOCK_SEQPACKET, "AF_UNIX, SOCK_SEQPACKET").map(|_| ()) }

fn unix_socketpair() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    send_all(&a, b"ping", "the first end")?;
    check(recv_exact(&b, 4, ARRIVE_MS, "the second end")? == b"ping", "the second end read the wrong bytes")?;
    send_all(&b, b"pong", "the second end")?;
    check(recv_exact(&a, 4, ARRIVE_MS, "the first end")? == b"pong", "the first end read the wrong bytes")
}

fn unix_socketpair_dgram() -> CaseResult {
    let (a, b) = pair(SOCK_DGRAM)?;
    send_all(&a, b"one", "the first end")?;
    send_all(&a, b"seven!!", "the first end")?;
    let (n, got) = recv_some(&b, 64, 0, "the second end")?;
    check(n == 3 && got == b"one", &format!("the first datagram arrived as {} bytes", shown(n)))?;
    let (n, got) = recv_some(&b, 64, 0, "the second end")?;
    check(n == 7 && got == b"seven!!", &format!("the second datagram arrived as {} bytes", shown(n)))
}

fn unix_socketpair_eopnotsupp() -> CaseResult {
    let mut sv = [-1i32; 2];
    let r = socketpair(AF_INET, SOCK_STREAM, 0, sv.as_mut_ptr())?;
    if r == 0 { let _ = (Fd(sv[0]), Fd(sv[1])); }
    want("socketpair(AF_INET, SOCK_STREAM)", r, EOPNOTSUPP)
}

fn unix_socketpair_eof() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    drop(b);
    expect_eof(&a, "the remaining end")
}

fn unix_socketpair_flags() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC)?;
    for (end, fd) in [("first", &a), ("second", &b)] {
        let fl = fd_flags(fd.0, F_GETFL)?;
        check(fl & O_NONBLOCK != 0, &format!("the {end} end's status flags are {fl:#o}, without O_NONBLOCK"))?;
        let fd_fl = fd_flags(fd.0, F_GETFD)?;
        check(fd_fl & FD_CLOEXEC != 0, &format!("the {end} end's descriptor flags are {fd_fl:#x}, without FD_CLOEXEC"))?;
    }
    Ok(())
}

fn unix_bind_path() -> CaseResult {
    let a = un_path("bind");
    let s = usock(SOCK_STREAM)?;
    zero("bind to a path", un_bind(&s, &a)?)?;
    let mut st: StatBuf = [0; 32];
    ok("stat of the bound path", stat(a.file(), st.as_mut_ptr() as *mut u8)?)?;
    check(st_mode(&st) & S_IFMT == S_IFSOCK, &format!("stat of the bound path reports mode {:o}, not S_IFSOCK", st_mode(&st)))
}

fn unix_bind_eaddrinuse() -> CaseResult {
    let a = un_path("busy");
    let s = usock(SOCK_STREAM)?;
    zero("bind to a path", un_bind(&s, &a)?)?;
    let t = usock(SOCK_STREAM)?;
    want("a second bind to the same path", un_bind(&t, &a)?, EADDRINUSE)
}

fn unix_connect_accept() -> CaseResult {
    let a = un_path("conn");
    let l = un_listener(&a)?;
    let c = un_connected(&a)?;
    let s = accepted(&l)?;
    send_all(&c, b"unix", "the client")?;
    check(recv_exact(&s, 4, ARRIVE_MS, "the server")? == b"unix", "the server read the wrong bytes")
}

fn unix_connect_enoent() -> CaseResult {
    let a = un_path("missing");
    let c = usock(SOCK_STREAM)?;
    want("connect to a path that does not exist", un_connect(&c, &a)?, ENOENT)
}

fn unix_connect_not_listening() -> CaseResult {
    let a = un_path("quiet");
    let s = usock(SOCK_STREAM)?;
    zero("bind", un_bind(&s, &a)?)?;
    let c = usock(SOCK_STREAM)?;
    want("connect to a bound socket that is not listening", un_connect(&c, &a)?, ECONNREFUSED)
}

fn unix_connect_stale() -> CaseResult {
    let a = un_path("stale");
    drop(un_listener(&a)?);
    let c = usock(SOCK_STREAM)?;
    want("connect to the path a closed listener left", un_connect(&c, &a)?, ECONNREFUSED)
}

/// Linux refuses a connection to a path that is not a socket with ECONNREFUSED.
fn unix_connect_not_socket() -> CaseResult {
    let a = un_path("file");
    let f = ok("creating a regular file", open(a.file(), O_WRONLY | O_CREAT | O_TRUNC, 0o600)?)?;
    let _f = Fd(f as i32);
    let c = usock(SOCK_STREAM)?;
    want("connect to a regular file's path", un_connect(&c, &a)?, ECONNREFUSED)
}

fn unix_connect_eprototype() -> CaseResult {
    let a = un_path("dgram");
    let d = sock(AF_UNIX, SOCK_DGRAM, "AF_UNIX, SOCK_DGRAM")?;
    zero("bind of the datagram socket", un_bind(&d, &a)?)?;
    let c = usock(SOCK_STREAM)?;
    want("a stream connect to a datagram socket's path", un_connect(&c, &a)?, EPROTOTYPE)
}

fn unix_unlink_after_bind() -> CaseResult {
    let a = un_path("gone");
    let l = un_listener(&a)?;
    let c = un_connected(&a)?;
    let s = accepted(&l)?;
    zero("unlink of the bound path", unlink(a.file())?)?;
    let late = usock(SOCK_STREAM)?;
    want("connect after the path was unlinked", un_connect(&late, &a)?, ENOENT)?;
    send_all(&c, b"still", "the client")?;
    check(recv_exact(&s, 5, ARRIVE_MS, "the server")? == b"still", "the connection accepted before the unlink read the wrong bytes")
}

fn unix_getsockname_path() -> CaseResult {
    let a = un_path("name");
    let l = un_listener(&a)?;
    let st = name_of(&l, false)?;
    check(st.family() == AF_UNIX && st.len == a.len && st.path() == a.path(),
        &format!("getsockname reported {}, expected the bound path with length {}", st.text(), a.len))
}

fn unix_getpeername_path() -> CaseResult {
    let a = un_path("peer");
    let _l = un_listener(&a)?;
    let c = un_connected(&a)?;
    let st = name_of(&c, true)?;
    check(st.family() == AF_UNIX && st.len == a.len && st.path() == a.path(),
        &format!("getpeername of the client reported {}, expected the server's path with length {}", st.text(), a.len))
}

/// An unbound Unix socket's name is just its family: Linux reports length 2.
fn unix_getsockname_unbound() -> CaseResult {
    let s = usock(SOCK_STREAM)?;
    let st = name_of(&s, false)?;
    check(st.family() == AF_UNIX && st.len == 2, &format!("getsockname of an unbound socket reported {}, expected AF_UNIX with length 2", st.text()))
}

/// Abstract addresses, whose sun_path starts with a NUL, are Linux's; POSIX has none.
fn unix_abstract() -> CaseResult {
    let a = un_abstract("conn");
    let l = un_listener(&a)?;
    let c = un_connected(&a)?;
    let s = accepted(&l)?;
    send_all(&c, b"abstract", "the client")?;
    check(recv_exact(&s, 8, ARRIVE_MS, "the server")? == b"abstract", "the server read the wrong bytes")
}

fn unix_abstract_eaddrinuse() -> CaseResult {
    let a = un_abstract("busy");
    let s = usock(SOCK_STREAM)?;
    zero("bind to an abstract name", un_bind(&s, &a)?)?;
    let t = usock(SOCK_STREAM)?;
    want("a second bind to the same abstract name", un_bind(&t, &a)?, EADDRINUSE)
}

fn unix_abstract_getsockname() -> CaseResult {
    let a = un_abstract("name");
    let s = usock(SOCK_STREAM)?;
    zero("bind to an abstract name", un_bind(&s, &a)?)?;
    let st = name_of(&s, false)?;
    check(st.family() == AF_UNIX && st.len == a.len && st.path() == a.path(),
        &format!("getsockname reported {}, expected the abstract name with length {}", st.text(), a.len))
}

fn unix_abstract_released() -> CaseResult {
    let a = un_abstract("again");
    let s = usock(SOCK_STREAM)?;
    zero("bind to an abstract name", un_bind(&s, &a)?)?;
    drop(s);
    let t = usock(SOCK_STREAM)?;
    zero("bind to the abstract name after its socket closed", un_bind(&t, &a)?)
}

fn unix_dgram_path() -> CaseResult {
    let ra = un_path("rx");
    let sa_ = un_path("tx");
    let r = sock(AF_UNIX, SOCK_DGRAM, "AF_UNIX, SOCK_DGRAM")?;
    zero("bind of the receiver", un_bind(&r, &ra)?)?;
    let s = sock(AF_UNIX, SOCK_DGRAM, "AF_UNIX, SOCK_DGRAM")?;
    zero("bind of the sender", un_bind(&s, &sa_)?)?;
    let n = sendto(s.0, b"dg".as_ptr(), 2, MSG_NOSIGNAL, ra.ptr(), ra.len)?;
    check(n == 2, &format!("sendto the receiver's path returned {}", shown(n)))?;
    let (n, got, from) = recvfrom_wait(&r, 64, "the receiver")?;
    check(n == 2 && got == b"dg", &format!("the receiver got {} bytes", shown(n)))?;
    check(from.family() == AF_UNIX && from.path() == sa_.path(), &format!("recvfrom reported the sender as {}", from.text()))
}

fn unix_path_too_long() -> CaseResult {
    let s = usock(SOCK_STREAM)?;
    let mut buf = [0u8; 128];
    buf[0] = AF_UNIX as u8;
    buf[2..120].fill(b'x');
    want("bind with an address length of 111, beyond struct sockaddr_un", bind(s.0, buf.as_ptr(), SUN_LEN + 1)?, EINVAL)
}

/// Send descriptor `fd` and one byte over `sock` with SCM_RIGHTS.
fn send_fd(sock: &Fd, fd: i32) -> Result<i64, CaseError> {
    let mut byte = [b'F'];
    let mut iov = IoVec { base: byte.as_mut_ptr(), len: 1 };
    let mut cm = Cmsg { len: CMSG_LEN_FD, level: SOL_SOCKET, kind: SCM_RIGHTS, data: [0; 48] };
    cm.data[..4].copy_from_slice(&fd.to_ne_bytes());
    let msg = MsgHdr { name: null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: &mut cm as *mut Cmsg as *mut u8, controllen: CMSG_SPACE_FD, flags: 0 };
    sendmsg(sock.0, &msg, MSG_NOSIGNAL)
}

/// Receive one byte and a descriptor passed with SCM_RIGHTS: the descriptor.
fn recv_fd(sock: &Fd) -> Result<Fd, CaseError> {
    let ms = bounded(ARRIVE_MS, CLEANUP_MS);
    let start = now_ms();
    loop {
        let mut byte = [0u8];
        let mut iov = IoVec { base: byte.as_mut_ptr(), len: 1 };
        let mut cm = Cmsg { len: 0, level: 0, kind: 0, data: [0; 48] };
        let mut msg = MsgHdr { name: null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: &mut cm as *mut Cmsg as *mut u8, controllen: core::mem::size_of::<Cmsg>(), flags: 0 };
        let n = recvmsg(sock.0, &mut msg, MSG_DONTWAIT)?;
        if n == -EAGAIN && now_ms().saturating_sub(start) < ms { nap(); continue; }
        if n != 1 { return err(format!("recvmsg returned {}", shown(n))); }
        if msg.controllen < CMSG_LEN_FD || cm.len < CMSG_LEN_FD || cm.level != SOL_SOCKET || cm.kind != SCM_RIGHTS {
            return err(format!("recvmsg returned control data of {} bytes with level {} type {} and length {}, not one SCM_RIGHTS descriptor",
                msg.controllen, cm.level, cm.kind, cm.len));
        }
        return Ok(Fd(i32::from_ne_bytes([cm.data[0], cm.data[1], cm.data[2], cm.data[3]])));
    }
}

fn unix_scm_rights() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    let mut p = [-1i32; 2];
    ok("pipe", pipe(p.as_mut_ptr())?)?;
    let (r, w) = (Fd(p[0]), Fd(p[1]));
    let n = send_fd(&a, w.0)?;
    check(n == 1, &format!("sendmsg with SCM_RIGHTS returned {}", shown(n)))?;
    drop(w);
    let got = recv_fd(&b)?;
    let n = write(got.0, b"via".as_ptr(), 3)?;
    check(n == 3, &format!("write to the received descriptor returned {}", shown(n)))?;
    let mut buf = [0u8; 8];
    let n = read(r.0, buf.as_mut_ptr(), 8)?;
    check(n == 3 && &buf[..3] == b"via", &format!("the pipe's read end got {} bytes from the received descriptor", shown(n)))
}

fn unix_scm_rights_same_file() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    let mut p = [-1i32; 2];
    ok("pipe", pipe(p.as_mut_ptr())?)?;
    let (_r, w) = (Fd(p[0]), Fd(p[1]));
    let n = send_fd(&a, w.0)?;
    check(n == 1, &format!("sendmsg with SCM_RIGHTS returned {}", shown(n)))?;
    let got = recv_fd(&b)?;
    check(got.0 != w.0, &format!("the received descriptor is {}, the same number as the one still open", got.0))?;
    let (sent, recvd) = (st_ino(&fstat_of(w.0)?), st_ino(&fstat_of(got.0)?));
    check(sent == recvd, &format!("the received descriptor's inode is {recvd}, the sent one's {sent}"))
}

/// struct ucred, as SO_PEERCRED reports it.
fn peercred(fd: &Fd) -> Result<[u32; 3], CaseError> {
    let mut cred = [0u32; 3];
    let mut len = 12u32;
    ok("getsockopt(SO_PEERCRED)", getsockopt(fd.0, SOL_SOCKET, SO_PEERCRED, cred.as_mut_ptr() as *mut u8, &mut len)?)?;
    check(len == 12, &format!("getsockopt(SO_PEERCRED) returned length {len}, not 12"))?;
    Ok(cred)
}

fn unix_peercred_socketpair() -> CaseResult {
    let (a, _b) = pair(SOCK_STREAM)?;
    let cred = peercred(&a)?;
    let want_cred = [pid() as u32, sc(nr::GETUID, &[]) as u32, sc(nr::GETGID, &[]) as u32];
    check(cred == want_cred, &format!("SO_PEERCRED reported pid {} uid {} gid {}, expected pid {} uid {} gid {}",
        cred[0], cred[1], cred[2], want_cred[0], want_cred[1], want_cred[2]))
}

fn unix_peercred_connect() -> CaseResult {
    let a = un_path("cred");
    let l = un_listener(&a)?;
    let child = spawn(|| {
        let _c = un_connected(&a)?;
        sleep_ms(8000);
        Ok(())
    })?;
    let s = accepted(&l)?;
    let cred = peercred(&s)?;
    check(cred[0] == child.pid as u32, &format!("SO_PEERCRED on the accepted socket reported pid {}, but the connecting process is {}", cred[0], child.pid))
}

fn unix_epipe() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    drop(b);
    want("send to a socketpair end whose peer closed", send(a.0, b"x".as_ptr(), 1, MSG_NOSIGNAL)?, EPIPE)
}

// ---------------------------------------------------------------------------
// Socket options and shutdown.

fn opt_so_type_stream() -> CaseResult {
    let s = tcp()?;
    let v = get_int(&s, SOL_SOCKET, SO_TYPE, "SO_TYPE")?;
    check(v == SOCK_STREAM, &format!("SO_TYPE of a TCP socket is {v}, not SOCK_STREAM (1)"))
}

fn opt_so_type_dgram() -> CaseResult {
    let s = udp()?;
    let v = get_int(&s, SOL_SOCKET, SO_TYPE, "SO_TYPE")?;
    check(v == SOCK_DGRAM, &format!("SO_TYPE of a UDP socket is {v}, not SOCK_DGRAM (2)"))
}

fn opt_so_protocol() -> CaseResult {
    let s = tcp()?;
    let v = get_int(&s, SOL_SOCKET, SO_PROTOCOL, "SO_PROTOCOL")?;
    check(v == IPPROTO_TCP, &format!("SO_PROTOCOL of a TCP socket is {v}, not IPPROTO_TCP (6)"))
}

fn opt_so_domain() -> CaseResult {
    let s = usock(SOCK_STREAM)?;
    let v = get_int(&s, SOL_SOCKET, SO_DOMAIN, "SO_DOMAIN")?;
    check(v == AF_UNIX, &format!("SO_DOMAIN of a Unix socket is {v}, not AF_UNIX (1)"))
}

fn opt_so_acceptconn() -> CaseResult {
    let s = tcp()?;
    zero("bind", bind_in(&s, LOOPBACK, port(0))?)?;
    let before = get_int(&s, SOL_SOCKET, SO_ACCEPTCONN, "SO_ACCEPTCONN")?;
    zero("listen", listen(s.0, 4)?)?;
    let after = get_int(&s, SOL_SOCKET, SO_ACCEPTCONN, "SO_ACCEPTCONN")?;
    check(before == 0 && after == 1, &format!("SO_ACCEPTCONN was {before} before listen and {after} after, expected 0 and 1"))
}

fn opt_so_reuseaddr() -> CaseResult {
    let s = tcp()?;
    let before = get_int(&s, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    set_on(&s, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    let after = get_int(&s, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    check(before == 0 && after != 0, &format!("SO_REUSEADDR read {before} before it was set and {after} after"))
}

/// Close a TCP connection from the server's side first, so the server's end of it waits
/// in TIME_WAIT on the listening port, and then close the listener too.
fn leave_time_wait(reuse: bool) -> Result<u16, CaseError> {
    let port = port(0);
    let l = tcp()?;
    if reuse { set_on(&l, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?; }
    zero(&format!("bind to 127.0.0.1:{port}"), bind_in(&l, LOOPBACK, port)?)?;
    zero("listen", listen(l.0, 4)?)?;
    let c = connected(port)?;
    let s = accepted(&l)?;
    drop(s);
    expect_eof(&c, "the client after the server closed")?;
    drop(c);
    drop(l);
    sleep_ms(50);
    Ok(port)
}

fn opt_reuseaddr_time_wait() -> CaseResult {
    let port = leave_time_wait(true)?;
    let n = tcp()?;
    set_on(&n, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    zero("bind with SO_REUSEADDR to a port whose connection is in TIME_WAIT", bind_in(&n, LOOPBACK, port)?)
}

fn opt_time_wait_eaddrinuse() -> CaseResult {
    let port = leave_time_wait(false)?;
    let n = tcp()?;
    want("bind without SO_REUSEADDR to a port whose connection is in TIME_WAIT", bind_in(&n, LOOPBACK, port)?, EADDRINUSE)
}

/// Linux lets UDP sockets that all set SO_REUSEADDR share an address and port.
fn opt_reuseaddr_udp() -> CaseResult {
    let a = udp()?;
    set_on(&a, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    zero("bind of the first socket", bind_in(&a, LOOPBACK, port(0))?)?;
    let b = udp()?;
    set_on(&b, SOL_SOCKET, SO_REUSEADDR, "SO_REUSEADDR")?;
    zero("bind of a second socket with SO_REUSEADDR to the same address and port", bind_in(&b, LOOPBACK, port(0))?)
}

/// Linux doubles a buffer size it is given, to leave room for its own bookkeeping, and
/// reports the doubled size; POSIX leaves the value read back open.
fn opt_buffer(name: i32, what: &str) -> CaseResult {
    let s = tcp()?;
    zero(&format!("setsockopt({what}, 32768)"), set_int(&s, SOL_SOCKET, name, 32768)?)?;
    let v = get_int(&s, SOL_SOCKET, name, what)?;
    check(v == 65536, &format!("{what} set to 32768 reads back {v}, expected Linux's doubled 65536"))
}
fn opt_so_rcvbuf() -> CaseResult { opt_buffer(SO_RCVBUF, "SO_RCVBUF") }
fn opt_so_sndbuf() -> CaseResult { opt_buffer(SO_SNDBUF, "SO_SNDBUF") }

fn opt_so_rcvtimeo() -> CaseResult {
    let s = udp()?;
    zero("setsockopt(SO_RCVTIMEO, 250 ms)", set_tv(&s, SO_RCVTIMEO, [0, 250_000])?)?;
    let mut tv: Tv = [0; 2];
    let mut len = 16u32;
    ok("getsockopt(SO_RCVTIMEO)", getsockopt(s.0, SOL_SOCKET, SO_RCVTIMEO, tv.as_mut_ptr() as *mut u8, &mut len)?)?;
    check(len == 16 && tv == [0, 250_000], &format!("SO_RCVTIMEO set to 250 ms reads back {} s {} us (length {len})", tv[0], tv[1]))
}

fn opt_so_rcvtimeo_eagain() -> CaseResult {
    let s = udp_at(port(0))?;
    zero("setsockopt(SO_RCVTIMEO, 200 ms)", set_tv(&s, SO_RCVTIMEO, [0, 200_000])?)?;
    let (r, took) = timed_call(2000, "recv with a 200 ms SO_RCVTIMEO", || {
        let mut buf = [0u8; 8];
        recv(s.0, buf.as_mut_ptr(), 8, 0)
    })?;
    want("recv with a 200 ms SO_RCVTIMEO and no data", r, EAGAIN)?;
    on_time("recv with SO_RCVTIMEO", took, 200)
}

fn opt_so_sndtimeo_eagain() -> CaseResult {
    let (a, _b) = pair(SOCK_STREAM)?;
    let chunk = vec![0u8; 4096];
    let mut sent = 0usize;
    let full = until(3000, || match send(a.0, chunk.as_ptr(), chunk.len(), MSG_DONTWAIT | MSG_NOSIGNAL) {
        Ok(n) if n > 0 => { sent += n as usize; sent > 64 << 20 }
        _ => true,
    });
    let probe = send(a.0, chunk.as_ptr(), 1, MSG_DONTWAIT | MSG_NOSIGNAL)?;
    check(full && probe == -EAGAIN, &format!("the socket did not fill: after {sent} bytes a further send(MSG_DONTWAIT) returned {}", shown(probe)))?;
    zero("setsockopt(SO_SNDTIMEO, 200 ms)", set_tv(&a, SO_SNDTIMEO, [0, 200_000])?)?;
    let (r, took) = timed_call(2000, "send with a 200 ms SO_SNDTIMEO", || send(a.0, b"x".as_ptr(), 1, MSG_NOSIGNAL))?;
    want("send on a full socket with a 200 ms SO_SNDTIMEO", r, EAGAIN)?;
    on_time("send with SO_SNDTIMEO", took, 200)
}

/// Linux refuses a timeval whose microseconds are not below a million with EDOM.
fn opt_so_rcvtimeo_edom() -> CaseResult {
    let s = udp()?;
    want("setsockopt(SO_RCVTIMEO) with tv_usec 1000000", set_tv(&s, SO_RCVTIMEO, [0, 1_000_000])?, EDOM)
}

fn opt_so_linger() -> CaseResult {
    let s = tcp()?;
    let set: [i32; 2] = [1, 5];
    zero("setsockopt(SO_LINGER, on, 5 s)", setsockopt(s.0, SOL_SOCKET, SO_LINGER, set.as_ptr() as *const u8, 8)?)?;
    let mut got = [0i32; 2];
    let mut len = 8u32;
    ok("getsockopt(SO_LINGER)", getsockopt(s.0, SOL_SOCKET, SO_LINGER, got.as_mut_ptr() as *mut u8, &mut len)?)?;
    check(len == 8 && got[0] != 0 && got[1] == 5, &format!("SO_LINGER reads back l_onoff {} l_linger {} (length {len})", got[0], got[1]))
}

fn opt_so_linger_reset() -> CaseResult {
    let t = tcp_pair()?;
    let set: [i32; 2] = [1, 0];
    zero("setsockopt(SO_LINGER, on, 0 s)", setsockopt(t.c.0, SOL_SOCKET, SO_LINGER, set.as_ptr() as *const u8, 8)?)?;
    drop(t.c);
    let mut buf = [0u8; 8];
    match recv_wait(&t.s, &mut buf, 0, ARRIVE_MS)? {
        None => fail(format!("the peer saw nothing within {ARRIVE_MS} ms of a close with a zero linger")),
        Some(n) => want("recv after the peer closed with SO_LINGER 0", n, ECONNRESET),
    }
}

fn opt_so_error() -> CaseResult {
    let s = tcp()?;
    let v = get_int(&s, SOL_SOCKET, SO_ERROR, "SO_ERROR")?;
    check(v == 0, &format!("SO_ERROR of a new socket is {}", errname(i64::from(v))))
}

/// Linux reports a refused loopback connect made without blocking as EINPROGRESS and
/// then leaves the refusal in SO_ERROR; POSIX would also allow ECONNREFUSED at once.
fn opt_so_error_refused() -> CaseResult {
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    want("a nonblocking connect to a port nobody listens on", connect_in(&s, LOOPBACK, port(9))?, EINPROGRESS)?;
    let mut e = 0;
    let mut bad = None;
    until(ARRIVE_MS, || match get_int(&s, SOL_SOCKET, SO_ERROR, "SO_ERROR") { Ok(v) => { e = v; v != 0 } Err(x) => { bad = Some(x); true } });
    if let Some(x) = bad { return Err(x); }
    check(i64::from(e) == ECONNREFUSED, &format!("SO_ERROR after the refused connect is {}, expected ECONNREFUSED", errname(i64::from(e))))?;
    let again = get_int(&s, SOL_SOCKET, SO_ERROR, "SO_ERROR")?;
    check(again == 0, &format!("SO_ERROR read a second time is {}: reading it did not clear it", errname(i64::from(again))))
}

fn opt_tcp_nodelay() -> CaseResult {
    let s = tcp()?;
    let before = get_int(&s, IPPROTO_TCP, TCP_NODELAY, "TCP_NODELAY")?;
    set_on(&s, IPPROTO_TCP, TCP_NODELAY, "TCP_NODELAY")?;
    let after = get_int(&s, IPPROTO_TCP, TCP_NODELAY, "TCP_NODELAY")?;
    check(before == 0 && after != 0, &format!("TCP_NODELAY read {before} before it was set and {after} after"))
}

fn opt_tcp_nodelay_udp() -> CaseResult {
    let s = udp()?;
    want("setsockopt(IPPROTO_TCP, TCP_NODELAY) on a UDP socket", set_int(&s, IPPROTO_TCP, TCP_NODELAY, 1)?, ENOPROTOOPT)
}

fn opt_so_keepalive() -> CaseResult {
    let s = tcp()?;
    set_on(&s, SOL_SOCKET, SO_KEEPALIVE, "SO_KEEPALIVE")?;
    let v = get_int(&s, SOL_SOCKET, SO_KEEPALIVE, "SO_KEEPALIVE")?;
    check(v != 0, "SO_KEEPALIVE reads back 0 after it was set")
}

fn opt_so_broadcast() -> CaseResult {
    let s = udp()?;
    let before = get_int(&s, SOL_SOCKET, SO_BROADCAST, "SO_BROADCAST")?;
    set_on(&s, SOL_SOCKET, SO_BROADCAST, "SO_BROADCAST")?;
    let after = get_int(&s, SOL_SOCKET, SO_BROADCAST, "SO_BROADCAST")?;
    check(before == 0 && after != 0, &format!("SO_BROADCAST read {before} before it was set and {after} after"))
}

fn opt_enoprotoopt_get() -> CaseResult {
    let s = tcp()?;
    let mut v = 0i32;
    let mut len = 4u32;
    want("getsockopt(SOL_SOCKET, 9999)", getsockopt(s.0, SOL_SOCKET, 9999, &mut v as *mut i32 as *mut u8, &mut len)?, ENOPROTOOPT)
}

fn opt_enoprotoopt_set() -> CaseResult {
    let s = tcp()?;
    want("setsockopt(SOL_SOCKET, 9999)", set_int(&s, SOL_SOCKET, 9999, 1)?, ENOPROTOOPT)
}

fn opt_enotsock() -> CaseResult {
    let mut p = [-1i32; 2];
    ok("pipe", pipe(p.as_mut_ptr())?)?;
    let (r, _w) = (Fd(p[0]), Fd(p[1]));
    let mut v = 0i32;
    let mut len = 4u32;
    want("getsockopt(SO_TYPE) on a pipe", getsockopt(r.0, SOL_SOCKET, SO_TYPE, &mut v as *mut i32 as *mut u8, &mut len)?, ENOTSOCK)
}

fn opt_optlen_einval() -> CaseResult {
    let s = tcp()?;
    let v = 1i32;
    want("setsockopt(SO_REUSEADDR) with an option length of 2", setsockopt(s.0, SOL_SOCKET, SO_REUSEADDR, &v as *const i32 as *const u8, 2)?, EINVAL)
}

fn opt_shut_rd() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_RD)", shutdown(t.s.0, SHUT_RD)?)?;
    let (r, _) = timed_call(ARRIVE_MS, "recv after SHUT_RD", || {
        let mut buf = [0u8; 8];
        recv(t.s.0, buf.as_mut_ptr(), 8, 0)
    })?;
    check(r == 0, &format!("recv after SHUT_RD returned {}, expected 0", shown(r)))
}

fn opt_shut_wr() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_WR)", shutdown(t.c.0, SHUT_WR)?)?;
    want("send after SHUT_WR", send(t.c.0, b"x".as_ptr(), 1, MSG_NOSIGNAL)?, EPIPE)
}

fn opt_shut_rdwr() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_RDWR)", shutdown(t.c.0, SHUT_RDWR)?)?;
    want("send after SHUT_RDWR", send(t.c.0, b"x".as_ptr(), 1, MSG_NOSIGNAL)?, EPIPE)?;
    let (r, _) = timed_call(ARRIVE_MS, "recv after SHUT_RDWR", || {
        let mut buf = [0u8; 8];
        recv(t.c.0, buf.as_mut_ptr(), 8, 0)
    })?;
    check(r == 0, &format!("recv after SHUT_RDWR returned {}, expected 0", shown(r)))
}

fn opt_shutdown_enotconn() -> CaseResult {
    let s = tcp()?;
    want("shutdown of an unconnected TCP socket", shutdown(s.0, SHUT_RDWR)?, ENOTCONN)
}

fn opt_shutdown_einval() -> CaseResult {
    let t = tcp_pair()?;
    want("shutdown with how 3", shutdown(t.c.0, 3)?, EINVAL)
}

// ---------------------------------------------------------------------------
// poll, select and epoll on sockets.

fn mux_poll_udp_idle() -> CaseResult {
    let s = udp_at(port(0))?;
    let (n, rev) = poll_one(&s, POLLIN, 0)?;
    check(n == 0 && rev == 0, &format!("poll for POLLIN on an empty UDP socket returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_udp_readable() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    send_dgram(&a, b"ready", port(1))?;
    let (n, rev) = poll_one(&b, POLLIN, ARRIVE_MS as i32)?;
    check(n == 1 && rev & POLLIN != 0, &format!("poll for POLLIN after a datagram was sent returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_udp_writable() -> CaseResult {
    let s = udp_at(port(0))?;
    let (n, rev) = poll_one(&s, POLLOUT, 0)?;
    check(n == 1 && rev & POLLOUT != 0, &format!("poll for POLLOUT on an idle UDP socket returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_tcp_idle() -> CaseResult {
    let t = tcp_pair()?;
    let (n, rev) = poll_one(&t.s, POLLIN, 0)?;
    check(n == 0 && rev == 0, &format!("poll for POLLIN on a connection with no data returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_tcp_readable() -> CaseResult {
    let t = tcp_pair()?;
    send_all(&t.c, b"data", "the client")?;
    let (n, rev) = poll_one(&t.s, POLLIN, ARRIVE_MS as i32)?;
    check(n == 1 && rev & POLLIN != 0, &format!("poll for POLLIN after the peer wrote returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_tcp_writable() -> CaseResult {
    let t = tcp_pair()?;
    let (n, rev) = poll_one(&t.c, POLLOUT, 0)?;
    check(n == 1 && rev & POLLOUT != 0, &format!("poll for POLLOUT on a new connection returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_accept_ready() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let (n, rev) = poll_one(&l, POLLIN, ARRIVE_MS as i32)?;
    check(n == 1 && rev & POLLIN != 0, &format!("poll for POLLIN on a listener with a pending connection returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_accept_idle() -> CaseResult {
    let l = listener(port(0))?;
    let start = mono();
    let (n, rev) = poll_one(&l, POLLIN, 50)?;
    let took = mono() - start;
    check(n == 0 && rev == 0, &format!("poll on a listener with nothing pending returned {} with revents {}", shown(n), hex16(rev)))?;
    on_time("poll with a 50 ms timeout", took, 50)
}

fn mux_poll_eof() -> CaseResult {
    let t = tcp_pair()?;
    drop(t.c);
    let rev = poll_until(&t.s, POLLIN, POLLIN)?;
    check(rev & POLLIN != 0, &format!("poll for POLLIN after the peer closed reported revents {} within {ARRIVE_MS} ms", hex16(rev)))?;
    let mut buf = [0u8; 8];
    let n = recv(t.s.0, buf.as_mut_ptr(), 8, MSG_DONTWAIT)?;
    check(n == 0, &format!("recv on the socket poll reported readable returned {}, expected end of file", shown(n)))
}

/// POLLRDHUP, the peer's shutdown of its writing side, is Linux's.
fn mux_poll_rdhup() -> CaseResult {
    let t = tcp_pair()?;
    zero("shutdown(SHUT_WR)", shutdown(t.c.0, SHUT_WR)?)?;
    let rev = poll_until(&t.s, POLLIN | POLLRDHUP, POLLRDHUP)?;
    check(rev & POLLRDHUP != 0, &format!("poll for POLLRDHUP after the peer's SHUT_WR reported revents {}", hex16(rev)))
}

fn mux_poll_hup_unix() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    drop(b);
    let rev = poll_until(&a, POLLIN, POLLHUP)?;
    check(rev & POLLHUP != 0, &format!("poll on a socketpair end whose peer closed reported revents {}, without POLLHUP", hex16(rev)))
}

fn mux_poll_hup_tcp() -> CaseResult {
    let t = tcp_pair()?;
    drop(t.c);
    expect_eof(&t.s, "the server after the client closed")?;
    zero("shutdown(SHUT_WR)", shutdown(t.s.0, SHUT_WR)?)?;
    let rev = poll_until(&t.s, POLLIN, POLLHUP)?;
    check(rev & POLLHUP != 0, &format!("poll on a connection shut down both ways reported revents {}, without POLLHUP", hex16(rev)))
}

fn mux_poll_err_refused() -> CaseResult {
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    let r = connect_in(&s, LOOPBACK, port(9))?;
    check(r == -EINPROGRESS || r == -ECONNREFUSED, &format!("the nonblocking connect returned {}", shown(r)))?;
    let rev = poll_until(&s, POLLOUT, POLLERR)?;
    check(rev & POLLERR != 0, &format!("poll on a socket whose connect was refused reported revents {}, without POLLERR", hex16(rev)))
}

fn mux_poll_unix_dgram() -> CaseResult {
    let (a, b) = pair(SOCK_DGRAM)?;
    send_all(&a, b"dg", "the first end")?;
    let (n, rev) = poll_one(&b, POLLIN, ARRIVE_MS as i32)?;
    check(n == 1 && rev & POLLIN != 0, &format!("poll for POLLIN on a Unix datagram socket with a datagram queued returned {} with revents {}", shown(n), hex16(rev)))
}

fn mux_poll_wakes_data() -> CaseResult {
    let t = tcp_pair()?;
    let note = page()?;
    let c = t.c;
    let mut writer = spawn(|| {
        sleep_ms(100);
        note.set(0, mono());
        send_all(&c, b"x", "the writer")
    })?;
    let (n, rev) = poll_one(&t.s, POLLIN, ARRIVE_MS as i32)?;
    let woke = mono();
    writer.finish(2000, "the writer")?;
    check(n == 1 && rev & POLLIN != 0, &format!("poll for POLLIN while another process wrote returned {} with revents {}", shown(n), hex16(rev)))?;
    woken("poll", woke, note.get(0))
}

fn mux_poll_wakes_accept() -> CaseResult {
    let port = port(0);
    let l = listener(port)?;
    let note = page()?;
    let _client = spawn(|| {
        sleep_ms(100);
        note.set(0, mono());
        let _c = connected(port)?;
        sleep_ms(8000);
        Ok(())
    })?;
    let (n, rev) = poll_one(&l, POLLIN, ARRIVE_MS as i32)?;
    let woke = mono();
    check(n == 1 && rev & POLLIN != 0, &format!("poll on the listener while another process connected returned {} with revents {}", shown(n), hex16(rev)))?;
    woken("poll", woke, note.get(0))
}

fn mux_poll_many() -> CaseResult {
    let sender = udp()?;
    let socks: Vec<Fd> = (0..8).map(|i| udp_at(port(1 + i))).collect::<Result<_, _>>()?;
    let busy = [1usize, 4, 6];
    for &i in &busy { send_dgram(&sender, b"m", port(1 + i as u16))?; }
    let mut fds: Vec<PollFd> = socks.iter().map(|s| PollFd { fd: s.0, events: POLLIN, revents: 0 }).collect();
    let mut n = 0;
    until(ARRIVE_MS, || {
        for p in fds.iter_mut() { p.revents = 0; }
        n = poll(fds.as_mut_ptr(), fds.len() as u64, 0).unwrap_or(-EINTR);
        n == 3
    });
    let ready: Vec<usize> = fds.iter().enumerate().filter(|(_, p)| p.revents & POLLIN != 0).map(|(i, _)| i).collect();
    check(n == 3 && ready == busy, &format!("poll on 8 sockets, 3 with a datagram (1, 4, 6), returned {} and reported {:?} readable", shown(n), ready))
}

fn mux_select_idle() -> CaseResult {
    let t = tcp_pair()?;
    let (n, set) = select_one(&t.s, false, 0)?;
    check(n == 0 && !set, &format!("select for reading on a connection with no data returned {}", shown(n)))
}

fn mux_select_readable() -> CaseResult {
    let t = tcp_pair()?;
    send_all(&t.c, b"data", "the client")?;
    let (n, set) = select_one(&t.s, false, ARRIVE_MS as i64)?;
    check(n == 1 && set, &format!("select for reading after the peer wrote returned {}{}", shown(n), if set { "" } else { " without the socket in its set" }))
}

fn mux_select_writable() -> CaseResult {
    let t = tcp_pair()?;
    let (n, set) = select_one(&t.c, true, 0)?;
    check(n == 1 && set, &format!("select for writing on a new connection returned {}", shown(n)))
}

fn mux_select_accept_ready() -> CaseResult {
    let l = listener(port(0))?;
    let _c = connected(port(0))?;
    let (n, set) = select_one(&l, false, ARRIVE_MS as i64)?;
    check(n == 1 && set, &format!("select for reading on a listener with a pending connection returned {}", shown(n)))
}

fn mux_select_eof() -> CaseResult {
    let t = tcp_pair()?;
    drop(t.c);
    let (n, set) = select_one(&t.s, false, ARRIVE_MS as i64)?;
    check(n == 1 && set, &format!("select for reading after the peer closed returned {}", shown(n)))
}

fn mux_select_timeout() -> CaseResult {
    let s = udp_at(port(0))?;
    let start = mono();
    let (n, set) = select_one(&s, false, 100)?;
    let took = mono() - start;
    check(n == 0 && !set, &format!("select on an empty socket returned {}", shown(n)))?;
    on_time("select with a 100 ms timeout", took, 100)
}

/// POSIX marks a socket writable once a nonblocking connect has finished, whether it
/// succeeded or failed with an error left pending.
fn mux_select_connect(refused: bool) -> CaseResult {
    let _l = if refused { None } else { Some(listener(port(0))?) };
    let s = sock(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, "AF_INET, SOCK_STREAM|SOCK_NONBLOCK")?;
    let r = connect_in(&s, LOOPBACK, if refused { port(9) } else { port(0) })?;
    check(r == -EINPROGRESS || r == 0 || (refused && r == -ECONNREFUSED), &format!("the nonblocking connect returned {}", shown(r)))?;
    let (n, set) = select_one(&s, true, ARRIVE_MS as i64)?;
    check(n == 1 && set, &format!("select for writing after the nonblocking connect returned {}", shown(n)))?;
    if r == -ECONNREFUSED { return Ok(()); }
    let e = i64::from(get_int(&s, SOL_SOCKET, SO_ERROR, "SO_ERROR")?);
    let want_e = if refused { ECONNREFUSED } else { 0 };
    check(e == want_e, &format!("SO_ERROR once select reported the socket writable is {}, expected {}",
        if e == 0 { "0".into() } else { errname(e) }, if want_e == 0 { "0".into() } else { errname(want_e) }))
}
fn mux_select_connect_writable() -> CaseResult { mux_select_connect(false) }
fn mux_select_connect_refused() -> CaseResult { mux_select_connect(true) }

/// An epoll instance watching `fd` for `events`, its data the descriptor.
fn epoll_on(fd: &Fd, events: u32) -> Result<Fd, CaseError> {
    let ep = Fd(ok("epoll_create1", epoll_create1(0)?)? as i32);
    let mut ev = EpollEvent { events, data: fd.0 as u64 };
    zero("epoll_ctl(EPOLL_CTL_ADD)", epoll_ctl(ep.0, EPOLL_CTL_ADD, fd.0, &mut ev)?)?;
    Ok(ep)
}

/// epoll_wait for one event with a timeout in ms: the count and the event.
fn epoll_one(ep: &Fd, timeout: i32) -> Result<(i64, EpollEvent), CaseError> {
    let mut ev = [EpollEvent::default(); 4];
    let n = epoll_wait(ep.0, ev.as_mut_ptr(), 4, timeout)?;
    Ok((n, ev[0]))
}

fn ev_text(n: i64, ev: EpollEvent) -> String {
    let (events, data) = (ev.events, ev.data);
    format!("{} with events {events:#x} data {data}", shown(n))
}

fn mux_epoll_readable() -> CaseResult {
    let t = tcp_pair()?;
    let ep = epoll_on(&t.s, EPOLLIN)?;
    send_all(&t.c, b"data", "the client")?;
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    let (events, data) = (ev.events, ev.data);
    check(n == 1 && events & EPOLLIN != 0 && data == t.s.0 as u64, &format!("epoll_wait after the peer wrote returned {}", ev_text(n, ev)))
}

fn mux_epoll_accept() -> CaseResult {
    let l = listener(port(0))?;
    let ep = epoll_on(&l, EPOLLIN)?;
    let _c = connected(port(0))?;
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    let events = ev.events;
    check(n == 1 && events & EPOLLIN != 0, &format!("epoll_wait on a listener with a pending connection returned {}", ev_text(n, ev)))
}

fn mux_epoll_timeout() -> CaseResult {
    let s = udp_at(port(0))?;
    let ep = epoll_on(&s, EPOLLIN)?;
    let start = mono();
    let (n, ev) = epoll_one(&ep, 100)?;
    let took = mono() - start;
    check(n == 0, &format!("epoll_wait on an empty socket returned {}", ev_text(n, ev)))?;
    on_time("epoll_wait with a 100 ms timeout", took, 100)
}

fn mux_epoll_edge() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    let ep = epoll_on(&b, EPOLLIN | EPOLLET)?;
    send_dgram(&a, b"edge", port(1))?;
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    check(n == 1, &format!("the first epoll_wait after a datagram arrived returned {}", ev_text(n, ev)))?;
    let (n, ev) = epoll_one(&ep, 0)?;
    check(n == 0, &format!("a second edge-triggered epoll_wait with nothing new returned {}", ev_text(n, ev)))
}

fn mux_epoll_oneshot() -> CaseResult {
    let a = udp()?;
    let b = udp_at(port(1))?;
    let ep = epoll_on(&b, EPOLLIN | EPOLLONESHOT)?;
    send_dgram(&a, b"one", port(1))?;
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    check(n == 1, &format!("the first epoll_wait returned {}", ev_text(n, ev)))?;
    let (n, ev) = epoll_one(&ep, 50)?;
    check(n == 0, &format!("epoll_wait after the one-shot event fired returned {}", ev_text(n, ev)))?;
    let mut re = EpollEvent { events: EPOLLIN | EPOLLONESHOT, data: b.0 as u64 };
    zero("epoll_ctl(EPOLL_CTL_MOD) to re-arm", epoll_ctl(ep.0, EPOLL_CTL_MOD, b.0, &mut re)?)?;
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    check(n == 1, &format!("epoll_wait after re-arming returned {}", ev_text(n, ev)))
}

fn mux_epoll_hup() -> CaseResult {
    let (a, b) = pair(SOCK_STREAM)?;
    let ep = epoll_on(&a, EPOLLIN)?;
    drop(b);
    let (n, ev) = epoll_one(&ep, ARRIVE_MS as i32)?;
    let events = ev.events;
    check(n == 1 && events & EPOLLHUP != 0, &format!("epoll_wait on a socketpair end whose peer closed returned {}", ev_text(n, ev)))
}

// ---------------------------------------------------------------------------
// Name resolution.

const AI_PASSIVE: i32 = 1;
const AI_CANONNAME: i32 = 2;
const AI_NUMERICHOST: i32 = 4;
const AI_NUMERICSERV: i32 = 0x400;
const EAI_BADFLAGS: i32 = -1;
const EAI_NONAME: i32 = -2;
const EAI_AGAIN: i32 = -3;
const EAI_FAMILY: i32 = -6;
const EAI_SOCKTYPE: i32 = -7;
const EAI_OVERFLOW: i32 = -12;
const NI_NUMERICHOST: i32 = 1;
const NI_NUMERICSERV: i32 = 2;
const NI_NAMEREQD: i32 = 8;

fn eai_name(code: i32) -> String {
    let name = match code {
        0 => "0", -1 => "EAI_BADFLAGS", -2 => "EAI_NONAME", -3 => "EAI_AGAIN", -4 => "EAI_FAIL", -6 => "EAI_FAMILY",
        -7 => "EAI_SOCKTYPE", -8 => "EAI_SERVICE", -10 => "EAI_MEMORY", -11 => "EAI_SYSTEM", -12 => "EAI_OVERFLOW",
        _ => return format!("code {code}"),
    };
    name.to_string()
}

fn hints(family: i32, socktype: i32, flags: i32) -> AddrInfo {
    AddrInfo { flags, family, socktype, protocol: 0, addrlen: 0, addr: null_mut(), canonname: null_mut(), next: null_mut() }
}

/// One entry of a getaddrinfo result.
struct Found { family: i32, socktype: i32, protocol: i32, addrlen: u32, addr: Store, canonname: String }

/// getaddrinfo: the EAI code and the entries, copied out before the list is freed.
fn lookup(node: Option<&str>, service: Option<&str>, h: &AddrInfo) -> Result<(i32, Vec<Found>), CaseError> {
    let (node, service) = (node.map(c), service.map(c));
    let mut res: *mut AddrInfo = null_mut();
    let code = getaddrinfo(node.as_ref().map_or(null(), |n| n.as_ptr()), service.as_ref().map_or(null(), |s| s.as_ptr()), h, &mut res)?;
    let mut out = Vec::new();
    if code == 0 {
        let mut p = res;
        while !p.is_null() && out.len() < 64 {
            // SAFETY: p is an entry of the list getaddrinfo returned, not yet freed.
            let e = unsafe { &*p };
            let mut addr = Store::new();
            let n = (e.addrlen as usize).min(addr.b.len());
            if !e.addr.is_null() {
                // SAFETY: e.addr points to e.addrlen bytes of the entry's address.
                unsafe { core::ptr::copy_nonoverlapping(e.addr, addr.b.as_mut_ptr(), n) };
            }
            addr.len = e.addrlen;
            out.push(Found { family: e.family, socktype: e.socktype, protocol: e.protocol, addrlen: e.addrlen, addr, canonname: c_text(e.canonname) });
            p = e.next;
        }
        if !res.is_null() { freeaddrinfo(res)?; }
    }
    Ok((code, out))
}

/// getaddrinfo that must succeed: its entries.
fn found(node: Option<&str>, service: Option<&str>, h: &AddrInfo) -> Result<Vec<Found>, CaseError> {
    let (code, list) = lookup(node, service, h)?;
    if code != 0 { return err(format!("getaddrinfo({node:?}, {service:?}) returned {}", eai_name(code))); }
    if list.is_empty() { return err(format!("getaddrinfo({node:?}, {service:?}) succeeded with no entries")); }
    Ok(list)
}

/// getaddrinfo that must fail with `code`.
fn lookup_fails(node: Option<&str>, service: Option<&str>, h: &AddrInfo, code: i32) -> CaseResult {
    let (got, _) = lookup(node, service, h)?;
    check(got == code, &format!("getaddrinfo({node:?}, {service:?}) returned {}, expected {}", eai_name(got), eai_name(code)))
}

fn addrs(list: &[Found]) -> String {
    list.iter().map(|f| f.addr.text()).collect::<Vec<_>>().join(", ")
}

/// getnameinfo of `addr`: the code, the host and the service.
fn reverse(addr: &SockIn, hostlen: u32, flags: i32) -> Result<(i32, String, String), CaseError> {
    let mut host = [0u8; 1025];
    let mut serv = [0u8; 32];
    let code = getnameinfo(sa(addr), SIN_LEN, host.as_mut_ptr(), hostlen.min(1025), serv.as_mut_ptr(), 32, flags)?;
    Ok((code, buf_text(&host), buf_text(&serv)))
}

const HOSTS: &str = "/etc/hosts";
/// A copy of /etc/hosts made before a case edits it: `y` and the file, or `n` when there
/// was none.
const HOSTS_SAVED: &str = "/tmp/net-hosts-saved";

/// Put back the /etc/hosts a case saved, if one did.
fn restore_hosts() {
    if let Ok(saved) = std::fs::read(HOSTS_SAVED) {
        let put_back = match saved.split_first() {
            Some((b'y', rest)) => std::fs::write(HOSTS, rest).is_ok(),
            Some((b'n', _)) => std::fs::remove_file(HOSTS).is_ok() || std::fs::metadata(HOSTS).is_err(),
            _ => true,
        };
        if put_back { let _ = std::fs::remove_file(HOSTS_SAVED); }
    }
}

/// /etc/hosts with a line of this case's added, put back when it is dropped.
struct HostsEdit { owner: i32 }

impl Drop for HostsEdit {
    fn drop(&mut self) { if pid() == self.owner { restore_hosts(); } }
}

fn edit_hosts(line: &str) -> Result<HostsEdit, CaseError> {
    restore_hosts();
    let old = std::fs::read(HOSTS).ok();
    let mut saved = vec![if old.is_some() { b'y' } else { b'n' }];
    saved.extend_from_slice(old.as_deref().unwrap_or_default());
    std::fs::write(HOSTS_SAVED, &saved).map_err(|e| format!("saving {HOSTS} failed: {e}"))?;
    let edit = HostsEdit { owner: pid() };
    let mut text = old.unwrap_or_default();
    if !text.is_empty() && !text.ends_with(b"\n") { text.push(b'\n'); }
    text.extend_from_slice(line.as_bytes());
    text.push(b'\n');
    std::fs::write(HOSTS, &text).map_err(|e| format!("writing {HOSTS} failed: {e}"))?;
    Ok(edit)
}

/// The address and names a case adds to /etc/hosts.
const HOSTS_ADDR: u32 = 0x0a4d_0102;
fn hosts_names() -> (String, String) { (format!("breenix-suite-{}", pid()), format!("suite-alias-{}", pid())) }
fn hosts_line() -> String { let (name, alias) = hosts_names(); format!("{} {name} {alias}", ip(HOSTS_ADDR)) }

/// Run a resolve case after putting back an /etc/hosts a killed case left edited.
fn resolved(run: fn() -> CaseResult) -> CaseResult {
    restore_hosts();
    run()
}

fn res_gai_numeric() -> CaseResult {
    let list = found(Some("127.0.0.1"), Some("80"), &hints(AF_INET, SOCK_STREAM, 0))?;
    let f = &list[0];
    check(list.len() == 1 && f.family == AF_INET && f.socktype == SOCK_STREAM && f.protocol == IPPROTO_TCP && f.addrlen == SIN_LEN
        && f.addr.family() == AF_INET && f.addr.addr() == LOOPBACK && f.addr.port() == 80,
        &format!("getaddrinfo(\"127.0.0.1\", \"80\", AF_INET, SOCK_STREAM) gave {} entries, the first family {} type {} protocol {} address {}",
            list.len(), f.family, f.socktype, f.protocol, f.addr.text()))
}

fn res_gai_numerichost() -> CaseResult {
    lookup_fails(Some("localhost"), None, &hints(AF_INET, SOCK_STREAM, AI_NUMERICHOST), EAI_NONAME)
}

fn res_gai_localhost() -> CaseResult {
    let list = found(Some("localhost"), None, &hints(AF_INET, SOCK_STREAM, 0))?;
    check(list.iter().any(|f| f.addr.family() == AF_INET && f.addr.addr() == LOOPBACK),
        &format!("getaddrinfo(\"localhost\", AF_INET) gave {}, without 127.0.0.1", addrs(&list)))
}

fn res_gai_passive() -> CaseResult {
    let list = found(None, Some("8080"), &hints(AF_INET, SOCK_STREAM, AI_PASSIVE))?;
    check(list.iter().all(|f| f.addr.family() == AF_INET && f.addr.addr() == 0 && f.addr.port() == 8080),
        &format!("getaddrinfo(NULL, \"8080\", AI_PASSIVE) gave {}, expected 0.0.0.0:8080", addrs(&list)))
}

fn res_gai_null_node() -> CaseResult {
    let list = found(None, Some("8080"), &hints(AF_INET, SOCK_STREAM, 0))?;
    check(list.iter().all(|f| f.addr.family() == AF_INET && f.addr.addr() == LOOPBACK && f.addr.port() == 8080),
        &format!("getaddrinfo(NULL, \"8080\") without AI_PASSIVE gave {}, expected the loopback address 127.0.0.1:8080", addrs(&list)))
}

fn res_gai_eai_noname() -> CaseResult { lookup_fails(None, None, &hints(AF_INET, SOCK_STREAM, 0), EAI_NONAME) }
fn res_gai_eai_family() -> CaseResult { lookup_fails(Some("127.0.0.1"), None, &hints(12345, SOCK_STREAM, 0), EAI_FAMILY) }
fn res_gai_eai_socktype() -> CaseResult { lookup_fails(Some("127.0.0.1"), None, &hints(AF_INET, 12345, 0), EAI_SOCKTYPE) }
fn res_gai_eai_badflags() -> CaseResult { lookup_fails(Some("127.0.0.1"), None, &hints(AF_INET, SOCK_STREAM, 0x1000_0000), EAI_BADFLAGS) }
fn res_gai_numericserv() -> CaseResult {
    lookup_fails(Some("127.0.0.1"), Some("http"), &hints(AF_INET, SOCK_STREAM, AI_NUMERICSERV), EAI_NONAME)
}

fn res_gai_service_port() -> CaseResult {
    let list = found(Some("127.0.0.1"), Some("8080"), &hints(AF_INET, SOCK_DGRAM, 0))?;
    let f = &list[0];
    check(f.addr.port() == 8080 && f.addr.b[2..4] == [0x1f, 0x90] && f.socktype == SOCK_DGRAM && f.protocol == IPPROTO_UDP,
        &format!("getaddrinfo(\"127.0.0.1\", \"8080\", SOCK_DGRAM) gave {} with type {} protocol {}", f.addr.text(), f.socktype, f.protocol))
}

fn res_gai_socktype_any() -> CaseResult {
    let list = found(Some("127.0.0.1"), Some("80"), &hints(AF_INET, 0, 0))?;
    let has = |t: i32, p: i32| list.iter().any(|f| f.socktype == t && f.protocol == p);
    check(has(SOCK_STREAM, IPPROTO_TCP) && has(SOCK_DGRAM, IPPROTO_UDP),
        &format!("getaddrinfo with ai_socktype 0 gave socket types {:?}, without both SOCK_STREAM/TCP and SOCK_DGRAM/UDP",
            list.iter().map(|f| (f.socktype, f.protocol)).collect::<Vec<_>>()))
}

fn res_gai_canonname() -> CaseResult {
    let list = found(Some("localhost"), None, &hints(AF_INET, SOCK_STREAM, AI_CANONNAME))?;
    check(list[0].canonname == "localhost", &format!("getaddrinfo(\"localhost\", AI_CANONNAME) gave canonical name {:?}", list[0].canonname))
}

/// RFC 6761 reserves .invalid: resolver libraries return a negative answer for it at
/// once, without asking a server.
fn res_gai_invalid_tld() -> CaseResult {
    lookup_fails(Some("no-such-host.invalid"), None, &hints(AF_INET, SOCK_STREAM, 0), EAI_NONAME)
}

fn res_gai_hosts_entry() -> CaseResult {
    let _edit = edit_hosts(&hosts_line())?;
    let (name, _) = hosts_names();
    let list = found(Some(&name), None, &hints(AF_INET, SOCK_STREAM, 0))?;
    check(list.iter().any(|f| f.addr.addr() == HOSTS_ADDR), &format!("getaddrinfo of a name /etc/hosts maps to {} gave {}", ip(HOSTS_ADDR), addrs(&list)))
}

fn res_gai_hosts_alias() -> CaseResult {
    let _edit = edit_hosts(&hosts_line())?;
    let (_, alias) = hosts_names();
    let list = found(Some(&alias), None, &hints(AF_INET, SOCK_STREAM, 0))?;
    check(list.iter().any(|f| f.addr.addr() == HOSTS_ADDR), &format!("getaddrinfo of an /etc/hosts alias for {} gave {}", ip(HOSTS_ADDR), addrs(&list)))
}

fn res_gai_strerror() -> CaseResult {
    let noname = c_text(gai_strerror(EAI_NONAME)?);
    let again = c_text(gai_strerror(EAI_AGAIN)?);
    check(!noname.is_empty() && !again.is_empty() && noname != again,
        &format!("gai_strerror gave {noname:?} for EAI_NONAME and {again:?} for EAI_AGAIN, expected two different messages"))
}

fn res_gni_numeric() -> CaseResult {
    let (code, host, serv) = reverse(&sin(LOOPBACK, 80), 1025, NI_NUMERICHOST | NI_NUMERICSERV)?;
    check(code == 0 && host == "127.0.0.1" && serv == "80",
        &format!("getnameinfo(127.0.0.1:80, NI_NUMERICHOST|NI_NUMERICSERV) returned {} with host {host:?} service {serv:?}", eai_name(code)))
}

fn res_gni_localhost() -> CaseResult {
    let (code, host, _) = reverse(&sin(LOOPBACK, 80), 1025, NI_NUMERICSERV)?;
    check(code == 0 && host == "localhost", &format!("getnameinfo(127.0.0.1) returned {} with host {host:?}, expected \"localhost\"", eai_name(code)))
}

fn res_gni_hosts_entry() -> CaseResult {
    let _edit = edit_hosts(&hosts_line())?;
    let (name, _) = hosts_names();
    let (code, host, _) = reverse(&sin(HOSTS_ADDR, 80), 1025, NI_NAMEREQD | NI_NUMERICSERV)?;
    check(code == 0 && host == name, &format!("getnameinfo({}, NI_NAMEREQD) returned {} with host {host:?}, expected {name:?} from /etc/hosts", ip(HOSTS_ADDR), eai_name(code)))
}

fn res_gni_overflow() -> CaseResult {
    let (code, _, _) = reverse(&sin(LOOPBACK, 80), 4, NI_NUMERICHOST | NI_NUMERICSERV)?;
    check(code == EAI_OVERFLOW, &format!("getnameinfo of 127.0.0.1 into a 4-byte host buffer returned {}, expected EAI_OVERFLOW", eai_name(code)))
}

fn res_gni_eai_family() -> CaseResult {
    let mut a = sin(LOOPBACK, 80);
    a.family = 12345;
    let (code, _, _) = reverse(&a, 1025, NI_NUMERICHOST)?;
    check(code == EAI_FAMILY, &format!("getnameinfo of an address of family 12345 returned {}, expected EAI_FAMILY", eai_name(code)))
}

fn res_inet_pton() -> CaseResult {
    let mut out = [0u8; 4];
    let r = inet_pton(AF_INET, c("192.168.1.2").as_ptr(), out.as_mut_ptr())?;
    check(r == 1 && out == [192, 168, 1, 2], &format!("inet_pton(\"192.168.1.2\") returned {r} with bytes {out:?}"))
}

fn res_inet_pton_invalid() -> CaseResult {
    for text in ["256.1.1.1", "1.2.3", "1.2.3.4x"] {
        let mut out = [0u8; 4];
        let r = inet_pton(AF_INET, c(text).as_ptr(), out.as_mut_ptr())?;
        check(r == 0, &format!("inet_pton({text:?}) returned {r}, expected 0 for a malformed address"))?;
    }
    Ok(())
}

fn res_inet_pton_eafnosupport() -> CaseResult {
    let mut out = [0u8; 16];
    let r = inet_pton(12345, c("1.2.3.4").as_ptr(), out.as_mut_ptr())?;
    let e = if r == -1 { errno() } else { 0 };
    check(r == -1 && e == EAFNOSUPPORT, &format!("inet_pton(12345, ...) returned {r} with errno {}, expected -1 and EAFNOSUPPORT", errname(e)))
}

fn res_inet_ntop() -> CaseResult {
    let src = LOOPBACK.to_be_bytes();
    let mut out = [0u8; 16];
    let p = inet_ntop(AF_INET, src.as_ptr(), out.as_mut_ptr(), 16)?;
    check(!p.is_null() && buf_text(&out) == "127.0.0.1", &format!("inet_ntop of 127.0.0.1 gave {:?}", buf_text(&out)))
}

fn res_inet_ntop_enospc() -> CaseResult {
    let src = LOOPBACK.to_be_bytes();
    let mut out = [0u8; 8];
    let p = inet_ntop(AF_INET, src.as_ptr(), out.as_mut_ptr(), 8)?;
    let e = if p.is_null() { errno() } else { 0 };
    check(p.is_null() && e == ENOSPC, &format!("inet_ntop of 127.0.0.1 into 8 bytes returned {} with errno {}, expected NULL and ENOSPC",
        if p.is_null() { "NULL" } else { "a string" }, errname(e)))
}

fn res_hosts_localhost() -> CaseResult {
    let text = std::fs::read_to_string(HOSTS).map_err(|e| format!("reading {HOSTS} failed: {e}"))?;
    let maps = text.lines().map(|l| l.split('#').next().unwrap_or("")).any(|l| {
        let mut fields = l.split_whitespace();
        fields.next() == Some("127.0.0.1") && fields.any(|f| f == "localhost")
    });
    check(maps, &format!("{HOSTS} has no line mapping 127.0.0.1 to localhost"))
}

/// Query the target's configured nameserver with the production DNS client.
fn res_gateway_dns_query() -> CaseResult {
    let configuration = std::fs::read_to_string("/etc/resolv.conf")
        .map_err(|e| format!("reading /etc/resolv.conf: {e}"))?;
    let server = configuration.lines().find_map(|line| {
        let mut fields = line.split('#').next()?.split_whitespace();
        if fields.next()? != "nameserver" { return None; }
        fields.next()?.parse::<std::net::Ipv4Addr>().ok()
    }).ok_or("/etc/resolv.conf has no IPv4 nameserver")?;
    let answer = libbreenix::dns::resolve_all("example.com", server.octets())
        .map_err(|e| format!("DNS query to {server}:53 failed: {e:?}"))?;
    check(answer.len != 0, "the DNS reply for example.com carries no A record")
}

/// getaddrinfo of example.com, which the C library resolves through the DNS server the
/// system names, on QEMU the user network's 10.0.2.3. It needs that network and an
/// outside one.
fn res_gateway_getaddrinfo() -> CaseResult {
    let list = found(Some("example.com"), None, &hints(AF_INET, SOCK_STREAM, 0))?;
    check(list.iter().any(|f| f.addr.family() == AF_INET && f.addr.addr() >> 24 != 127 && f.addr.addr() != 0),
        &format!("getaddrinfo(\"example.com\") gave {}, with no routable IPv4 address", addrs(&list)))
}

// ---------------------------------------------------------------------------

static SUITE: Suite = suite("networking", "Networking & sockets", &[
    category("udp", "UDP", &[
        case("socket", "socket(AF_INET, SOCK_DGRAM) returns a descriptor fstat reports as S_IFSOCK", udp_socket),
        case("socket-protocol", "socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) succeeds", udp_socket_protocol),
        case("socket-protocol-mismatch", "socket(AF_INET, SOCK_DGRAM, IPPROTO_TCP) fails with EPROTONOSUPPORT", udp_socket_protocol_mismatch),
        case("socket-family", "socket of an unknown address family fails with EAFNOSUPPORT", udp_socket_family),
        case("socket-type", "socket of an unknown socket type fails with EINVAL", udp_socket_type),
        case("bind", "bind to 127.0.0.1 and a free port succeeds", udp_bind),
        case("bind-eaddrinuse", "A second bind to the same address and port fails with EADDRINUSE", udp_bind_eaddrinuse),
        case("bind-twice", "bind of a socket that is already bound fails with EINVAL", udp_bind_twice),
        case("bind-eaddrnotavail", "bind to an address no interface has fails with EADDRNOTAVAIL", udp_bind_eaddrnotavail),
        case("bind-short", "bind with an address length shorter than sockaddr_in fails with EINVAL", udp_bind_short),
        case("bind-family", "bind to an address of another family fails with EAFNOSUPPORT", udp_bind_family),
        case("bind-ephemeral", "bind to port 0 picks a port getsockname reports", udp_bind_ephemeral),
        case("sendto-recvfrom", "A datagram sent over loopback arrives whole and recvfrom reports its sender", udp_sendto_recvfrom),
        case("sendto-autobind", "sendto from an unbound socket binds it to an ephemeral port", udp_sendto_autobind),
        case("boundaries", "Datagrams of 1, 100 and 1000 bytes arrive as three messages of those sizes", udp_boundaries),
        case("truncation", "A datagram read into a short buffer is cut short and its rest discarded", udp_truncation),
        case("msg-trunc", "recv with MSG_TRUNC returns a datagram's full length even into a short buffer", udp_msg_trunc),
        case("recvmsg-trunc", "recvmsg of a datagram into a short buffer sets MSG_TRUNC in msg_flags", udp_recvmsg_trunc),
        case("zero-length", "A zero-length datagram is delivered as a message of 0 bytes", udp_zero_length),
        case("peek", "MSG_PEEK returns a datagram without removing it", udp_peek),
        case("dontwait", "recv with MSG_DONTWAIT on an empty socket fails with EAGAIN", udp_dontwait),
        case("nonblock", "recv on an empty SOCK_NONBLOCK socket fails with EAGAIN", udp_nonblock),
        case("connect-send", "A connected UDP socket sends to its peer with send", udp_connect_send),
        case("connect-filter", "A connected UDP socket receives only from its peer", udp_connect_filter),
        case("connect-getpeername", "getpeername of a connected UDP socket reports its peer", udp_connect_getpeername),
        case("getpeername-enotconn", "getpeername of an unconnected UDP socket fails with ENOTCONN", udp_getpeername_enotconn),
        case("send-edestaddrreq", "send on an unconnected UDP socket fails with EDESTADDRREQ", udp_send_edestaddrreq),
        case("connect-unspec", "connect to AF_UNSPEC dissolves a UDP socket's association", udp_connect_unspec),
        case("connect-refused", "A connected UDP socket that sends to a closed port then fails with ECONNREFUSED", udp_connect_refused),
        case("broadcast-eacces", "sendto the broadcast address without SO_BROADCAST fails with EACCES", udp_broadcast_eacces),
        case("broadcast", "sendto the broadcast address with SO_BROADCAST succeeds", udp_broadcast),
        case("emsgsize", "sendto of a datagram larger than 65507 bytes fails with EMSGSIZE", udp_emsgsize),
        case("large", "A 16384-byte datagram arrives whole over loopback", udp_large),
        case("enotsock", "sendto on a descriptor that is not a socket fails with ENOTSOCK", udp_enotsock),
    ]),
    category("tcp", "TCP", &[
        case("socket", "socket(AF_INET, SOCK_STREAM) returns a descriptor fstat reports as S_IFSOCK", tcp_socket),
        case("listen", "bind and listen on 127.0.0.1 succeed", tcp_listen),
        case("listen-autobind", "listen on an unbound socket binds it to an ephemeral port", tcp_listen_autobind),
        case("connect-accept", "connect over loopback completes and accept returns a new descriptor", tcp_connect_accept),
        case("accept-address", "accept reports the connecting socket's address", tcp_accept_address),
        case("getpeername-client", "getpeername of the connecting socket reports the listener's address", tcp_getpeername_client),
        case("getpeername-server", "getpeername of the accepted socket reports the connecting socket's address", tcp_getpeername_server),
        case("getsockname-accepted", "getsockname of the accepted socket reports the listening address", tcp_getsockname_accepted),
        case("econnrefused", "connect to a port nobody listens on fails with ECONNREFUSED", tcp_econnrefused),
        case("transfer", "256 KiB sent by one process arrive in order in another, then end of file", tcp_transfer),
        case("reverse", "64 KiB sent from the accepted socket arrive in order at the connecting one", tcp_reverse),
        case("eof", "Bytes sent before close are read and then recv returns 0", tcp_eof),
        case("half-close", "shutdown(SHUT_WR) gives the peer end of file", tcp_half_close),
        case("half-close-reverse", "After shutdown(SHUT_WR) the socket still receives from its peer", tcp_half_close_reverse),
        case("rst-unread", "close with unread data resets the connection: the peer gets ECONNRESET", tcp_rst_unread),
        case("epipe-after-close", "Sending to a peer that closed fails with EPIPE once its reset arrives", tcp_epipe_after_close),
        case("sigpipe", "write to a reset connection raises SIGPIPE", tcp_sigpipe),
        case("nonblock-connect", "A nonblocking connect fails with EINPROGRESS", tcp_nonblock_connect),
        case("nonblock-connect-writable", "A nonblocking connect becomes writable with SO_ERROR 0", tcp_nonblock_connect_writable),
        case("eisconn", "connect of a connected socket fails with EISCONN", tcp_eisconn),
        case("accept-eagain", "accept on a nonblocking listener with nothing pending fails with EAGAIN", tcp_accept_eagain),
        case("accept-einval", "accept on a socket that is not listening fails with EINVAL", tcp_accept_einval),
        case("bind-eaddrinuse", "bind to a listening socket's address fails with EADDRINUSE", tcp_bind_eaddrinuse),
        case("backlog", "Four connections made before any accept are accepted in the order they connected", tcp_backlog),
        case("backlog-zero", "A listener with backlog 0 still accepts a connection", tcp_backlog_zero),
        case("accept4-nonblock", "accept4 with SOCK_NONBLOCK returns a socket with O_NONBLOCK set", tcp_accept4_nonblock),
        case("accept4-cloexec", "accept4 with SOCK_CLOEXEC returns a socket with FD_CLOEXEC set", tcp_accept4_cloexec),
        case("accept4-einval", "accept4 with an unknown flag fails with EINVAL", tcp_accept4_einval),
        case("accept-no-inherit", "accept does not pass the listener's O_NONBLOCK to the new socket", tcp_accept_no_inherit),
        case("socket-nonblock", "socket with SOCK_NONBLOCK sets O_NONBLOCK", tcp_socket_nonblock),
        case("socket-cloexec", "socket with SOCK_CLOEXEC sets FD_CLOEXEC", tcp_socket_cloexec),
        case("recv-eagain", "recv with MSG_DONTWAIT and nothing queued fails with EAGAIN", tcp_recv_eagain),
        case("recv-enotconn", "recv on a never-connected TCP socket fails with ENOTCONN", tcp_recv_enotconn),
        case("peek", "MSG_PEEK on a stream returns bytes that a later recv reads again", tcp_peek),
        case("waitall", "recv with MSG_WAITALL waits for the full count sent in two parts", tcp_waitall),
    ]),
    category("unix", "Unix-domain sockets", &[
        case("stream-socket", "socket(AF_UNIX, SOCK_STREAM) succeeds", unix_stream_socket),
        case("dgram-socket", "socket(AF_UNIX, SOCK_DGRAM) succeeds", unix_dgram_socket),
        case("seqpacket-socket", "socket(AF_UNIX, SOCK_SEQPACKET) succeeds", unix_seqpacket_socket),
        case("socketpair", "A stream socketpair carries bytes both ways", unix_socketpair),
        case("socketpair-dgram", "A datagram socketpair keeps message boundaries", unix_socketpair_dgram),
        case("socketpair-eopnotsupp", "socketpair(AF_INET, SOCK_STREAM) fails with EOPNOTSUPP", unix_socketpair_eopnotsupp),
        case("socketpair-eof", "Closing one end of a socketpair gives the other end of file", unix_socketpair_eof),
        case("socketpair-flags", "socketpair with SOCK_NONBLOCK|SOCK_CLOEXEC sets both flags on both ends", unix_socketpair_flags),
        case("bind-path", "bind to a path creates a file stat reports as S_IFSOCK", unix_bind_path),
        case("bind-eaddrinuse", "bind to a path that exists fails with EADDRINUSE", unix_bind_eaddrinuse),
        case("connect-accept", "connect to a listening path is accepted and carries bytes", unix_connect_accept),
        case("connect-enoent", "connect to a path that does not exist fails with ENOENT", unix_connect_enoent),
        case("connect-not-listening", "connect to a bound socket that is not listening fails with ECONNREFUSED", unix_connect_not_listening),
        case("connect-stale", "connect to the path a closed listener left fails with ECONNREFUSED", unix_connect_stale),
        case("connect-not-socket", "connect to a regular file's path fails with ECONNREFUSED", unix_connect_not_socket),
        case("connect-eprototype", "A stream connect to a datagram socket's path fails with EPROTOTYPE", unix_connect_eprototype),
        case("unlink-after-bind", "Unlinking a bound path stops new connections but not accepted ones", unix_unlink_after_bind),
        case("getsockname-path", "getsockname reports the bound path and its length", unix_getsockname_path),
        case("getpeername-path", "getpeername of a connected socket reports the server's path", unix_getpeername_path),
        case("getsockname-unbound", "getsockname of an unbound socket reports AF_UNIX with length 2", unix_getsockname_unbound),
        case("abstract", "An abstract address binds, listens and accepts a connection", unix_abstract),
        case("abstract-eaddrinuse", "A second bind to an abstract name fails with EADDRINUSE", unix_abstract_eaddrinuse),
        case("abstract-getsockname", "getsockname reports an abstract name with its exact length", unix_abstract_getsockname),
        case("abstract-released", "An abstract name is free again once its socket closes", unix_abstract_released),
        case("dgram-path", "A datagram sent to a bound path arrives with the sender's path", unix_dgram_path),
        case("path-too-long", "bind with an address longer than sockaddr_un fails with EINVAL", unix_path_too_long),
        case("scm-rights", "A descriptor passed with SCM_RIGHTS works in the receiver", unix_scm_rights),
        case("scm-rights-same-file", "A descriptor passed with SCM_RIGHTS is a new number for the same open file", unix_scm_rights_same_file),
        case("peercred-socketpair", "SO_PEERCRED on a socketpair reports the process's pid, uid and gid", unix_peercred_socketpair),
        case("peercred-connect", "SO_PEERCRED on an accepted socket reports the connecting process", unix_peercred_connect),
        case("epipe", "send to a socketpair end whose peer closed fails with EPIPE", unix_epipe),
    ]),
    category("options", "Socket options and shutdown", &[
        case("so-type-stream", "SO_TYPE of a TCP socket is SOCK_STREAM", opt_so_type_stream),
        case("so-type-dgram", "SO_TYPE of a UDP socket is SOCK_DGRAM", opt_so_type_dgram),
        case("so-protocol", "SO_PROTOCOL of a TCP socket is IPPROTO_TCP", opt_so_protocol),
        case("so-domain", "SO_DOMAIN of a Unix socket is AF_UNIX", opt_so_domain),
        case("so-acceptconn", "SO_ACCEPTCONN is 0 before listen and 1 after", opt_so_acceptconn),
        case("so-reuseaddr", "SO_REUSEADDR starts 0 and reads back set", opt_so_reuseaddr),
        case("reuseaddr-time-wait", "With SO_REUSEADDR a port whose connection is in TIME_WAIT binds again", opt_reuseaddr_time_wait),
        case("time-wait-eaddrinuse", "Without SO_REUSEADDR a port whose connection is in TIME_WAIT fails with EADDRINUSE", opt_time_wait_eaddrinuse),
        case("reuseaddr-udp", "UDP sockets that all set SO_REUSEADDR share an address and port", opt_reuseaddr_udp),
        case("so-rcvbuf", "SO_RCVBUF set to 32768 reads back doubled, 65536", opt_so_rcvbuf),
        case("so-sndbuf", "SO_SNDBUF set to 32768 reads back doubled, 65536", opt_so_sndbuf),
        case("so-rcvtimeo", "SO_RCVTIMEO reads back the timeval it was set to", opt_so_rcvtimeo),
        case("so-rcvtimeo-eagain", "recv with SO_RCVTIMEO and no data fails with EAGAIN after the timeout", opt_so_rcvtimeo_eagain),
        case("so-sndtimeo-eagain", "send on a full socket with SO_SNDTIMEO fails with EAGAIN after the timeout", opt_so_sndtimeo_eagain),
        case("so-rcvtimeo-edom", "SO_RCVTIMEO with tv_usec of a million fails with EDOM", opt_so_rcvtimeo_edom),
        case("so-linger", "SO_LINGER reads back the l_onoff and l_linger it was set to", opt_so_linger),
        case("so-linger-reset", "close with SO_LINGER of 0 seconds resets the connection", opt_so_linger_reset),
        case("so-error", "SO_ERROR of a new socket is 0", opt_so_error),
        case("so-error-refused", "A refused nonblocking connect leaves ECONNREFUSED in SO_ERROR, and reading it clears it", opt_so_error_refused),
        case("tcp-nodelay", "TCP_NODELAY starts 0 and reads back set", opt_tcp_nodelay),
        case("tcp-nodelay-udp", "TCP_NODELAY on a UDP socket fails with ENOPROTOOPT", opt_tcp_nodelay_udp),
        case("so-keepalive", "SO_KEEPALIVE reads back set", opt_so_keepalive),
        case("so-broadcast", "SO_BROADCAST starts 0 and reads back set", opt_so_broadcast),
        case("enoprotoopt-get", "getsockopt of an unknown option fails with ENOPROTOOPT", opt_enoprotoopt_get),
        case("enoprotoopt-set", "setsockopt of an unknown option fails with ENOPROTOOPT", opt_enoprotoopt_set),
        case("enotsock", "getsockopt on a descriptor that is not a socket fails with ENOTSOCK", opt_enotsock),
        case("optlen-einval", "setsockopt of an int option with a 2-byte value fails with EINVAL", opt_optlen_einval),
        case("shut-rd", "After shutdown(SHUT_RD) recv returns 0 at once", opt_shut_rd),
        case("shut-wr", "After shutdown(SHUT_WR) send fails with EPIPE", opt_shut_wr),
        case("shut-rdwr", "After shutdown(SHUT_RDWR) send fails with EPIPE and recv returns 0", opt_shut_rdwr),
        case("shutdown-enotconn", "shutdown of an unconnected TCP socket fails with ENOTCONN", opt_shutdown_enotconn),
        case("shutdown-einval", "shutdown with an unknown how fails with EINVAL", opt_shutdown_einval),
    ]),
    category("multiplex", "poll and select on sockets", &[
        case("poll-udp-idle", "poll reports nothing on an empty UDP socket", mux_poll_udp_idle),
        case("poll-udp-readable", "poll reports POLLIN once a datagram arrives", mux_poll_udp_readable),
        case("poll-udp-writable", "poll reports POLLOUT on an idle UDP socket", mux_poll_udp_writable),
        case("poll-tcp-idle", "poll reports nothing on a connection with no data", mux_poll_tcp_idle),
        case("poll-tcp-readable", "poll reports POLLIN once the peer writes", mux_poll_tcp_readable),
        case("poll-tcp-writable", "poll reports POLLOUT on a new connection", mux_poll_tcp_writable),
        case("poll-accept-ready", "poll reports POLLIN on a listener with a pending connection", mux_poll_accept_ready),
        case("poll-accept-idle", "poll on a listener with nothing pending times out", mux_poll_accept_idle),
        case("poll-eof", "poll reports POLLIN at end of file and recv returns 0", mux_poll_eof),
        case("poll-rdhup", "poll reports POLLRDHUP after the peer's SHUT_WR", mux_poll_rdhup),
        case("poll-hup-unix", "poll reports POLLHUP on a socketpair end whose peer closed", mux_poll_hup_unix),
        case("poll-hup-tcp", "poll reports POLLHUP on a connection shut down both ways", mux_poll_hup_tcp),
        case("poll-err-refused", "poll reports POLLERR on a socket whose connect was refused", mux_poll_err_refused),
        case("poll-unix-dgram", "poll reports POLLIN on a Unix datagram socket with a datagram queued", mux_poll_unix_dgram),
        case("poll-wakes-data", "A poll blocked on a socket wakes when another process writes", mux_poll_wakes_data),
        case("poll-wakes-accept", "A poll blocked on a listener wakes when another process connects", mux_poll_wakes_accept),
        case("poll-many", "poll on 8 sockets reports exactly the 3 with data", mux_poll_many),
        case("select-idle", "select reports nothing readable on a connection with no data", mux_select_idle),
        case("select-readable", "select reports a connection readable once the peer writes", mux_select_readable),
        case("select-writable", "select reports a new connection writable", mux_select_writable),
        case("select-accept-ready", "select reports a listener with a pending connection readable", mux_select_accept_ready),
        case("select-eof", "select reports a connection readable at end of file", mux_select_eof),
        case("select-timeout", "select on an idle socket times out", mux_select_timeout),
        case("select-connect-writable", "select reports a nonblocking connect writable once it completes", mux_select_connect_writable),
        case("select-connect-refused", "select reports a refused nonblocking connect writable with ECONNREFUSED pending", mux_select_connect_refused),
        case("epoll-readable", "epoll_wait reports EPOLLIN and the registered data once the peer writes", mux_epoll_readable),
        case("epoll-accept", "epoll_wait reports EPOLLIN on a listener with a pending connection", mux_epoll_accept),
        case("epoll-timeout", "epoll_wait on an idle socket times out", mux_epoll_timeout),
        case("epoll-edge", "EPOLLET reports a datagram's arrival once", mux_epoll_edge),
        case("epoll-oneshot", "EPOLLONESHOT reports once until re-armed with EPOLL_CTL_MOD", mux_epoll_oneshot),
        case("epoll-hup", "epoll_wait reports EPOLLHUP on a socketpair end whose peer closed", mux_epoll_hup),
    ]),
    category("resolve", "Name resolution", &[
        case("gai-numeric", "getaddrinfo of 127.0.0.1 and port 80 gives one AF_INET TCP address", || resolved(res_gai_numeric)),
        case("gai-numerichost", "getaddrinfo of a name with AI_NUMERICHOST fails with EAI_NONAME", || resolved(res_gai_numerichost)),
        case("gai-localhost", "getaddrinfo of localhost gives 127.0.0.1", || resolved(res_gai_localhost)),
        case("gai-passive", "getaddrinfo of no host with AI_PASSIVE gives the wildcard address", || resolved(res_gai_passive)),
        case("gai-null-node", "getaddrinfo of no host without AI_PASSIVE gives the loopback address", || resolved(res_gai_null_node)),
        case("gai-eai-noname", "getaddrinfo of no host and no service fails with EAI_NONAME", || resolved(res_gai_eai_noname)),
        case("gai-eai-family", "getaddrinfo with an unknown family fails with EAI_FAMILY", || resolved(res_gai_eai_family)),
        case("gai-eai-socktype", "getaddrinfo with an unknown socket type fails with EAI_SOCKTYPE", || resolved(res_gai_eai_socktype)),
        case("gai-eai-badflags", "getaddrinfo with an unknown flag fails with EAI_BADFLAGS", || resolved(res_gai_eai_badflags)),
        case("gai-numericserv", "getaddrinfo of a service name with AI_NUMERICSERV fails with EAI_NONAME", || resolved(res_gai_numericserv)),
        case("gai-service-port", "getaddrinfo of a numeric service sets the port in network order", || resolved(res_gai_service_port)),
        case("gai-socktype-any", "getaddrinfo with no socket type gives both TCP and UDP entries", || resolved(res_gai_socktype_any)),
        case("gai-canonname", "getaddrinfo with AI_CANONNAME reports localhost's canonical name", || resolved(res_gai_canonname)),
        case("gai-invalid-tld", "getaddrinfo of a name under .invalid fails with EAI_NONAME", || resolved(res_gai_invalid_tld)),
        case("gai-hosts-entry", "getaddrinfo of a name added to /etc/hosts gives its address", || resolved(res_gai_hosts_entry)),
        case("gai-hosts-alias", "getaddrinfo of an /etc/hosts alias gives its line's address", || resolved(res_gai_hosts_alias)),
        case("gai-strerror", "gai_strerror gives a different message for each error", || resolved(res_gai_strerror)),
        case("gni-numeric", "getnameinfo with NI_NUMERICHOST|NI_NUMERICSERV gives 127.0.0.1 and 80", || resolved(res_gni_numeric)),
        case("gni-localhost", "getnameinfo of 127.0.0.1 gives localhost", || resolved(res_gni_localhost)),
        case("gni-hosts-entry", "getnameinfo with NI_NAMEREQD gives the name /etc/hosts lists for an address", || resolved(res_gni_hosts_entry)),
        case("gni-overflow", "getnameinfo into a host buffer too small fails with EAI_OVERFLOW", || resolved(res_gni_overflow)),
        case("gni-eai-family", "getnameinfo of an address of an unknown family fails with EAI_FAMILY", || resolved(res_gni_eai_family)),
        case("inet-pton", "inet_pton converts a dotted-quad address to network order", || resolved(res_inet_pton)),
        case("inet-pton-invalid", "inet_pton returns 0 for malformed addresses", || resolved(res_inet_pton_invalid)),
        case("inet-pton-eafnosupport", "inet_pton with an unknown family fails with EAFNOSUPPORT", || resolved(res_inet_pton_eafnosupport)),
        case("inet-ntop", "inet_ntop formats 127.0.0.1", || resolved(res_inet_ntop)),
        case("inet-ntop-enospc", "inet_ntop into a buffer too small fails with ENOSPC", || resolved(res_inet_ntop_enospc)),
        case("hosts-localhost", "/etc/hosts maps localhost to 127.0.0.1", || resolved(res_hosts_localhost)),
        case("gateway-dns-query", "A DNS query to the configured nameserver is answered with an A record", || resolved(res_gateway_dns_query)),
        case("gateway-getaddrinfo", "getaddrinfo of example.com resolves through the configured nameserver", || resolved(res_gateway_getaddrinfo)),
    ]),
]);

fn main() { SUITE.run() }
