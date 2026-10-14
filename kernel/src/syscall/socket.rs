//! Socket system calls.
//!
//! Each call resolves its descriptor to the socket under PROCESS_MANAGER and
//! releases PM before calling the protocol (`socket::object::ProtoOps`). This
//! layer owns everything that touches the caller: user memory goes through
//! `socket::addr` and `userptr`, SIGPIPE is raised here on EPIPE unless
//! MSG_NOSIGNAL is set (write always raises it), and a pending SO_ERROR is
//! reported before a fresh send or receive.

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::errno::{
    EAFNOSUPPORT, EBADF, EFAULT, EINVAL, EMSGSIZE, ENOBUFS, ENOMEM, EOPNOTSUPP, EPIPE,
    EPROTONOSUPPORT, ESOCKTNOSUPPORT, ESRCH,
};
use super::{userptr, SyscallResult};
use crate::ipc::fd::{flags, status_flags, FdKind, FdTableInner, FileDescriptor};
use crate::task::process_task::close_extracted_fd;
use crate::socket::addr::{read_sockaddr, write_sockaddr, SockAddr};
use crate::socket::object::{
    caller_ucred, socket_from_fd, OptBuf, RecvMsg, SendMsg, Socket, SocketProto, UCred,
};
use crate::socket::{opts, tcp::TcpSock, udp::UdpSock, unix::UnixSock};

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
/// Linux's AF_MAX (NPROTO): a family at or past it is EAFNOSUPPORT.
const AF_MAX: i32 = 46;
const SOCK_STREAM: u16 = 1;
const SOCK_DGRAM: u16 = 2;
const SOCK_RAW: u16 = 3;
/// Linux's SOCK_MAX: a type at or past it is EINVAL.
const SOCK_MAX: u16 = 11;
const SOCK_TYPE_MASK: u32 = 0xf;
const SOCK_NONBLOCK: u32 = 0o4000;
const SOCK_CLOEXEC: u32 = 0o2000000;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;
/// Linux's IPPROTO_MAX: an AF_INET protocol outside 0..IPPROTO_MAX is EINVAL.
const IPPROTO_MAX: i32 = 263;
/// PF_UNIX, which Linux accepts as AF_UNIX's protocol.
const PROTO_UNIX: i32 = 1;
/// Linux's sizeof(struct sockaddr_storage): sendmsg clamps msg_namelen to it.
const SOCKADDR_STORAGE: u32 = 128;

const MSG_PEEK: u32 = 0x2;
const MSG_CTRUNC: u32 = 0x8;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_NOSIGNAL: u32 = 0x4000;
const MSG_CMSG_CLOEXEC: u32 = 0x4000_0000;

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
/// Linux's SCM_MAX_FD: descriptors one SCM_RIGHTS message may carry.
const SCM_MAX_FD: usize = 253;
/// Linux's net.core.optmem_max: the largest control buffer sendmsg reads.
const CONTROL_MAX: usize = 20_480;
/// Linux's UIO_MAXIOV.
const IOV_MAX: u64 = 1024;
/// Linux's net.core.somaxconn: the cap on a listen backlog.
const SOMAXCONN: i32 = 4096;
/// The most bytes one protocol send or receive moves through the kernel. A
/// stream send moves a longer buffer in pieces of this size and a stream
/// receive returns a short count; a datagram or record longer than this is
/// EMSGSIZE (longer than UDP's limit, and than an AF_UNIX send buffer).
const MAX_IO: usize = 256 * 1024;

const CMSG_HDR: usize = 16;

fn cmsg_align(len: usize) -> usize {
    (len + 7) & !7
}

