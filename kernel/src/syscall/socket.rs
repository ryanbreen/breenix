//! Socket system call implementations
//!
//! Implements socket, bind, sendto, recvfrom syscalls for UDP.

use alloc::boxed::Box;

use super::errno::{EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINVAL, ENETUNREACH, ENOTSOCK, ENOMEM};
use super::{ErrorCode, SyscallResult};
use crate::socket::types::{AF_INET, SOCK_DGRAM, SockAddrIn};
use crate::socket::udp::UdpSocket;
use crate::socket::{FdKind, FileDescriptor};

/// sys_socket - Create a new socket
///
/// Arguments:
///   domain: Address family (AF_INET = 2)
///   sock_type: Socket type (SOCK_DGRAM = 2 for UDP)
///   protocol: Protocol (0 = default, or IPPROTO_UDP = 17)
///
/// Returns: file descriptor on success, negative errno on error
pub fn sys_socket(domain: u64, sock_type: u64, _protocol: u64) -> SyscallResult {
    log::debug!("sys_socket: called with domain={}, type={}", domain, sock_type);

    // Validate domain
    if domain as u16 != AF_INET {
        log::debug!("sys_socket: unsupported domain {}", domain);
        return SyscallResult::Err(EAFNOSUPPORT as u64);
    }

    // Validate socket type
    if sock_type as u16 != SOCK_DGRAM {
        log::debug!("sys_socket: unsupported type {} (only UDP supported)", sock_type);
        return SyscallResult::Err(EINVAL as u64);
    }

    // Get current thread and process (same pattern as mmap.rs)
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_socket: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            log::error!("sys_socket: No process manager!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_socket: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Create UDP socket
    let socket = UdpSocket::new();
    let fd = FileDescriptor::new(FdKind::UdpSocket(Box::new(socket)), 0);

    // Allocate file descriptor in process
    match process.fd_table.alloc(fd) {
        Some(num) => {
            log::info!("UDP: Socket created fd={}", num);
            SyscallResult::Ok(num as u64)
        }
        None => {
            log::warn!("sys_socket: fd_table full (no free slots)");
            SyscallResult::Err(ENOMEM as u64)
        }
    }
}

/// sys_bind - Bind a socket to a local address
///
/// Arguments:
///   fd: Socket file descriptor
///   addr: Pointer to sockaddr_in structure
///   addrlen: Length of address structure
///
/// Returns: 0 on success, negative errno on error
pub fn sys_bind(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    // Validate address length
    if addrlen < 16 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read address from userspace
    let addr = unsafe {
        if addr_ptr == 0 {
            return SyscallResult::Err(EFAULT as u64);
        }
        let addr_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, 16);
        match SockAddrIn::from_bytes(addr_bytes) {
            Some(a) => a,
            None => return SyscallResult::Err(EINVAL as u64),
        }
    };

    // Validate address family
    if addr.family != AF_INET {
        return SyscallResult::Err(EAFNOSUPPORT as u64);
    }

    // Get current thread and process (same pattern as mmap.rs)
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_bind: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_bind: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get_mut(fd as u32) {
        Some(e) => e,
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Verify it's a UDP socket
    let socket = match &mut fd_entry.kind {
        FdKind::UdpSocket(s) => s,
        _ => return SyscallResult::Err(ENOTSOCK as u64),
    };

    // Bind the socket
    match socket.bind(pid, addr.addr, addr.port_host()) {
        Ok(()) => {
            log::info!("UDP: Socket bound to port {}", addr.port_host());
            SyscallResult::Ok(0)
        }
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// sys_sendto - Send data to a destination address
///
/// Arguments:
///   fd: Socket file descriptor
///   buf: Pointer to data buffer
///   len: Length of data
///   flags: Send flags (ignored for now)
///   dest_addr: Pointer to destination sockaddr_in
///   addrlen: Length of address structure
///
/// Returns: bytes sent on success, negative errno on error
pub fn sys_sendto(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    _flags: u64,
    dest_addr_ptr: u64,
    addrlen: u64,
) -> SyscallResult {
    // Validate pointers
    if buf_ptr == 0 || dest_addr_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Validate address length
    if addrlen < 16 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read destination address from userspace
    let dest_addr = unsafe {
        let addr_bytes = core::slice::from_raw_parts(dest_addr_ptr as *const u8, 16);
        match SockAddrIn::from_bytes(addr_bytes) {
            Some(a) => a,
            None => return SyscallResult::Err(EINVAL as u64),
        }
    };

    // Read data from userspace (copy to owned buffer so we can release lock)
    let data: alloc::vec::Vec<u8> = unsafe {
        core::slice::from_raw_parts(buf_ptr as *const u8, len as usize).to_vec()
    };

    // Extract source port while holding process manager lock, then release it
    // This prevents deadlock when loopback delivery needs the same lock
    let src_port = {
        let current_thread_id = match crate::per_cpu::current_thread() {
            Some(thread) => thread.id,
            None => {
                log::error!("sys_sendto: No current thread in per-CPU data!");
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let manager_guard = crate::process::manager();
        let manager = match &*manager_guard {
            Some(m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (_pid, process) = match manager.find_process_by_thread(current_thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_sendto: No process found for thread_id={}", current_thread_id);
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Get the socket from fd table
        let fd_entry = match process.fd_table.get(fd as u32) {
            Some(e) => e,
            None => return SyscallResult::Err(EBADF as u64),
        };

        // Verify it's a UDP socket and extract source port
        match &fd_entry.kind {
            FdKind::UdpSocket(s) => s.local_port().unwrap_or(0),
            _ => return SyscallResult::Err(ENOTSOCK as u64),
        }
        // manager_guard dropped here, releasing the lock
    };

    // Now send without holding the process manager lock
    // Build UDP packet
    let udp_packet = crate::net::udp::build_udp_packet(src_port, dest_addr.port_host(), &data);

    // Send via IP layer
    let result = crate::net::send_ipv4(dest_addr.addr, crate::net::ipv4::PROTOCOL_UDP, &udp_packet);

    // Drain any loopback packets that were queued during send
    // This is safe now because we don't hold the process manager lock
    crate::net::drain_loopback_queue();

    match result {
        Ok(()) => {
            log::info!("UDP: Packet sent successfully, bytes={}", data.len());
            SyscallResult::Ok(data.len() as u64)
        }
        Err(_) => SyscallResult::Err(ENETUNREACH as u64),
    }
}

/// sys_recvfrom - Receive data from a socket
///
/// Arguments:
///   fd: Socket file descriptor
///   buf: Pointer to receive buffer
///   len: Length of buffer
///   flags: Receive flags (ignored for now)
///   src_addr: Pointer to sockaddr_in for source address (can be NULL)
///   addrlen: Pointer to address length (can be NULL)
///
/// Returns: bytes received on success, negative errno on error
pub fn sys_recvfrom(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    _flags: u64,
    src_addr_ptr: u64,
    addrlen_ptr: u64,
) -> SyscallResult {
    // Validate buffer pointer
    if buf_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Get current thread and process (same pattern as mmap.rs)
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_recvfrom: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_recvfrom: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get_mut(fd as u32) {
        Some(e) => e,
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Verify it's a UDP socket
    let socket = match &mut fd_entry.kind {
        FdKind::UdpSocket(s) => s,
        _ => return SyscallResult::Err(ENOTSOCK as u64),
    };

    // Try to receive a packet
    let packet = match socket.recv_from() {
        Some(p) => p,
        None => return SyscallResult::Err(EAGAIN as u64), // Would block
    };

    // Copy data to userspace
    let copy_len = core::cmp::min(len as usize, packet.data.len());
    unsafe {
        let buf = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len);
        buf.copy_from_slice(&packet.data[..copy_len]);
    }

    // Write source address if requested
    if src_addr_ptr != 0 && addrlen_ptr != 0 {
        let src_addr = SockAddrIn::new(packet.src_addr, packet.src_port);
        let addr_bytes = src_addr.to_bytes();
        unsafe {
            let addrlen = *(addrlen_ptr as *const u32);
            let copy_addr_len = core::cmp::min(addrlen as usize, addr_bytes.len());
            let addr_buf = core::slice::from_raw_parts_mut(src_addr_ptr as *mut u8, copy_addr_len);
            addr_buf.copy_from_slice(&addr_bytes[..copy_addr_len]);
            *(addrlen_ptr as *mut u32) = addr_bytes.len() as u32;
        }
    }

    log::debug!("UDP: Received {} bytes from {}.{}.{}.{}:{}",
        copy_len,
        packet.src_addr[0], packet.src_addr[1], packet.src_addr[2], packet.src_addr[3],
        packet.src_port
    );

    SyscallResult::Ok(copy_len as u64)
}
