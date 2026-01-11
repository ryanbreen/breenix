//! Socket system call implementations
//!
//! Implements socket, bind, sendto, recvfrom syscalls for UDP and TCP.

use super::errno::{EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINVAL, ENETUNREACH, ENOTSOCK, EADDRINUSE, ENOTCONN, EISCONN, EOPNOTSUPP, ECONNREFUSED, ETIMEDOUT};
use super::{ErrorCode, SyscallResult};
use crate::socket::types::{AF_INET, SOCK_DGRAM, SOCK_STREAM, SockAddrIn};
use crate::socket::udp::UdpSocket;
use crate::ipc::fd::FdKind;

/// sys_socket - Create a new socket
///
/// Arguments:
///   domain: Address family (AF_INET = 2)
///   sock_type: Socket type (SOCK_DGRAM = 2 for UDP, SOCK_STREAM = 1 for TCP)
///   protocol: Protocol (0 = default)
///
/// Returns: file descriptor on success, negative errno on error
pub fn sys_socket(domain: u64, sock_type: u64, _protocol: u64) -> SyscallResult {
    log::debug!("sys_socket: called with domain={}, type={}", domain, sock_type);

    // Validate domain
    if domain as u16 != AF_INET {
        log::debug!("sys_socket: unsupported domain {}", domain);
        return SyscallResult::Err(EAFNOSUPPORT as u64);
    }

    // Get current thread and process
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

    // Create socket based on type
    let fd_kind = match sock_type as u16 {
        SOCK_DGRAM => {
            // Create UDP socket wrapped in Arc<Mutex<>> for sharing
            let socket = alloc::sync::Arc::new(spin::Mutex::new(UdpSocket::new()));
            FdKind::UdpSocket(socket)
        }
        SOCK_STREAM => {
            // Create TCP socket (initially unbound, port = 0)
            FdKind::TcpSocket(0)
        }
        _ => {
            log::debug!("sys_socket: unsupported type {}", sock_type);
            return SyscallResult::Err(EINVAL as u64);
        }
    };

    // Allocate file descriptor in process
    match process.fd_table.alloc(fd_kind) {
        Ok(num) => {
            let kind_str = if sock_type as u16 == SOCK_STREAM { "TCP" } else { "UDP" };
            log::info!("{}: Socket created fd={}", kind_str, num);
            SyscallResult::Ok(num as u64)
        }
        Err(e) => {
            log::warn!("sys_socket: fd_table full (no free slots)");
            SyscallResult::Err(e as u64)
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
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e.clone(),
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Handle bind based on socket type
    match &fd_entry.kind {
        FdKind::UdpSocket(s) => {
            // Bind UDP socket
            let socket_ref = s.clone();
            let mut socket = socket_ref.lock();
            match socket.bind(pid, addr.addr, addr.port_host()) {
                Ok(()) => {
                    log::info!("UDP: Socket bound to port {}", addr.port_host());
                    SyscallResult::Ok(0)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        FdKind::TcpSocket(existing_port) => {
            // TCP socket binding - update the socket's port
            if *existing_port != 0 {
                // Already bound
                return SyscallResult::Err(EINVAL as u64);
            }

            let port = addr.port_host();

            // Check if port is already in use by another TCP listener
            {
                let listeners = crate::net::tcp::TCP_LISTENERS.lock();
                if listeners.contains_key(&port) {
                    log::debug!("TCP: bind failed, port {} already in use", port);
                    return SyscallResult::Err(EADDRINUSE as u64);
                }
            }

            // Update the fd entry with the bound port
            let fd_num = fd as usize;
            if let Some(entry) = process.fd_table.get_mut(fd_num as i32) {
                entry.kind = FdKind::TcpSocket(port);
            }

            log::info!("TCP: Socket bound to port {}", port);
            SyscallResult::Ok(0)
        }
        _ => SyscallResult::Err(ENOTSOCK as u64),
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
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(e) => e,
            None => return SyscallResult::Err(EBADF as u64),
        };

        // Verify it's a UDP socket and extract source port
        match &fd_entry.kind {
            FdKind::UdpSocket(s) => s.lock().local_port().unwrap_or(0),
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
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e,
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Verify it's a UDP socket and get Arc<Mutex<>> reference
    let socket_ref = match &fd_entry.kind {
        FdKind::UdpSocket(s) => s.clone(),
        _ => return SyscallResult::Err(ENOTSOCK as u64),
    };

    // Try to receive a packet (lock the mutex)
    let packet = match socket_ref.lock().recv_from() {
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

/// sys_listen - Mark a TCP socket as listening for connections
///
/// Arguments:
///   fd: Socket file descriptor (must be bound)
///   backlog: Maximum pending connections
///
/// Returns: 0 on success, negative errno on error
pub fn sys_listen(fd: u64, backlog: u64) -> SyscallResult {
    log::debug!("sys_listen: fd={}, backlog={}", fd, backlog);

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_listen: No current thread in per-CPU data!");
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
            log::error!("sys_listen: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e.clone(),
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Must be a bound TCP socket
    let port = match &fd_entry.kind {
        FdKind::TcpSocket(p) => {
            if *p == 0 {
                // Not bound
                return SyscallResult::Err(EINVAL as u64);
            }
            *p
        }
        FdKind::TcpListener(_) => {
            // Already listening
            return SyscallResult::Err(EINVAL as u64);
        }
        _ => return SyscallResult::Err(EOPNOTSUPP as u64),
    };

    // Start listening
    if let Err(_) = crate::net::tcp::tcp_listen(port, backlog as usize, pid) {
        return SyscallResult::Err(EADDRINUSE as u64);
    }

    // Update fd to TcpListener
    if let Some(entry) = process.fd_table.get_mut(fd as i32) {
        entry.kind = FdKind::TcpListener(port);
    }

    log::info!("TCP: Socket now listening on port {}", port);
    SyscallResult::Ok(0)
}

/// sys_accept - Accept a connection on a listening socket
///
/// Arguments:
///   fd: Listening socket file descriptor
///   addr: Pointer to sockaddr_in for client address (can be NULL)
///   addrlen: Pointer to address length (can be NULL)
///
/// Returns: new socket fd on success, negative errno on error
pub fn sys_accept(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    log::debug!("sys_accept: fd={}", fd);

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_accept: No current thread in per-CPU data!");
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
            log::error!("sys_accept: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e.clone(),
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Must be a TCP listener
    let port = match &fd_entry.kind {
        FdKind::TcpListener(p) => *p,
        _ => return SyscallResult::Err(EOPNOTSUPP as u64),
    };

    // Try to accept a pending connection
    let conn_id = match crate::net::tcp::tcp_accept(port) {
        Some(id) => id,
        None => return SyscallResult::Err(EAGAIN as u64), // No pending connections
    };

    // Write client address if requested
    if addr_ptr != 0 && addrlen_ptr != 0 {
        let client_addr = SockAddrIn::new(conn_id.remote_ip, conn_id.remote_port);
        let addr_bytes = client_addr.to_bytes();
        unsafe {
            let addrlen = *(addrlen_ptr as *const u32);
            let copy_addr_len = core::cmp::min(addrlen as usize, addr_bytes.len());
            let addr_buf = core::slice::from_raw_parts_mut(addr_ptr as *mut u8, copy_addr_len);
            addr_buf.copy_from_slice(&addr_bytes[..copy_addr_len]);
            *(addrlen_ptr as *mut u32) = addr_bytes.len() as u32;
        }
    }

    // Create new fd for the connection
    match process.fd_table.alloc(FdKind::TcpConnection(conn_id)) {
        Ok(new_fd) => {
            log::info!("TCP: Accepted connection on fd {}, new fd {}", fd, new_fd);
            SyscallResult::Ok(new_fd as u64)
        }
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// sys_connect - Initiate a TCP connection
///
/// Arguments:
///   fd: Socket file descriptor
///   addr: Pointer to destination sockaddr_in
///   addrlen: Length of address structure
///
/// Returns: 0 on success, negative errno on error
pub fn sys_connect(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    log::debug!("sys_connect: fd={}", fd);

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

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_connect: No current thread in per-CPU data!");
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
            log::error!("sys_connect: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e.clone(),
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Handle connect based on socket type
    match &fd_entry.kind {
        FdKind::TcpSocket(local_port) => {
            // Assign ephemeral port if not bound
            let port = if *local_port == 0 {
                // Use a simple ephemeral port allocation
                static EPHEMERAL_PORT: core::sync::atomic::AtomicU16 =
                    core::sync::atomic::AtomicU16::new(49152);
                EPHEMERAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
            } else {
                *local_port
            };

            // Initiate connection
            let conn_id = match crate::net::tcp::tcp_connect(
                port,
                addr.addr,
                addr.port_host(),
                pid,
            ) {
                Ok(id) => id,
                Err(_) => return SyscallResult::Err(ECONNREFUSED as u64),
            };

            // Update fd to TcpConnection
            if let Some(entry) = process.fd_table.get_mut(fd as i32) {
                entry.kind = FdKind::TcpConnection(conn_id);
            }

            log::info!("TCP: Connect initiated to {}.{}.{}.{}:{}",
                addr.addr[0], addr.addr[1], addr.addr[2], addr.addr[3],
                addr.port_host());

            // Drop manager lock before waiting
            drop(manager_guard);

            // Wait for connection to establish (poll with yields)
            const MAX_WAIT_ITERATIONS: u32 = 1000;
            for i in 0..MAX_WAIT_ITERATIONS {
                // Poll for incoming packets (process SYN-ACK)
                crate::net::process_rx();
                // Also drain loopback queue for localhost connections
                crate::net::drain_loopback_queue();

                // Check if connected
                if crate::net::tcp::tcp_is_established(&conn_id) {
                    // Drain loopback one more time to deliver the ACK to the server
                    // This ensures the server's pending connection has ack_received = true
                    crate::net::drain_loopback_queue();
                    log::info!("TCP: Connection established after {} iterations", i);
                    return SyscallResult::Ok(0);
                }

                // Check if connection failed
                if crate::net::tcp::tcp_is_failed(&conn_id) {
                    log::warn!("TCP: Connection failed");
                    return SyscallResult::Err(ECONNREFUSED as u64);
                }

                // Yield to allow other processing
                crate::task::scheduler::yield_current();
            }

            // Timeout
            log::warn!("TCP: Connect timed out waiting for handshake");
            SyscallResult::Err(ETIMEDOUT as u64)
        }
        FdKind::TcpConnection(_) => {
            // Already connected
            SyscallResult::Err(EISCONN as u64)
        }
        _ => SyscallResult::Err(EOPNOTSUPP as u64),
    }
}

/// sys_shutdown - Shut down part of a full-duplex connection
///
/// Arguments:
///   fd: Socket file descriptor
///   how: SHUT_RD (0), SHUT_WR (1), or SHUT_RDWR (2)
///
/// Returns: 0 on success, negative errno on error
pub fn sys_shutdown(fd: u64, how: u64) -> SyscallResult {
    log::debug!("sys_shutdown: fd={}, how={}", fd, how);

    // Validate how parameter
    if how > 2 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_shutdown: No current thread in per-CPU data!");
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
            log::error!("sys_shutdown: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get the socket from fd table
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(e) => e.clone(),
        None => return SyscallResult::Err(EBADF as u64),
    };

    // Must be a TCP connection
    match &fd_entry.kind {
        FdKind::TcpConnection(conn_id) => {
            // Set shutdown flags on the connection
            let shut_rd = how == 0 || how == 2; // SHUT_RD or SHUT_RDWR
            let shut_wr = how == 1 || how == 2; // SHUT_WR or SHUT_RDWR

            crate::net::tcp::tcp_shutdown(conn_id, shut_rd, shut_wr);

            log::info!("TCP: Shutdown fd={} how={}", fd, how);
            SyscallResult::Ok(0)
        }
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) => {
            // Not connected
            SyscallResult::Err(ENOTCONN as u64)
        }
        _ => SyscallResult::Err(EOPNOTSUPP as u64),
    }
}