fn to_result(result: Result<u64, i32>) -> SyscallResult {
    match result {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

fn user(result: Result<(), u64>) -> Result<(), i32> {
    result.map_err(|errno| errno as i32)
}

/// Reset the timer quantum after a blocking wait (architecture-specific).
#[cfg(feature = "boot_tests")]
#[inline(always)]
fn reset_quantum() {
    #[cfg(target_arch = "x86_64")]
    crate::interrupts::timer::reset_quantum();
    #[cfg(target_arch = "aarch64")]
    crate::arch_impl::aarch64::timer_interrupt::reset_quantum();
}

/// Test hook to verify reset_quantum wiring on ARM64.
#[cfg(feature = "boot_tests")]
pub fn test_reset_quantum_hook() {
    reset_quantum();
}

/// Run `f` on the caller's descriptor table, under PROCESS_MANAGER.
fn with_table<R>(f: impl FnOnce(&mut FdTableInner) -> R) -> Result<R, i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let mut guard = crate::process::manager();
    let manager = guard.as_mut().ok_or(ESRCH)?;
    let (_, process) = manager.find_process_by_thread_mut(tid).ok_or(ESRCH)?;
    Ok(f(&mut process.fd_table))
}

/// Hold up to `count` free descriptor slots, stopping at the first that
/// cannot be held. A call holds its slots before anything it cannot undo,
/// and installs into them once nothing else can fail. The second value is
/// why the slots stopped short (EMFILE, ENOMEM), 0 when all were held.
fn hold_prefix(count: usize) -> Result<(Vec<i32>, i32), i32> {
    let mut slots = Vec::new();
    slots.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    let stopped = with_table(|table| {
        while slots.len() < count {
            match table.try_reserve_slot() {
                Ok(fd) => slots.push(fd),
                Err(errno) => return errno,
            }
        }
        0
    })?;
    Ok((slots, stopped))
}

/// Hold `count` slots, all or none.
fn hold(count: usize) -> Result<Vec<i32>, i32> {
    let (slots, stopped) = hold_prefix(count)?;
    if stopped != 0 {
        unhold(&slots);
        return Err(stopped);
    }
    Ok(slots)
}

/// Give back held slots a call no longer needs.
fn unhold(slots: &[i32]) {
    let _ = with_table(|table| {
        for fd in slots {
            table.release_reserved(*fd);
        }
    });
}

/// Install `entries` in held `slots`, pairwise. A held slot is this call's
/// alone, so installing into it cannot fail; an entry left over (the
/// process is gone) is closed.
fn install(slots: &[i32], entries: impl IntoIterator<Item = FileDescriptor>) {
    let mut entries = entries.into_iter();
    let _ = with_table(|table| {
        for fd in slots {
            let Some(entry) = entries.next() else {
                break;
            };
            if let Err(entry) = table.install_held(*fd, entry) {
                // Not reachable while the slot is held; keep the entry.
                let _ = table.install_reserved(*fd, entry);
            }
        }
    });
    discard_rights(entries);
}

fn socket_entry(socket: Arc<Socket>, cloexec: bool, nonblock: bool) -> Result<FileDescriptor, i32> {
    FileDescriptor::try_with_flags(
        FdKind::Socket(socket),
        if cloexec { flags::FD_CLOEXEC } else { 0 },
        if nonblock {
            status_flags::O_NONBLOCK
        } else {
            0
        },
    )
}

/// Close descriptors that never reached a table (SCM_RIGHTS that were not
/// delivered) the way a close of an extracted descriptor closes them, so
/// pipe, FIFO and PTY reference counts drop with them. Allocates nothing.
fn discard_rights(rights: impl IntoIterator<Item = FileDescriptor>) {
    for right in rights {
        close_extracted_fd(right);
    }
}

/// The family, type and protocol a new socket gets, with Linux's checks in
/// Linux's order: the family's range (EAFNOSUPPORT), the type's (EINVAL),
/// then the family's own checks. AF_UNIX takes SOCK_RAW as SOCK_DGRAM.
fn check_kind(family: i32, kind: u16, protocol: i32) -> Result<(u16, u16, u16), i32> {
    if !(0..AF_MAX).contains(&family) {
        return Err(EAFNOSUPPORT);
    }
    if kind >= SOCK_MAX {
        return Err(EINVAL);
    }
    match family as u16 {
        AF_INET => {
            if !(0..IPPROTO_MAX).contains(&protocol) {
                return Err(EINVAL);
            }
            let (want, number) = match kind {
                SOCK_STREAM => (IPPROTO_TCP, 6),
                SOCK_DGRAM => (IPPROTO_UDP, 17),
                _ => return Err(ESOCKTNOSUPPORT),
            };
            if protocol != 0 && protocol != want {
                return Err(EPROTONOSUPPORT);
            }
            Ok((AF_INET, kind, number))
        }
        AF_UNIX => {
            if protocol != 0 && protocol != PROTO_UNIX {
                return Err(EPROTONOSUPPORT);
            }
            let kind = if kind == SOCK_RAW { SOCK_DGRAM } else { kind };
            if !UnixSock::supports_kind(kind) {
                return Err(ESOCKTNOSUPPORT);
            }
            Ok((AF_UNIX, kind, 0))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

/// The socket a new descriptor gets for `family`, `kind` and `protocol`.
fn create(family: i32, kind: u16, protocol: i32) -> Result<Arc<Socket>, i32> {
    let (family, kind, protocol) = check_kind(family, kind, protocol)?;
    match (family, kind) {
        (AF_INET, SOCK_STREAM) => {
            Socket::new(AF_INET, SOCK_STREAM, protocol, SocketProto::Tcp(TcpSock::new()))
        }
        (AF_INET, _) => Socket::new(AF_INET, SOCK_DGRAM, protocol, SocketProto::Udp(UdpSock::new()?)),
        _ => Socket::new(AF_UNIX, kind, 0, SocketProto::Unix(UnixSock::new(kind))),
    }
}

/// Split socket()'s type argument, an int, into the type and the
/// SOCK_NONBLOCK and SOCK_CLOEXEC flags; any other flag is EINVAL.
fn split_type(sock_type: u64) -> Result<(u16, bool, bool), i32> {
    let sock_type = sock_type as u32;
    let extra = sock_type & !SOCK_TYPE_MASK;
    if extra & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    Ok((
        (sock_type & SOCK_TYPE_MASK) as u16,
        extra & SOCK_NONBLOCK != 0,
        extra & SOCK_CLOEXEC != 0,
    ))
}

/// An int argument: the low 32 bits of the register, as Linux reads it.
fn int_arg(value: u64) -> i32 {
    value as u32 as i32
}

/// socket(domain, type, protocol)
pub fn sys_socket(domain: u64, sock_type: u64, protocol: u64) -> SyscallResult {
    to_result((|| {
        let (kind, nonblock, cloexec) = split_type(sock_type)?;
        let socket = create(int_arg(domain), kind, int_arg(protocol))?;
        let entry = socket_entry(socket, cloexec, nonblock)?;
        let slots = hold(1)?;
        install(&slots, [entry]);
        Ok(slots[0] as u64)
    })())
}

/// socketpair(domain, type, protocol, sv). Only AF_UNIX makes pairs; Linux
/// gives AF_INET EOPNOTSUPP once its type and protocol check out. The
/// descriptors are installed only once sv holds their numbers.
pub fn sys_socketpair(domain: u64, sock_type: u64, protocol: u64, sv_ptr: u64) -> SyscallResult {
    to_result((|| {
        let (kind, nonblock, cloexec) = split_type(sock_type)?;
        let (family, kind, _) = check_kind(int_arg(domain), kind, int_arg(protocol))?;
        if family != AF_UNIX {
            return Err(EOPNOTSUPP);
        }
        if sv_ptr == 0 {
            return Err(EFAULT);
        }
        let (a, b) = UnixSock::new_pair(kind, caller_ucred())?;
        let first = socket_entry(a, cloexec, nonblock)?;
        let second = socket_entry(b, cloexec, nonblock)?;
        let slots = hold(2)?;
        let sv = [slots[0], slots[1]];
        // sv may be a copy-on-write page: write it with PM released.
        if let Err(errno) = userptr::copy_to_user(sv_ptr as *mut [i32; 2], &sv) {
            unhold(&slots);
            return Err(errno as i32);
        }
        install(&slots, [first, second]);
        Ok(0)
    })())
}

/// Close descriptors this call installed before a later step failed, as
/// close(2) closes them: record locks, IPC close hooks, and pipe, FIFO and
/// PTY counts. Allocates nothing.
fn close_own(fds: &[i32]) {
    for fd in fds {
        let closed = with_table(|table| table.close(*fd).ok());
        let Ok(Some(entry)) = closed else {
            continue;
        };
        let owner = {
            let tid = crate::task::scheduler::current_thread_id();
            let guard = crate::process::manager();
            guard
                .as_ref()
                .zip(tid)
                .and_then(|(manager, tid)| manager.find_process_by_thread(tid))
                .map(|(_, process)| {
                    (
                        crate::ipc::object::close_owner(process.id, process.thread_group_id),
                        process.lock_owner.id(),
                    )
                })
        };
        if let Some((close_pid, lock_owner)) = owner {
            crate::fs::locks::release_closed(lock_owner, &entry.kind);
            if let FdKind::Ipc(object) = &entry.kind {
                object.on_close(close_pid);
            }
        }
        close_extracted_fd(entry);
    }
}

/// The length argument of bind, connect and sendto: Linux reads it as an
/// int, and a negative one is EINVAL.
fn addr_len(len: u64) -> Result<u32, i32> {
    let len = int_arg(len);
    if len < 0 {
        return Err(EINVAL);
    }
    Ok(len as u32)
}

/// bind(fd, addr, addrlen)
pub fn sys_bind(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        let addr = read_sockaddr(addr_ptr, addr_len(addrlen)?, sk.family)?;
        sk.ops().bind(&sk, &addr)?;
        Ok(0)
    })())
}

/// listen(fd, backlog). Linux caps the backlog at somaxconn, and reads a
/// negative one as unsigned, so it is capped too.
pub fn sys_listen(fd: u64, backlog: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        let backlog = backlog as i32;
        let backlog = if !(0..=SOMAXCONN).contains(&backlog) {
            SOMAXCONN
        } else {
            backlog
        };
        sk.ops().listen(&sk, backlog)?;
        sk.common.notify();
        Ok(0)
    })())
}

