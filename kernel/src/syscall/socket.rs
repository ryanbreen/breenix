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
    EPROTONOSUPPORT, ESRCH,
};
use super::{userptr, SyscallResult};
use crate::ipc::fd::{flags, status_flags, FdKind, FileDescriptor};
use crate::socket::addr::{read_sockaddr, write_sockaddr, SockAddr};
use crate::socket::object::{
    caller_ucred, socket_from_fd, OptBuf, RecvMsg, SendMsg, Socket, SocketProto, UCred,
};
use crate::socket::{opts, tcp::TcpSock, udp::UdpSock, unix::UnixSock};

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
const SOCK_STREAM: u16 = 1;
const SOCK_DGRAM: u16 = 2;
const SOCK_TYPE_MASK: u64 = 0xf;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const IPPROTO_TCP: u64 = 6;
const IPPROTO_UDP: u64 = 17;
/// PF_UNIX, which Linux accepts as AF_UNIX's protocol.
const PROTO_UNIX: u64 = 1;

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
/// The most bytes one send or receive moves through the kernel. Streams
/// return a short count; a datagram this long is too long for UDP anyway.
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

/// Put `entries` in the caller's descriptor table, all or none: their
/// numbers, in order.
fn install_all(entries: Vec<FileDescriptor>) -> Result<Vec<i32>, i32> {
    let mut fds = Vec::new();
    fds.try_reserve_exact(entries.len()).map_err(|_| ENOMEM)?;
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let mut guard = crate::process::manager();
    let manager = guard.as_mut().ok_or(ESRCH)?;
    let (_, process) = manager.find_process_by_thread_mut(tid).ok_or(ESRCH)?;
    for entry in entries {
        match process.fd_table.alloc_with_entry(entry) {
            Ok(fd) => fds.push(fd),
            Err(errno) => {
                for fd in &fds {
                    let _ = process.fd_table.close(*fd);
                }
                return Err(errno);
            }
        }
    }
    Ok(fds)
}