/// accept(fd, addr, addrlen)
pub fn sys_accept(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    sys_accept4(fd, addr_ptr, addrlen_ptr, 0)
}

/// accept4(fd, addr, addrlen, flags). SOCK_NONBLOCK and SOCK_CLOEXEC apply to
/// the new descriptor, which inherits neither from the listener (Linux).
pub fn sys_accept4(fd: u64, addr_ptr: u64, addrlen_ptr: u64, accept_flags: u64) -> SyscallResult {
    to_result((|| {
        if (accept_flags as u32) & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
            return Err(EINVAL);
        }
        let accept_flags = accept_flags as u32;
        let (sk, status) = socket_from_fd(fd as i32)?;
        let nonblock = status & status_flags::O_NONBLOCK != 0;
        // The slot is held before a connection is taken off the queue, so
        // EMFILE leaves the connection queued, as on Linux.
        let slots = hold(1)?;
        let accepted = sk.ops().accept(&sk, nonblock).and_then(|(socket, peer)| {
            let entry = socket_entry(
                socket,
                accept_flags & SOCK_CLOEXEC != 0,
                accept_flags & SOCK_NONBLOCK != 0,
            )?;
            if addr_ptr != 0 {
                // A peer address the caller cannot take fails the call, and
                // the new socket goes with it.
                write_sockaddr(&peer, addr_ptr, addrlen_ptr)?;
            }
            Ok(entry)
        });
        match accepted {
            Ok(entry) => {
                install(&slots, [entry]);
                Ok(slots[0] as u64)
            }
            Err(errno) => {
                unhold(&slots);
                Err(errno)
            }
        }
    })())
}

/// connect(fd, addr, addrlen)
pub fn sys_connect(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let addr = read_sockaddr(addr_ptr, addr_len(addrlen)?, sk.family)?;
        let nonblock = status & status_flags::O_NONBLOCK != 0;
        sk.ops().connect(&sk, &addr, nonblock)?;
        Ok(0)
    })())
}

/// shutdown(fd, how)
pub fn sys_shutdown(fd: u64, how: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        if how > 2 {
            return Err(EINVAL);
        }
        sk.ops().shutdown(&sk, how as u8)?;
        sk.common.set_shut(how as u8);
        sk.common.notify();
        Ok(0)
    })())
}

/// getsockname(fd, addr, addrlen)
pub fn sys_getsockname(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        write_sockaddr(&sk.ops().try_local_name(&sk)?, addr_ptr, addrlen_ptr)?;
        Ok(0)
    })())
}

/// getpeername(fd, addr, addrlen)
pub fn sys_getpeername(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        let peer = sk.ops().peer_name(&sk)?;
        write_sockaddr(&peer, addr_ptr, addrlen_ptr)?;
        Ok(0)
    })())
}

/// setsockopt(fd, level, name, optval, optlen)
pub fn sys_setsockopt(fd: u64, level: u64, name: u64, optval: u64, optlen: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        let len = optlen as u32 as i32;
        if len < 0 {
            return Err(EINVAL);
        }
        // Only the option's own bytes are read, so a value that ends at the
        // end of a page is never read past.
        let mut value = [0u8; 16];
        let n = opts::value_len(level as i32, name as i32, len as usize).min(value.len());
        user(userptr::read_user_bytes(value.as_mut_ptr(), optval, n))?;
        opts::setsockopt(&sk, level as i32, name as i32, &value[..n])?;
        Ok(0)
    })())
}

/// getsockopt(fd, level, name, optval, optlen). As on Linux, a value longer
/// than *optlen is truncated and *optlen is set to the bytes written.
pub fn sys_getsockopt(fd: u64, level: u64, name: u64, optval: u64, optlen: u64) -> SyscallResult {
    to_result((|| {
        let (sk, _) = socket_from_fd(fd as i32)?;
        let room: u32 = userptr::copy_from_user(optlen as *const u32).map_err(|e| e as i32)?;
        if (room as i32) < 0 {
            return Err(EINVAL);
        }
        let mut out = OptBuf::new();
        opts::getsockopt(&sk, level as i32, name as i32, &mut out)?;
        let n = out.len.min(room as usize);
        user(userptr::write_user_bytes(optval, out.bytes.as_ptr(), n))?;
        user(userptr::copy_to_user(optlen as *mut u32, &(n as u32)))?;
        Ok(0)
    })())
}

/// Copy `len` bytes of user memory at `ptr` into a new kernel buffer.
fn copy_in(ptr: u64, len: usize) -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    data.resize(len, 0);
    user(userptr::read_user_bytes(data.as_mut_ptr(), ptr, len))?;
    Ok(data)
}

/// One protocol send: a pending error first; the descriptors the protocol
/// did not take are closed.
fn send_once(sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
    let pending = sk.common.take_error();
    let result = if pending != 0 {
        Err(pending)
    } else {
        sk.ops().sendmsg(sk, msg)
    };
    discard_rights(core::mem::take(&mut msg.rights));
    result
}

/// SIGPIPE for a send that failed with EPIPE, unless MSG_NOSIGNAL.
fn sigpipe(result: Result<usize, i32>, flags: u32) -> Result<usize, i32> {
    if result == Err(EPIPE) && flags & MSG_NOSIGNAL == 0 {
        super::signal::raise_sigpipe();
    }
    result
}