fn socket_entry(socket: Arc<Socket>, cloexec: bool, nonblock: bool) -> FileDescriptor {
    FileDescriptor::with_flags(
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
/// pipe, FIFO and PTY reference counts drop with them.
fn discard_rights(rights: Vec<FileDescriptor>) {
    if rights.is_empty() {
        return;
    }
    let mut entries = Vec::new();
    if entries.try_reserve_exact(rights.len()).is_err() {
        // Without room to pair them with numbers, close them one at a time.
        for right in rights {
            let mut single = Vec::new();
            if single.try_reserve_exact(1).is_ok() {
                single.push((0usize, right));
                crate::task::process_task::close_extracted_fds(single);
            }
        }
        return;
    }
    entries.extend(rights.into_iter().map(|right| (0usize, right)));
    crate::task::process_task::close_extracted_fds(entries);
}

/// The socket a new descriptor gets for `family`, `kind` and `protocol`.
fn create(family: u64, kind: u16, protocol: u64) -> Result<Arc<Socket>, i32> {
    match family as u16 {
        AF_INET => match kind {
            SOCK_STREAM => {
                if protocol != 0 && protocol != IPPROTO_TCP {
                    return Err(EPROTONOSUPPORT);
                }
                Socket::new(AF_INET, SOCK_STREAM, 6, SocketProto::Tcp(TcpSock::new()))
            }
            SOCK_DGRAM => {
                if protocol != 0 && protocol != IPPROTO_UDP {
                    return Err(EPROTONOSUPPORT);
                }
                Socket::new(AF_INET, SOCK_DGRAM, 17, SocketProto::Udp(UdpSock::new()?))
            }
            _ => Err(EINVAL),
        },
        AF_UNIX => {
            if protocol != 0 && protocol != PROTO_UNIX {
                return Err(EPROTONOSUPPORT);
            }
            if !UnixSock::supports_kind(kind) {
                return Err(EINVAL);
            }
            Socket::new(AF_UNIX, kind, 0, SocketProto::Unix(UnixSock::new(kind)))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

/// Split socket()'s type argument into the type and the SOCK_NONBLOCK and
/// SOCK_CLOEXEC flags; any other flag is EINVAL.
fn split_type(sock_type: u64) -> Result<(u16, bool, bool), i32> {
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

/// socket(domain, type, protocol)
pub fn sys_socket(domain: u64, sock_type: u64, protocol: u64) -> SyscallResult {
    to_result((|| {
        let (kind, nonblock, cloexec) = split_type(sock_type)?;
        let socket = create(domain, kind, protocol)?;
        let fds = install_all(alloc::vec![socket_entry(socket, cloexec, nonblock)])?;
        Ok(fds[0] as u64)
    })())
}

/// socketpair(domain, type, protocol, sv). Only AF_UNIX makes pairs; Linux
/// gives AF_INET EOPNOTSUPP once its type and protocol check out.
pub fn sys_socketpair(domain: u64, sock_type: u64, protocol: u64, sv_ptr: u64) -> SyscallResult {
    to_result((|| {
        let (kind, nonblock, cloexec) = split_type(sock_type)?;
        match domain as u16 {
            AF_UNIX => {}
            AF_INET => {
                create(domain, kind, protocol)?;
                return Err(EOPNOTSUPP);
            }
            _ => return Err(EAFNOSUPPORT),
        }
        if protocol != 0 && protocol != PROTO_UNIX {
            return Err(EPROTONOSUPPORT);
        }
        if !UnixSock::supports_kind(kind) {
            return Err(EINVAL);
        }
        if sv_ptr == 0 {
            return Err(EFAULT);
        }
        let (a, b) = UnixSock::new_pair(kind, caller_ucred())?;
        let fds = install_all(alloc::vec![
            socket_entry(a, cloexec, nonblock),
            socket_entry(b, cloexec, nonblock),
        ])?;
        let sv = [fds[0], fds[1]];
        // sv may be a copy-on-write page: write it with PM released.
        if let Err(errno) = userptr::copy_to_user(sv_ptr as *mut [i32; 2], &sv) {
            close_own(&sv);
            return Err(errno as i32);
        }
        Ok(0)
    })())
}

/// Close descriptors this call installed before it failed.
fn close_own(fds: &[i32]) {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let closed: Vec<FileDescriptor> = {
        let mut guard = crate::process::manager();
        let Some(process) = guard
            .as_mut()
            .and_then(|manager| manager.find_process_by_thread_mut(tid))
            .map(|(_, process)| process)
        else {
            return;
        };
        fds.iter()
            .filter_map(|fd| process.fd_table.close(*fd).ok())
            .collect()
    };
    // The sockets are released here, with PM dropped.
    drop(closed);
}

/// The length argument of bind and connect: Linux reads it as an int.
fn addr_len(len: u64) -> Result<u32, i32> {
    if len > i32::MAX as u64 {
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
        if accept_flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
            return Err(EINVAL);
        }
        let (sk, status) = socket_from_fd(fd as i32)?;
        let nonblock = status & status_flags::O_NONBLOCK != 0;
        let (socket, peer) = sk.ops().accept(&sk, nonblock)?;
        if addr_ptr != 0 {
            // A peer address the caller cannot take fails the call, and the
            // new socket goes with it.
            write_sockaddr(&peer, addr_ptr, addrlen_ptr)?;
        }
        let fds = install_all(alloc::vec![socket_entry(
            socket,
            accept_flags & SOCK_CLOEXEC != 0,
            accept_flags & SOCK_NONBLOCK != 0,
        )])?;
        Ok(fds[0] as u64)
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
        write_sockaddr(&sk.ops().local_name(&sk), addr_ptr, addrlen_ptr)?;
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
        // No option this kernel knows is longer than 16 bytes.
        let mut value = [0u8; 64];
        let n = (len as usize).min(value.len());
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

/// Send `msg` on `sk`: report a pending error first, close any descriptors
/// the protocol did not take, and raise SIGPIPE for EPIPE unless MSG_NOSIGNAL.
fn send(sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
    let pending = sk.common.take_error();
    let result = if pending != 0 {
        Err(pending)
    } else {
        sk.ops().sendmsg(sk, msg)
    };
    discard_rights(core::mem::take(&mut msg.rights));
    if result == Err(EPIPE) && msg.flags & MSG_NOSIGNAL == 0 {
        super::signal::raise_sigpipe();
    }
    result
}

/// Receive into `msg` on `sk`, reporting a pending error first.
fn receive(sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
    let pending = sk.common.take_error();
    if pending != 0 {
        return Err(pending);
    }
    sk.ops().recvmsg(sk, msg)
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

/// write(2) on a socket: a send with no flags, which always raises SIGPIPE
/// for EPIPE. Called with PM released.
pub(crate) fn socket_write(sk: &Arc<Socket>, data: &[u8], nonblock: bool) -> SyscallResult {
    let mut msg = SendMsg {
        data,
        dest: None,
        flags: 0,
        nonblock,
        rights: Vec::new(),
        cred: send_cred(sk),
    };
    to_result(send(sk, &mut msg).map(|n| n as u64))
}

/// read(2) on a socket: a receive with no flags. Called with PM released.
pub(crate) fn socket_read(
    sk: &Arc<Socket>,
    buf_ptr: u64,
    count: usize,
    nonblock: bool,
) -> SyscallResult {
    to_result((|| {
        let mut msg = RecvMsg {
            cap: count.min(MAX_IO),
            data: Vec::new(),
            flags: 0,
            nonblock,
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        let received = receive(sk, &mut msg);
        discard_rights(core::mem::take(&mut msg.rights));
        let received = received?;
        user(userptr::write_user_bytes(
            buf_ptr,
            msg.data.as_ptr(),
            msg.data.len(),
        ))?;
        Ok(received as u64)
    })())
}

/// sendto(fd, buf, len, flags, dest_addr, addrlen); send is sendto with no
/// address.
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
        let dest = if dest_addr_ptr != 0 {
            Some(read_sockaddr(dest_addr_ptr, addr_len(addrlen)?, sk.family)?)
        } else {
            None
        };
        let data = copy_in(buf_ptr, (len as usize).min(MAX_IO))?;
        let mut msg = SendMsg {
            data: &data,
            dest,
            flags,
            nonblock: is_nonblocking(status, flags),
            rights: Vec::new(),
            cred: send_cred(&sk),
        };
        Ok(send(&sk, &mut msg)? as u64)
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
        let mut msg = RecvMsg {
            cap: len.min(MAX_IO),
            data: Vec::new(),
            flags,
            nonblock: is_nonblocking(status, flags),
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        let received = receive(&sk, &mut msg);
        discard_rights(core::mem::take(&mut msg.rights));
        let received = received?;
        user(userptr::write_user_bytes(
            buf_ptr,
            msg.data.as_ptr(),
            msg.data.len(),
        ))?;
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

/// Gather up to MAX_IO bytes from the iovecs.
fn gather(iovs: &[IoVec], total: usize) -> Result<Vec<u8>, i32> {
    let len = total.min(MAX_IO);
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    for iov in iovs {
        let room = len - data.len();
        if room == 0 {
            break;
        }
        let take = (iov.len as usize).min(room);
        let at = data.len();
        data.resize(at + take, 0);
        user(userptr::read_user_bytes(
            data[at..].as_mut_ptr(),
            iov.base,
            take,
        ))?;
    }
    Ok(data)
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

/// sendmsg(fd, msg, flags)
pub fn sys_sendmsg(fd: u64, msg_ptr: u64, msg_flags: u64) -> SyscallResult {
    to_result((|| {
        let (sk, status) = socket_from_fd(fd as i32)?;
        let flags = msg_flags as u32;
        let hdr: MsgHdr =
            userptr::copy_from_user(msg_ptr as *const MsgHdr).map_err(|e| e as i32)?;
        let (iovs, total) = read_iovecs(hdr.iov, hdr.iovlen)?;
        let dest = if hdr.name != 0 && hdr.namelen != 0 {
            Some(read_sockaddr(hdr.name, hdr.namelen, sk.family)?)
        } else {
            None
        };
        let data = gather(&iovs, total)?;
        let rights = read_rights(&sk, hdr.control, hdr.controllen)?;
        let mut msg = SendMsg {
            data: &data,
            dest,
            flags,
            nonblock: is_nonblocking(status, flags),
            rights,
            cred: send_cred(&sk),
        };
        Ok(send(&sk, &mut msg)? as u64)
    })())
}

/// Install received descriptors and describe them in the caller's control
/// buffer, as many as fit. Returns the control bytes used and whether some
/// were left out (MSG_CTRUNC); those are closed.
fn deliver_rights(
    rights: Vec<FileDescriptor>,
    control: u64,
    controllen: u64,
    cloexec: bool,
) -> Result<(usize, bool), i32> {
    if rights.is_empty() {
        return Ok((0, false));
    }
    let room = if control == 0 || (controllen as usize) < CMSG_HDR {
        0
    } else {
        (controllen as usize - CMSG_HDR) / 4
    };
    let mut rights = rights;
    let fit = room.min(rights.len());
    let rest = rights.split_off(fit);
    let truncated = !rest.is_empty();
    discard_rights(rest);
    if fit == 0 {
        return Ok((0, truncated));
    }
    let entries: Vec<FileDescriptor> = rights
        .into_iter()
        .map(|mut entry| {
            entry.flags = if cloexec { flags::FD_CLOEXEC } else { 0 };
            entry
        })
        .collect();
    let fds = install_all(entries)?;
    let len = CMSG_HDR + fds.len() * 4;
    let mut cmsg = Vec::new();
    cmsg.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    cmsg.extend_from_slice(&(len as u64).to_ne_bytes());
    cmsg.extend_from_slice(&SOL_SOCKET.to_ne_bytes());
    cmsg.extend_from_slice(&SCM_RIGHTS.to_ne_bytes());
    for fd in &fds {
        cmsg.extend_from_slice(&fd.to_ne_bytes());
    }
    if let Err(errno) = user(userptr::write_user_bytes(control, cmsg.as_ptr(), len)) {
        close_own(&fds);
        return Err(errno);
    }
    Ok((cmsg_align(len).min(controllen as usize), truncated))
}

/// recvmsg(fd, msg, flags)
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
        let mut msg = RecvMsg {
            cap: total.min(MAX_IO),
            data: Vec::new(),
            flags,
            nonblock: is_nonblocking(status, flags),
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        let received = receive(&sk, &mut msg);
        let rights = core::mem::take(&mut msg.rights);
        let received = match received {
            Ok(n) => n,
            Err(errno) => {
                discard_rights(rights);
                return Err(errno);
            }
        };
        scatter(&iovs, &msg.data)?;
        let namelen_ptr = msg_ptr + MSGHDR_NAMELEN;
        if hdr.name != 0 {
            write_from(msg.from.as_ref(), hdr.name, namelen_ptr)?;
        } else {
            user(userptr::copy_to_user(namelen_ptr as *mut u32, &0u32))?;
        }
        let (used, truncated) = deliver_rights(
            rights,
            hdr.control,
            hdr.controllen,
            flags & MSG_CMSG_CLOEXEC != 0,
        )?;
        let mut out_flags = msg.out_flags;
        if truncated {
            out_flags |= MSG_CTRUNC;
        }
        user(userptr::copy_to_user(
            (msg_ptr + MSGHDR_CONTROLLEN) as *mut u64,
            &(used as u64),
        ))?;
        user(userptr::copy_to_user(
            (msg_ptr + MSGHDR_FLAGS) as *mut i32,
            &(out_flags as i32),
        ))?;
        Ok(received as u64)
    })())
}