/// A send of `len` bytes that `fill(offset, piece)` copies in. A stream
/// moves them in pieces of at most MAX_IO and returns the bytes sent once a
/// piece goes short or fails after earlier pieces went (Linux returns the
/// count, and raises no SIGPIPE, once anything was sent). A datagram or
/// record goes whole, and one longer than MAX_IO is EMSGSIZE. `rights` and
/// `dest` go with the first piece.
fn send_all(
    sk: &Arc<Socket>,
    len: usize,
    flags: u32,
    nonblock: bool,
    mut dest: Option<SockAddr>,
    mut rights: Vec<FileDescriptor>,
    mut fill: impl FnMut(usize, &mut [u8]) -> Result<(), i32>,
) -> Result<usize, i32> {
    if sk.kind != SOCK_STREAM && len > MAX_IO {
        discard_rights(rights);
        return Err(EMSGSIZE);
    }
    let mut piece = Vec::new();
    if piece.try_reserve_exact(len.min(MAX_IO)).is_err() {
        discard_rights(rights);
        return Err(ENOMEM);
    }
    let cred = send_cred(sk);
    let mut sent = 0;
    loop {
        let take = (len - sent).min(MAX_IO);
        piece.clear();
        piece.resize(take, 0);
        let result = fill(sent, &mut piece).and_then(|()| {
            let mut msg = SendMsg {
                data: &piece,
                dest: dest.take(),
                flags,
                nonblock,
                rights: core::mem::take(&mut rights),
                cred,
            };
            send_once(sk, &mut msg)
        });
        discard_rights(core::mem::take(&mut rights));
        match result {
            Ok(n) => {
                sent += n;
                if n < take || sent == len {
                    return Ok(sent);
                }
            }
            Err(_) if sent > 0 => return Ok(sent),
            Err(errno) => return sigpipe(Err(errno), flags),
        }
    }
}

/// A receive into `msg` on `sk` that hands the message to `deliver`, the
/// copy to the caller. Stream bytes an earlier receive could not copy come
/// first, from the stash; bytes this one cannot copy go back there.
fn receive(
    sk: &Arc<Socket>,
    msg: &mut RecvMsg,
    deliver: &mut dyn FnMut(&mut RecvMsg) -> Result<(), i32>,
) -> Result<usize, i32> {
    if sk.kind == SOCK_STREAM {
        let peek = msg.flags & MSG_PEEK != 0;
        if let Some(part) = sk.common.take_stash(msg.cap, peek) {
            let (data, rights) = part?;
            msg.data = data;
            msg.rights = rights;
            let received = msg.data.len();
            if let Err(errno) = deliver(msg) {
                if !peek {
                    let data = core::mem::take(&mut msg.data);
                    let rights = core::mem::take(&mut msg.rights);
                    if let Err((_, rights)) = sk.common.put_back(data, rights) {
                        // Without memory to rejoin them, the bytes are lost
                        // and the descriptors closed.
                        discard_rights(rights);
                    }
                }
                return Err(errno);
            }
            return Ok(received);
        }
    }
    // A datagram socket reports a pending error ahead of its queue, as Linux's
    // datagram receive does. A stream protocol reports it after the bytes
    // queued before it (TCP on a reset), so it is left to the protocol.
    if sk.kind != SOCK_STREAM {
        let pending = sk.common.take_error();
        if pending != 0 {
            return Err(pending);
        }
    }
    sk.ops().recvmsg_with(sk, msg, deliver)
}

/// The identity a send carries: AF_UNIX's peers can ask for it.
fn send_cred(sk: &Socket) -> UCred {
    if sk.family == AF_UNIX {
        caller_ucred()
    } else {
        UCred::default()
    }
}

fn is_nonblocking(status: u32, msg_flags: u32) -> bool {
    status & status_flags::O_NONBLOCK != 0 || msg_flags & MSG_DONTWAIT != 0
}

fn new_recv(cap: usize, flags: u32, nonblock: bool) -> RecvMsg {
    RecvMsg {
        cap: cap.min(MAX_IO),
        data: Vec::new(),
        flags,
        nonblock,
        out_flags: 0,
        from: None,
        rights: Vec::new(),
    }
}

/// write(2) on a socket: a send with no flags, which always raises SIGPIPE
/// for EPIPE. Called with PM released.
pub(crate) fn socket_write(sk: &Arc<Socket>, data: &[u8], nonblock: bool) -> SyscallResult {
    to_result(
        send_all(sk, data.len(), 0, nonblock, None, Vec::new(), |at, piece| {
            piece.copy_from_slice(&data[at..at + piece.len()]);
            Ok(())
        })
        .map(|n| n as u64),
    )
}

/// read(2) on a socket: a receive with no flags. Called with PM released.
/// A read of zero bytes returns 0 at once, as Linux's sock_read_iter does.
pub(crate) fn socket_read(
    sk: &Arc<Socket>,
    buf_ptr: u64,
    count: usize,
    nonblock: bool,
) -> SyscallResult {
    if count == 0 {
        return SyscallResult::Ok(0);
    }
    to_result((|| {
        let mut msg = new_recv(count, 0, nonblock);
        let received = receive(sk, &mut msg, &mut |msg| {
            user(userptr::write_user_bytes(
                buf_ptr,
                msg.data.as_ptr(),
                msg.data.len(),
            ))
        });
        discard_rights(core::mem::take(&mut msg.rights));
        Ok(received? as u64)
    })())
}

/// sendto(fd, buf, len, flags, dest_addr, addrlen); send is sendto with no
/// address. A destination with addrlen 0 is no destination, except on a UDP
/// socket, where Linux's udp_sendmsg finds it too short (EINVAL).
pub fn sys_sendto(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    msg_flags: u64,
    dest_addr_ptr: u64,
    addrlen: u64,
) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let flags = msg_flags as u32;
        let dest = match (dest_addr_ptr, addr_len(addrlen)?) {
            (0, _) => None,
            (_, 0) if sk.family == AF_INET && sk.kind == SOCK_DGRAM => return Err(EINVAL),
            (_, 0) => None,
            (ptr, len) => Some(read_sockaddr(ptr, len, sk.family)?),
        };
        let len = len as usize;
        let sent = send_all(
            &sk,
            len,
            flags,
            is_nonblocking(status, flags),
            dest,
            Vec::new(),
            |at, piece| {
                user(userptr::read_user_bytes(
                    piece.as_mut_ptr(),
                    buf_ptr + at as u64,
                    piece.len(),
                ))
            },
        )?;
        Ok(sent as u64)
    })())
}

/// Store a received message's source address: none (a stream) stores length 0.
fn write_from(from: Option<&SockAddr>, addr_ptr: u64, addrlen_ptr: u64) -> Result<(), i32> {
    match from {
        Some(addr) => write_sockaddr(addr, addr_ptr, addrlen_ptr),
        None => user(userptr::copy_to_user(addrlen_ptr as *mut u32, &0u32)),
    }
}

/// recvfrom(fd, buf, len, flags, src_addr, addrlen); recv is recvfrom with
/// no address.
pub fn sys_recvfrom(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    msg_flags: u64,
    src_addr_ptr: u64,
    addrlen_ptr: u64,
) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let flags = msg_flags as u32;
        let len = len as usize;
        if len > 0 && !crate::memory::layout::is_valid_user_range(buf_ptr, len.min(MAX_IO)) {
            return Err(EFAULT);
        }
        let mut msg = new_recv(len, flags, is_nonblocking(status, flags));
        let received = receive(&sk, &mut msg, &mut |msg| {
            user(userptr::write_user_bytes(
                buf_ptr,
                msg.data.as_ptr(),
                msg.data.len(),
            ))
        });
        discard_rights(core::mem::take(&mut msg.rights));
        let received = received?;
        if src_addr_ptr != 0 {
            write_from(msg.from.as_ref(), src_addr_ptr, addrlen_ptr)?;
        }
        Ok(received as u64)
    })())
}

/// The Linux ABI's struct msghdr.
#[repr(C)]
#[derive(Clone, Copy)]
struct MsgHdr {
    name: u64,
    namelen: u32,
    _pad: u32,
    iov: u64,
    iovlen: u64,
    control: u64,
    controllen: u64,
    flags: i32,
    _pad2: u32,
}

const MSGHDR_NAMELEN: u64 = 8;
const MSGHDR_CONTROLLEN: u64 = 40;
const MSGHDR_FLAGS: u64 = 48;

/// struct iovec.
#[repr(C)]
#[derive(Clone, Copy)]
struct IoVec {
    base: u64,
    len: u64,
}

/// The iovec array of a msghdr: EMSGSIZE past IOV_MAX entries, EINVAL when
/// the lengths add up past what a ssize_t holds.
fn read_iovecs(ptr: u64, count: u64) -> Result<(Vec<IoVec>, usize), i32> {
    if count > IOV_MAX {
        return Err(EMSGSIZE);
    }
    let mut iovs = Vec::new();
    iovs.try_reserve_exact(count as usize).map_err(|_| ENOMEM)?;
    let mut total: usize = 0;
    for i in 0..count {
        let at = ptr
            .checked_add(i * core::mem::size_of::<IoVec>() as u64)
            .ok_or(EFAULT)?;
        let iov: IoVec = userptr::copy_from_user(at as *const IoVec).map_err(|e| e as i32)?;
        total = total
            .checked_add(iov.len as usize)
            .filter(|t| *t <= isize::MAX as usize)
            .ok_or(EINVAL)?;
        iovs.push(iov);
    }
    Ok((iovs, total))
}

/// Copy the bytes of the iovecs from byte `skip` on into `out`.
fn gather(iovs: &[IoVec], skip: usize, out: &mut [u8]) -> Result<(), i32> {
    let mut skip = skip;
    let mut at = 0;
    for iov in iovs {
        if at == out.len() {
            break;
        }
        let len = iov.len as usize;
        if skip >= len {
            skip -= len;
            continue;
        }
        let take = (len - skip).min(out.len() - at);
        user(userptr::read_user_bytes(
            out[at..].as_mut_ptr(),
            iov.base + skip as u64,
            take,
        ))?;
        at += take;
        skip = 0;
    }
    Ok(())
}

/// Scatter `data` over the iovecs.
fn scatter(iovs: &[IoVec], data: &[u8]) -> Result<(), i32> {
    let mut at = 0;
    for iov in iovs {
        if at == data.len() {
            break;
        }
        let take = (iov.len as usize).min(data.len() - at);
        user(userptr::write_user_bytes(
            iov.base,
            data[at..].as_ptr(),
            take,
        ))?;
        at += take;
    }
    Ok(())
}

/// The descriptors a sendmsg control buffer passes with SCM_RIGHTS, each a
/// counted reference taken from the caller's table. Control messages at
/// other levels are not this layer's; SCM_RIGHTS on a socket that is not
/// AF_UNIX, a malformed header, or another SOL_SOCKET type is EINVAL.
fn read_rights(sk: &Socket, control: u64, controllen: u64) -> Result<Vec<FileDescriptor>, i32> {
    if controllen == 0 {
        return Ok(Vec::new());
    }
    if controllen as usize > CONTROL_MAX {
        return Err(ENOBUFS);
    }
    let buffer = copy_in(control, controllen as usize)?;
    let mut fds: Vec<i32> = Vec::new();
    let mut at = 0usize;
    while at + CMSG_HDR <= buffer.len() {
        let len = u64::from_ne_bytes(buffer[at..at + 8].try_into().map_err(|_| EINVAL)?) as usize;
        let level = i32::from_ne_bytes(buffer[at + 8..at + 12].try_into().map_err(|_| EINVAL)?);
        let kind = i32::from_ne_bytes(buffer[at + 12..at + 16].try_into().map_err(|_| EINVAL)?);
        if len < CMSG_HDR || len > buffer.len() - at {
            return Err(EINVAL);
        }
        if level == SOL_SOCKET {
            if kind != SCM_RIGHTS || sk.family != AF_UNIX {
                return Err(EINVAL);
            }
            let count = (len - CMSG_HDR) / 4;
            if fds.len() + count > SCM_MAX_FD {
                return Err(EINVAL);
            }
            fds.try_reserve(count).map_err(|_| ENOMEM)?;
            for i in 0..count {
                let off = at + CMSG_HDR + i * 4;
                fds.push(i32::from_ne_bytes(
                    buffer[off..off + 4].try_into().map_err(|_| EINVAL)?,
                ));
            }
        }
        at += cmsg_align(len);
    }
    if fds.is_empty() {
        return Ok(Vec::new());
    }
    let mut rights = Vec::new();
    rights.try_reserve_exact(fds.len()).map_err(|_| ENOMEM)?;
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let missing = {
        let guard = crate::process::manager();
        let manager = guard.as_ref().ok_or(ESRCH)?;
        let (_, process) = manager.find_process_by_thread(tid).ok_or(ESRCH)?;
        let mut missing = false;
        for fd in &fds {
            match process.fd_table.get(*fd) {
                Some(entry) => rights.push(entry.counted_clone()),
                None => {
                    missing = true;
                    break;
                }
            }
        }
        missing
    };
    if missing {
        discard_rights(rights);
        return Err(EBADF);
    }
    Ok(rights)
}

/// sendmsg(fd, msg, flags). A msg_namelen of 0 is no destination and one
/// past sockaddr_storage is clamped to it, as Linux's copy_msghdr_from_user
/// does; a negative one is EINVAL.
pub fn sys_sendmsg(fd: u64, msg_ptr: u64, msg_flags: u64) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let flags = msg_flags as u32;
        let hdr: MsgHdr =
            userptr::copy_from_user(msg_ptr as *const MsgHdr).map_err(|e| e as i32)?;
        if (hdr.namelen as i32) < 0 {
            return Err(EINVAL);
        }
        let (iovs, total) = read_iovecs(hdr.iov, hdr.iovlen)?;
        let dest = if hdr.name != 0 && hdr.namelen != 0 {
            Some(read_sockaddr(
                hdr.name,
                hdr.namelen.min(SOCKADDR_STORAGE),
                sk.family,
            )?)
        } else {
            None
        };
        let rights = read_rights(&sk, hdr.control, hdr.controllen)?;
        let sent = send_all(
            &sk,
            total,
            flags,
            is_nonblocking(status, flags),
            dest,
            rights,
            |at, piece| gather(&iovs, at, piece),
        )?;
        Ok(sent as u64)
    })())
}

/// Install received descriptors and describe them in one SCM_RIGHTS message
/// in the caller's control buffer: as many as the buffer holds and the
/// descriptor limit allows. As on Linux this never fails the receive: what
/// cannot be installed or described is closed and the message is truncated
/// (MSG_CTRUNC). Returns the control bytes used, whether it was truncated,
/// and the descriptors installed. The numbers are written before anything
/// is installed, so a control buffer the caller cannot take installs none.
fn deliver_rights(
    rights: Vec<FileDescriptor>,
    control: u64,
    controllen: u64,
    cloexec: bool,
) -> (usize, bool, Vec<i32>) {
    if rights.is_empty() {
        return (0, false, Vec::new());
    }
    let room = if control == 0 || (controllen as usize) < CMSG_HDR {
        0
    } else {
        (controllen as usize - CMSG_HDR) / 4
    };
    let want = room.min(rights.len());
    let slots = match hold_prefix(want) {
        Ok((slots, _)) if !slots.is_empty() => slots,
        Ok((slots, _)) => {
            discard_rights(rights);
            return (0, true, slots);
        }
        Err(_) => {
            discard_rights(rights);
            return (0, true, Vec::new());
        }
    };
    let fit = slots.len();
    let len = CMSG_HDR + fit * 4;
    let mut header = [0u8; CMSG_HDR];
    header[..8].copy_from_slice(&(len as u64).to_ne_bytes());
    header[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
    header[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    let written = user(userptr::write_user_bytes(control, header.as_ptr(), CMSG_HDR)).and_then(
        |()| {
            user(userptr::write_user_bytes(
                control + CMSG_HDR as u64,
                slots.as_ptr() as *const u8,
                fit * 4,
            ))
        },
    );
    if written.is_err() {
        unhold(&slots);
        discard_rights(rights);
        return (0, true, Vec::new());
    }
    let truncated = fit < rights.len();
    let mut rights = rights.into_iter();
    install(
        &slots,
        rights.by_ref().take(fit).map(|mut entry| {
            entry.flags = if cloexec { flags::FD_CLOEXEC } else { 0 };
            entry
        }),
    );
    discard_rights(rights);
    (cmsg_align(len).min(controllen as usize), truncated, slots)
}

/// recvmsg(fd, msg, flags). The returned msg_flags carry MSG_CMSG_CLOEXEC
/// when it was asked for, as Linux's do.
pub fn sys_recvmsg(fd: u64, msg_ptr: u64, msg_flags: u64) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let flags = msg_flags as u32;
        let hdr: MsgHdr =
            userptr::copy_from_user(msg_ptr as *const MsgHdr).map_err(|e| e as i32)?;
        let (iovs, total) = read_iovecs(hdr.iov, hdr.iovlen)?;
        for iov in &iovs {
            if iov.len != 0
                && !crate::memory::layout::is_valid_user_range(iov.base, iov.len as usize)
            {
                return Err(EFAULT);
            }
        }
        let mut msg = new_recv(total, flags, is_nonblocking(status, flags));
        let received = receive(&sk, &mut msg, &mut |msg| scatter(&iovs, &msg.data));
        let rights = core::mem::take(&mut msg.rights);
        let received = match received {
            Ok(n) => n,
            Err(errno) => {
                discard_rights(rights);
                return Err(errno);
            }
        };
        let namelen_ptr = msg_ptr + MSGHDR_NAMELEN;
        let named = if hdr.name != 0 {
            write_from(msg.from.as_ref(), hdr.name, namelen_ptr)
        } else {
            user(userptr::copy_to_user(namelen_ptr as *mut u32, &0u32))
        };
        if let Err(errno) = named {
            discard_rights(rights);
            return Err(errno);
        }
        let (used, truncated, installed) = deliver_rights(
            rights,
            hdr.control,
            hdr.controllen,
            flags & MSG_CMSG_CLOEXEC != 0,
        );
        let mut out_flags = msg.out_flags | (flags & MSG_CMSG_CLOEXEC);
        if truncated {
            out_flags |= MSG_CTRUNC;
        }
        let header = user(userptr::copy_to_user(
            (msg_ptr + MSGHDR_CONTROLLEN) as *mut u64,
            &(used as u64),
        ))
        .and_then(|()| {
            user(userptr::copy_to_user(
                (msg_ptr + MSGHDR_FLAGS) as *mut i32,
                &(out_flags as i32),
            ))
        });
        if let Err(errno) = header {
            // The caller cannot learn which descriptors arrived: close them.
            close_own(&installed);
            return Err(errno);
        }
        Ok(received as u64)
    })())
}
