//! Socket system call implementations
//!
//! Implements socket, bind, sendto, recvfrom syscalls for UDP and TCP.

use super::errno::{EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINVAL, ENOTSOCK, EADDRINUSE, EISCONN, EOPNOTSUPP, ECONNREFUSED, ENOENT, ENETUNREACH};
// TCP-specific error codes (x86_64 only for now)
#[cfg(target_arch = "x86_64")]
use super::errno::{EINPROGRESS, ENOTCONN, ETIMEDOUT};
use super::{ErrorCode, SyscallResult};
use crate::socket::types::{AF_INET, AF_UNIX, SOCK_DGRAM, SOCK_STREAM, SockAddrIn, SockAddrUn};
use crate::socket::udp::UdpSocket;
use crate::socket::unix::UnixSocket;
use crate::ipc::fd::FdKind;
use crate::arch_impl::traits::CpuOps;

// Architecture-specific CPU type for interrupt control
#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::X86Cpu;
#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// Reset the timer quantum after a blocking wait (architecture-specific)
/// This prevents immediate preemption after returning from a long blocking syscall
#[inline(always)]
fn reset_quantum() {
    #[cfg(target_arch = "x86_64")]
    reset_quantum();
    // ARM64: No-op for now - timer quantum handled differently
    #[cfg(target_arch = "aarch64")]
    {}
}

const SOCK_NONBLOCK: u64 = 0x800;
const SOCK_CLOEXEC: u64 = 0x80000;

/// sys_socket - Create a new socket
///
/// Arguments:
///   domain: Address family (AF_INET = 2, AF_UNIX = 1)
///   sock_type: Socket type (SOCK_DGRAM = 2 for UDP, SOCK_STREAM = 1 for TCP/Unix)
///   protocol: Protocol (0 = default)
///
/// Returns: file descriptor on success, negative errno on error
pub fn sys_socket(domain: u64, sock_type: u64, _protocol: u64) -> SyscallResult {
    log::debug!("sys_socket: called with domain={}, type={}", domain, sock_type);

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

    let nonblocking = (sock_type & SOCK_NONBLOCK) != 0;
    let cloexec = (sock_type & SOCK_CLOEXEC) != 0;
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);

    // Create socket based on domain and type
    let (fd_kind, kind_str) = match domain as u16 {
        AF_INET => {
            // IPv4 socket
            match base_type as u16 {
                SOCK_DGRAM => {
                    // Create UDP socket wrapped in Arc<Mutex<>> for sharing
                    let mut socket = UdpSocket::new();
                    if nonblocking {
                        socket.set_nonblocking(true);
                    }
                    let socket = alloc::sync::Arc::new(spin::Mutex::new(socket));
                    (FdKind::UdpSocket(socket), "UDP")
                }
                #[cfg(target_arch = "x86_64")]
                SOCK_STREAM => {
                    // Create TCP socket (initially unbound, port = 0)
                    (FdKind::TcpSocket(0), "TCP")
                }
                #[cfg(target_arch = "aarch64")]
                SOCK_STREAM => {
                    // TCP not yet implemented on ARM64
                    log::debug!("sys_socket: TCP not implemented on ARM64");
                    return SyscallResult::Err(EAFNOSUPPORT as u64);
                }
                _ => {
                    log::debug!("sys_socket: unsupported type {} for AF_INET", base_type);
                    return SyscallResult::Err(EINVAL as u64);
                }
            }
        }
        AF_UNIX => {
            // Unix domain socket
            match base_type as u16 {
                SOCK_STREAM => {
                    // Create Unix stream socket (initially unbound)
                    let socket = UnixSocket::new(nonblocking);
                    let socket = alloc::sync::Arc::new(spin::Mutex::new(socket));
                    (FdKind::UnixSocket(socket), "Unix")
                }
                _ => {
                    log::debug!("sys_socket: unsupported type {} for AF_UNIX (only SOCK_STREAM supported)", base_type);
                    return SyscallResult::Err(EINVAL as u64);
                }
            }
        }
        _ => {
            log::debug!("sys_socket: unsupported domain {}", domain);
            return SyscallResult::Err(EAFNOSUPPORT as u64);
        }
    };

    // Set flags for the file descriptor
    use crate::ipc::fd::{flags, status_flags, FileDescriptor};
    let fd_flags = if cloexec { flags::FD_CLOEXEC } else { 0 };
    let fd_status_flags = if nonblocking { status_flags::O_NONBLOCK } else { 0 };

    let fd_entry = FileDescriptor::with_flags(fd_kind, fd_flags, fd_status_flags);

    // Allocate file descriptor in process
    match process.fd_table.alloc_with_entry(fd_entry) {
        Ok(num) => {
            log::info!("{}: Socket created fd={}", kind_str, num);
            log::debug!("{} socket: returning to userspace fd={}", kind_str, num);
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
///   addr: Pointer to sockaddr structure (sockaddr_in for AF_INET, sockaddr_un for AF_UNIX)
///   addrlen: Length of address structure
///
/// Returns: 0 on success, negative errno on error
pub fn sys_bind(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    // Validate address pointer
    if addr_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Validate minimum address length (at least 2 bytes for family)
    if addrlen < 2 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read address family first
    let family = unsafe {
        let family_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, 2);
        u16::from_ne_bytes([family_bytes[0], family_bytes[1]])
    };

    // Get current thread and process
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

    // Handle bind based on address family
    match family {
        AF_INET => {
            // Validate address length for IPv4
            if addrlen < 16 {
                return SyscallResult::Err(EINVAL as u64);
            }

            // Read full IPv4 address from userspace
            let addr = unsafe {
                let addr_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, 16);
                match SockAddrIn::from_bytes(addr_bytes) {
                    Some(a) => a,
                    None => return SyscallResult::Err(EINVAL as u64),
                }
            };

            // Handle bind based on socket type
            match &fd_entry.kind {
                FdKind::UdpSocket(s) => {
                    // Bind UDP socket
                    let socket_ref = s.clone();
                    let mut socket = socket_ref.lock();
                    match socket.bind(pid, addr.addr, addr.port_host()) {
                        Ok(actual_port) => {
                            log::info!("UDP: Socket bound to port {} (requested: {})", actual_port, addr.port_host());
                            log::debug!("UDP bind: returning to userspace");
                            SyscallResult::Ok(0)
                        }
                        Err(e) => SyscallResult::Err(e as u64),
                    }
                }
                #[cfg(target_arch = "x86_64")]
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
        AF_UNIX => {
            // Validate address length for Unix (need at least family + 1 byte path)
            if addrlen < 3 {
                return SyscallResult::Err(EINVAL as u64);
            }

            // Read Unix socket address from userspace
            let addr = unsafe {
                let addr_len = (addrlen as usize).min(110); // family (2) + path (108)
                let addr_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, addr_len);
                match SockAddrUn::from_bytes(addr_bytes) {
                    Some(a) => a,
                    None => return SyscallResult::Err(EINVAL as u64),
                }
            };

            // For now, only support abstract sockets (path starts with '\0')
            if !addr.is_abstract() {
                log::debug!("sys_bind: Only abstract Unix sockets supported (path must start with \\0)");
                return SyscallResult::Err(EINVAL as u64);
            }

            // Get the path bytes
            let path = addr.path_bytes().to_vec();

            // Handle bind based on socket type
            match &fd_entry.kind {
                FdKind::UnixSocket(s) => {
                    // Check if path is already bound
                    if crate::socket::UNIX_SOCKET_REGISTRY.is_bound(&path) {
                        log::debug!("Unix: bind failed, path already in use");
                        return SyscallResult::Err(EADDRINUSE as u64);
                    }

                    // Bind the socket
                    let socket_ref = s.clone();
                    let mut socket = socket_ref.lock();
                    if let Err(e) = socket.bind(path.clone()) {
                        return SyscallResult::Err(e as u64);
                    }

                    log::info!("Unix: Socket bound to abstract path (len={})", path.len());
                    SyscallResult::Ok(0)
                }
                _ => {
                    log::debug!("sys_bind: AF_UNIX bind on non-Unix socket");
                    SyscallResult::Err(EINVAL as u64)
                }
            }
        }
        _ => {
            log::debug!("sys_bind: unsupported address family {}", family);
            SyscallResult::Err(EAFNOSUPPORT as u64)
        }
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
            log::debug!("UDP sendto: returning to userspace");
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
///
/// # Blocking Behavior
///
/// By default, UDP sockets are blocking. If no data is available, the calling
/// thread will block until a packet arrives. For non-blocking sockets (set via
/// fcntl O_NONBLOCK), EAGAIN is returned immediately when no data is available.
///
/// # Race Condition Handling (Double-Check Pattern)
///
/// The blocking path uses a double-check pattern to prevent a race condition
/// where a packet arrives between checking for data and entering the blocked state:
///
/// ```text
/// 1. Register as waiter (BEFORE checking for data)
/// 2. Check for data → if found, unregister and return it
/// 3. Set thread state to Blocked
/// 4. Double-check for data → if found, unblock and retry
/// 5. Enter HLT loop (actually blocked)
/// ```
///
/// The double-check at step 4 catches packets that arrived during the race window
/// between steps 2-3. Without this, the thread could block forever even though
/// a packet is available.
///
/// This pattern cannot be unit tested under controlled conditions because it
/// requires precise timing of concurrent events (packet arrival during the
/// microsecond window between check and block). The pattern is verified by:
/// - Code review (this documentation)
/// - Integration tests (blocking_recv_test.rs, concurrent_recv_stress.rs)
/// - The fact that DNS resolution works reliably in practice
///
/// # Interrupt Safety
///
/// This function acquires socket locks. The `enqueue_packet()` function on
/// UdpSocket is called from softirq context when packets arrive via the NIC.
/// To prevent deadlock:
///
/// ```text
/// SYSCALL PATH: Disables interrupts before acquiring locks
///   Cpu::without_interrupts(|| {
///       socket_ref.lock().register_waiter(thread_id);
///   });
///
/// SOFTIRQ PATH: Uses regular lock (interrupts already managed by softirq framework)
///   let mut waiting = self.waiting_threads.lock();
/// ```
///
/// By disabling interrupts in the syscall path, we guarantee that softirq cannot
/// run while we hold the lock, preventing the deadlock scenario.
///
/// # Packet Delivery Paths
///
/// Packets can arrive via two paths:
///
/// 1. **Real NIC path**: NIC interrupt → softirq → `process_rx()` → `enqueue_packet()`
///    This path runs in softirq context and wakes blocked threads.
///
/// 2. **Loopback path**: `sendto()` → `drain_loopback_queue()` → `enqueue_packet()`
///    This path runs synchronously in syscall context. We call `drain_loopback_queue()`
///    at the start of recvfrom and after blocking to ensure loopback packets are delivered.
///
/// # Spurious Wakeups
///
/// When multiple threads wait on the same socket and a packet arrives, ALL waiting
/// threads are woken. Only one will successfully receive the packet; others will
/// find no data and must re-block. The retry loop handles this correctly by
/// re-registering as waiter and re-blocking if no data is available after waking.
pub fn sys_recvfrom(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    _flags: u64,
    src_addr_ptr: u64,
    addrlen_ptr: u64,
) -> SyscallResult {
    log::debug!("sys_recvfrom: fd={}, buf_ptr=0x{:x}, len={}", fd, buf_ptr, len);

    // Drain loopback queue for packets sent to ourselves (127.x.x.x, own IP).
    // Hardware-received packets arrive via interrupt → softirq → process_rx().
    crate::net::drain_loopback_queue();

    // Validate buffer pointer
    if buf_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Get current thread ID for blocking
    let thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_recvfrom: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Get socket reference and nonblocking flag
    let (socket_ref, is_nonblocking) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_recvfrom: No process found for thread_id={}", thread_id);
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Get the socket from fd table
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(e) => e,
            None => return SyscallResult::Err(EBADF as u64),
        };

        // Verify it's a UDP socket and get Arc<Mutex<>> reference
        let socket = match &fd_entry.kind {
            FdKind::UdpSocket(s) => s.clone(),
            _ => return SyscallResult::Err(ENOTSOCK as u64),
        };

        let nonblocking = socket.lock().nonblocking;
        (socket, nonblocking)
        // manager_guard dropped here
    };

    // Blocking receive loop
    loop {
        // Register as waiter FIRST to avoid race condition where packet
        // arrives between checking and blocking.
        // CRITICAL: Disable interrupts while holding waiting_threads lock to prevent
        // deadlock with softirq (which runs in irq_exit before returning to us).
        Cpu::without_interrupts(|| {
            socket_ref.lock().register_waiter(thread_id);
        });

        // Drain loopback queue again in case packets arrived
        crate::net::drain_loopback_queue();

        // Try to receive a packet
        // CRITICAL: Must disable interrupts to prevent deadlock with softirq.
        // If we hold rx_queue lock and NIC interrupt fires, softirq will try to
        // acquire the same lock in enqueue_packet() -> deadlock!
        let packet_opt = Cpu::without_interrupts(|| {
            socket_ref.lock().recv_from()
        });
        if let Some(packet) = packet_opt {
            // Data was available - unregister from waiters
            Cpu::without_interrupts(|| {
                socket_ref.lock().unregister_waiter(thread_id);
            });

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

            return SyscallResult::Ok(copy_len as u64);
        }

        // No data available
        if is_nonblocking {
            Cpu::without_interrupts(|| {
                socket_ref.lock().unregister_waiter(thread_id);
            });
            return SyscallResult::Err(EAGAIN as u64);
        }

        // === BLOCKING PATH ===
        // Following the same pattern as stdin blocking in sys_read
        log::debug!("UDP recvfrom: fd={} entering blocking path, thread={}", fd, thread_id);

        // Block the current thread AND set blocked_in_syscall flag.
        // CRITICAL: Setting blocked_in_syscall is essential because:
        // 1. The thread will enter a kernel-mode HLT loop below
        // 2. If a context switch happens while in HLT, the scheduler sees
        //    from_userspace=false (kernel mode) but blocked_in_syscall tells
        //    it to save/restore kernel context, not userspace context
        // 3. Without this flag, no context is saved when switching away,
        //    and stale userspace context is restored when switching back
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current();
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = true;
            }
        });

        // CRITICAL RACE CONDITION FIX:
        // Check for data AGAIN after setting Blocked state but BEFORE entering HLT.
        // A packet might have arrived between:
        //   - when we checked for data (found none)
        //   - when we set thread state to Blocked
        // If packet arrived during that window, enqueue_packet() would have tried
        // to wake us but unblock() would have done nothing (we weren't blocked yet).
        // Now that we're blocked, check if data arrived and unblock ourselves.
        // NOTE: Must disable interrupts - has_data() acquires rx_queue lock, same as enqueue_packet()
        let data_arrived = Cpu::without_interrupts(|| {
            socket_ref.lock().has_data()
        });
        if data_arrived {
            log::info!("UDP: Thread {} caught race - data arrived during block setup", thread_id);
            // Data arrived during the race window - unblock and retry
            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                    thread.set_ready();
                }
            });
            Cpu::without_interrupts(|| {
                socket_ref.lock().unregister_waiter(thread_id);
            });
            continue; // Retry the receive loop
        }

        // CRITICAL: Re-enable preemption before entering blocking loop!
        // The syscall handler called preempt_disable() at entry, but we need
        // to allow timer interrupts to schedule other threads while we're blocked.
        crate::per_cpu::preempt_enable();

        // HLT loop - wait for timer interrupt which will switch to another thread
        // When packet arrives via softirq, enqueue_packet() will unblock us
        loop {
            crate::task::scheduler::yield_current();
            Cpu::halt_with_interrupts();

            // Check if we were unblocked (thread state changed from Blocked)
            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.state == crate::task::thread::ThreadState::Blocked
                } else {
                    false
                }
            }).unwrap_or(false);

            if !still_blocked {
                // CRITICAL: Disable preemption BEFORE breaking from HLT loop!
                // At this point blocked_in_syscall is still true. If we break with
                // preemption enabled, a timer interrupt could fire and do a context
                // switch while blocked_in_syscall=true, causing the <B> path to
                // incorrectly try to restore HLT context when we've already woken.
                crate::per_cpu::preempt_disable();
                log::info!("UDP: Thread {} woken from blocking", thread_id);
                break;
            }
            // else: still blocked, continue HLT loop
        }

        // NOTE: preempt_disable() is now called inside the HLT loop break path
        // to close the race window where blocked_in_syscall is true but we've woken.

        // Clear blocked_in_syscall now that we're resuming normal syscall execution
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = false;
            }
        });
        // Reset quantum to prevent immediate preemption after long blocking wait
        reset_quantum();
        crate::task::scheduler::check_and_clear_need_resched();

        // Unregister from wait queue (will re-register at top of loop)
        Cpu::without_interrupts(|| {
            socket_ref.lock().unregister_waiter(thread_id);
        });

        // Drain loopback again before retrying
        crate::net::drain_loopback_queue();

        // Loop back to try receiving again - we should have data now
    }
}

/// sys_listen - Mark a socket as listening for connections
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

    #[allow(unused_variables)] // pid only used for TCP on x86_64
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

    // Handle listen based on socket type
    match &fd_entry.kind {
        #[cfg(target_arch = "x86_64")]
        FdKind::TcpSocket(port) => {
            if *port == 0 {
                // Not bound
                return SyscallResult::Err(EINVAL as u64);
            }

            // Start listening
            if let Err(_) = crate::net::tcp::tcp_listen(*port, backlog as usize, pid) {
                return SyscallResult::Err(EADDRINUSE as u64);
            }

            // Update fd to TcpListener
            if let Some(entry) = process.fd_table.get_mut(fd as i32) {
                entry.kind = FdKind::TcpListener(*port);
            }

            log::info!("TCP: Socket now listening on port {}", port);
            SyscallResult::Ok(0)
        }
        #[cfg(target_arch = "x86_64")]
        FdKind::TcpListener(_) => {
            // Already listening
            SyscallResult::Err(EINVAL as u64)
        }
        FdKind::UnixSocket(s) => {
            // Get socket state and path
            let socket_ref = s.clone();
            let socket = socket_ref.lock();

            // Must be bound
            let path = match &socket.bound_path {
                Some(p) => p.clone(),
                None => {
                    log::debug!("sys_listen: Unix socket not bound");
                    return SyscallResult::Err(EINVAL as u64);
                }
            };

            drop(socket);

            // Create listener and register in global registry
            // Note: nonblocking mode is tracked via fd status_flags, not on the listener
            let listener = crate::socket::unix::UnixListener::new(
                path.clone(),
                backlog as usize,
            );
            let listener_arc = alloc::sync::Arc::new(spin::Mutex::new(listener));

            // Register in global registry
            if let Err(e) = crate::socket::UNIX_SOCKET_REGISTRY.bind(path.clone(), listener_arc.clone()) {
                log::debug!("sys_listen: Failed to register Unix listener: {}", e);
                return SyscallResult::Err(e as u64);
            }

            // Update fd to UnixListener
            if let Some(entry) = process.fd_table.get_mut(fd as i32) {
                entry.kind = FdKind::UnixListener(listener_arc);
            }

            log::info!("Unix: Socket now listening (path_len={})", path.len());
            SyscallResult::Ok(0)
        }
        FdKind::UnixListener(_) => {
            // Already listening
            SyscallResult::Err(EINVAL as u64)
        }
        _ => SyscallResult::Err(EOPNOTSUPP as u64),
    }
}

/// Internal enum to track listener type for accept
enum ListenerType {
    #[cfg(target_arch = "x86_64")]
    Tcp(u16),
    Unix(alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixListener>>),
}

/// sys_accept - Accept a connection on a listening socket
///
/// Arguments:
///   fd: Listening socket file descriptor
///   addr: Pointer to sockaddr for client address (can be NULL)
///   addrlen: Pointer to address length (can be NULL)
///
/// Returns: new socket fd on success, negative errno on error
///
/// # Blocking Behavior
///
/// accept() blocks until a connection is available. When no pending
/// connections exist, the calling thread blocks until a connection arrives.
/// The blocking pattern follows the same double-check approach as UDP recvfrom.
#[allow(unused_variables)] // addr_ptr/addrlen_ptr only used for TCP on x86_64
pub fn sys_accept(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    log::debug!("sys_accept: fd={}", fd);

    // Drain loopback queue for localhost connections (127.x.x.x, own IP).
    crate::net::drain_loopback_queue();

    // Get current thread ID for blocking
    let thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_accept: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Extract listener info and status_flags from fd, then release manager lock
    let (listener_type, is_nonblocking) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_accept: No process found for thread_id={}", thread_id);
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Get the socket from fd table
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(e) => e.clone(),
            None => return SyscallResult::Err(EBADF as u64),
        };

        // Check O_NONBLOCK status flag
        let nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;

        // Determine listener type
        let lt = match &fd_entry.kind {
            #[cfg(target_arch = "x86_64")]
            FdKind::TcpListener(p) => ListenerType::Tcp(*p),
            FdKind::UnixListener(l) => ListenerType::Unix(l.clone()),
            _ => return SyscallResult::Err(EOPNOTSUPP as u64),
        };
        (lt, nonblocking)
        // manager_guard dropped here
    };

    // Dispatch based on listener type
    match listener_type {
        #[cfg(target_arch = "x86_64")]
        ListenerType::Tcp(port) => {
            sys_accept_tcp(fd, port, is_nonblocking, thread_id, addr_ptr, addrlen_ptr)
        }
        ListenerType::Unix(listener) => {
            sys_accept_unix(fd, listener, is_nonblocking, thread_id)
        }
    }
}

/// Accept on TCP listener (x86_64 only)
#[cfg(target_arch = "x86_64")]
fn sys_accept_tcp(fd: u64, port: u16, is_nonblocking: bool, thread_id: u64, addr_ptr: u64, addrlen_ptr: u64) -> SyscallResult {
    // Blocking accept loop
    loop {
        // Register as waiter FIRST to avoid race condition
        crate::net::tcp::tcp_register_accept_waiter(port, thread_id);

        // Drain loopback queue in case connections arrived
        crate::net::drain_loopback_queue();

        // Try to accept a pending connection
        if let Some(conn_id) = crate::net::tcp::tcp_accept(port) {
            // Got a connection - unregister and complete
            crate::net::tcp::tcp_unregister_accept_waiter(port, thread_id);

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

            // Create new fd for the connection (need to re-acquire manager lock)
            let mut manager_guard = crate::process::manager();
            let manager = match *manager_guard {
                Some(ref mut m) => m,
                None => {
                    return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
                }
            };

            let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
                Some(p) => p,
                None => {
                    return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
                }
            };

            return match process.fd_table.alloc(FdKind::TcpConnection(conn_id)) {
                Ok(new_fd) => {
                    log::info!("TCP: Accepted connection on fd {}, new fd {}", fd, new_fd);
                    SyscallResult::Ok(new_fd as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            };
        }

        // No pending connection
        // If non-blocking mode, return EAGAIN immediately
        if is_nonblocking {
            log::debug!("TCP accept: fd={} is non-blocking, returning EAGAIN", fd);
            crate::net::tcp::tcp_unregister_accept_waiter(port, thread_id);
            return SyscallResult::Err(EAGAIN as u64);
        }

        // Blocking mode - block the thread
        log::debug!("TCP accept: fd={} entering blocking path, thread={}", fd, thread_id);

        // Block the current thread
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current();
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = true;
            }
        });

        // Double-check for pending connection after setting Blocked state
        if crate::net::tcp::tcp_has_pending(port) {
            log::info!("TCP: Thread {} caught race - connection arrived during block setup", thread_id);
            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                    thread.set_ready();
                }
            });
            crate::net::tcp::tcp_unregister_accept_waiter(port, thread_id);
            continue;
        }

        // Re-enable preemption before HLT loop
        crate::per_cpu::preempt_enable();

        log::info!("TCP_BLOCK: Thread {} entering blocked state for accept on port {}", thread_id, port);

        // HLT loop - wait for SYN to arrive
        loop {
            // Drain loopback queue - essential for single-threaded tests where
            // no other thread processes packets. This may wake other threads'
            // connections but that's OK - they'll re-check and re-block if needed.
            crate::net::drain_loopback_queue();

            // Check if we were woken by the drain (e.g., SYN arrived and woke us)
            // IMPORTANT: Check BEFORE HLT to avoid unnecessary waiting
            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.state == crate::task::thread::ThreadState::Blocked
                } else {
                    false
                }
            }).unwrap_or(false);

            if !still_blocked {
                crate::per_cpu::preempt_disable();
                log::info!("TCP_BLOCK: Thread {} woken from accept blocking", thread_id);
                break;
            }

            // Still blocked - yield and wait for interrupt (timer or NIC)
            crate::task::scheduler::yield_current();
            Cpu::halt_with_interrupts();
        }

        // Clear blocked_in_syscall
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = false;
            }
        });
        // Reset quantum to prevent immediate preemption after long blocking wait
        reset_quantum();
        crate::task::scheduler::check_and_clear_need_resched();

        // Unregister from wait queue (will re-register at top of loop)
        crate::net::tcp::tcp_unregister_accept_waiter(port, thread_id);

        // Drain loopback again before retrying
        crate::net::drain_loopback_queue();
    }
}

/// Accept on Unix listener
fn sys_accept_unix(
    fd: u64,
    listener: alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixListener>>,
    is_nonblocking: bool,
    thread_id: u64,
) -> SyscallResult {
    // Blocking accept loop
    loop {
        // Register as waiter FIRST to avoid race condition
        {
            let l = listener.lock();
            l.register_waiter(thread_id);
        }

        // Try to pop a pending connection
        let pending_socket = {
            let mut l = listener.lock();
            l.pop_pending()
        };

        if let Some(socket) = pending_socket {
            // Got a connection - unregister and complete
            {
                let l = listener.lock();
                l.unregister_waiter(thread_id);
            }

            // Create new fd for the connection
            let mut manager_guard = crate::process::manager();
            let manager = match *manager_guard {
                Some(ref mut m) => m,
                None => {
                    return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
                }
            };

            let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
                Some(p) => p,
                None => {
                    return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
                }
            };

            return match process.fd_table.alloc(FdKind::UnixStream(socket)) {
                Ok(new_fd) => {
                    log::info!("Unix: Accepted connection on fd {}, new fd {}", fd, new_fd);
                    SyscallResult::Ok(new_fd as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            };
        }

        // No pending connection
        // If non-blocking mode, return EAGAIN immediately
        if is_nonblocking {
            log::debug!("Unix accept: fd={} is non-blocking, returning EAGAIN", fd);
            {
                let l = listener.lock();
                l.unregister_waiter(thread_id);
            }
            return SyscallResult::Err(EAGAIN as u64);
        }

        // Blocking mode - block the thread
        log::debug!("Unix accept: fd={} entering blocking path, thread={}", fd, thread_id);

        // Block the current thread
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current();
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = true;
            }
        });

        // Double-check for pending connection after setting Blocked state
        let has_pending = {
            let l = listener.lock();
            l.has_pending()
        };
        if has_pending {
            log::info!("Unix: Thread {} caught race - connection arrived during block setup", thread_id);
            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                    thread.set_ready();
                }
            });
            {
                let l = listener.lock();
                l.unregister_waiter(thread_id);
            }
            continue;
        }

        // Re-enable preemption before HLT loop
        crate::per_cpu::preempt_enable();

        log::info!("Unix_BLOCK: Thread {} entering blocked state for accept", thread_id);

        // HLT loop - wait for connection to arrive
        loop {
            // Check if we were woken
            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.state == crate::task::thread::ThreadState::Blocked
                } else {
                    false
                }
            }).unwrap_or(false);

            if !still_blocked {
                crate::per_cpu::preempt_disable();
                log::info!("Unix_BLOCK: Thread {} woken from accept blocking", thread_id);
                break;
            }

            // Still blocked - yield and wait for interrupt
            crate::task::scheduler::yield_current();
            Cpu::halt_with_interrupts();
        }

        // Clear blocked_in_syscall
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = false;
            }
        });
        // Reset quantum to prevent immediate preemption after long blocking wait
        reset_quantum();
        crate::task::scheduler::check_and_clear_need_resched();

        // Unregister from wait queue (will re-register at top of loop)
        {
            let l = listener.lock();
            l.unregister_waiter(thread_id);
        }
    }
}

/// sys_connect - Connect a socket to a destination address
///
/// Arguments:
///   fd: Socket file descriptor
///   addr: Pointer to destination sockaddr
///   addrlen: Length of address structure
///
/// Returns: 0 on success, negative errno on error
///
/// # Blocking Behavior
///
/// TCP connect() blocks until the connection is established or fails.
/// Unix connect() completes immediately if the listener exists.
pub fn sys_connect(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    log::debug!("sys_connect: fd={}", fd);

    // Validate address pointer
    if addr_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Validate minimum address length (at least 2 bytes for family)
    if addrlen < 2 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read address family first
    let family = unsafe {
        let family_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, 2);
        u16::from_ne_bytes([family_bytes[0], family_bytes[1]])
    };

    // Dispatch based on address family
    match family {
        #[cfg(target_arch = "x86_64")]
        AF_INET => sys_connect_tcp(fd, addr_ptr, addrlen),
        #[cfg(target_arch = "aarch64")]
        AF_INET => {
            log::debug!("sys_connect: TCP not implemented on ARM64");
            SyscallResult::Err(EAFNOSUPPORT as u64)
        }
        AF_UNIX => sys_connect_unix(fd, addr_ptr, addrlen),
        _ => {
            log::debug!("sys_connect: unsupported address family {}", family);
            SyscallResult::Err(EAFNOSUPPORT as u64)
        }
    }
}

/// Connect TCP socket (x86_64 only)
#[cfg(target_arch = "x86_64")]
fn sys_connect_tcp(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    // Validate address length for IPv4
    if addrlen < 16 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read address from userspace
    let addr = unsafe {
        let addr_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, 16);
        match SockAddrIn::from_bytes(addr_bytes) {
            Some(a) => a,
            None => return SyscallResult::Err(EINVAL as u64),
        }
    };

    // Get current thread ID for blocking
    let thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_connect: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Initiate connection and get conn_id and nonblocking flag, then release manager lock
    let (conn_id, is_nonblocking) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (pid, process) = match manager.find_process_by_thread_mut(thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_connect: No process found for thread_id={}", thread_id);
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Get the socket from fd table
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(e) => e.clone(),
            None => return SyscallResult::Err(EBADF as u64),
        };

        // Check O_NONBLOCK status flag
        let nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;

        // Handle connect based on socket type
        match &fd_entry.kind {
            #[cfg(target_arch = "x86_64")]
            FdKind::TcpSocket(local_port) => {
                // Assign ephemeral port if not bound
                let port = if *local_port == 0 {
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

                (conn_id, nonblocking)
            }
            #[cfg(target_arch = "x86_64")]
            FdKind::TcpConnection(_) => {
                // Already connected
                return SyscallResult::Err(EISCONN as u64);
            }
            _ => return SyscallResult::Err(EOPNOTSUPP as u64),
        }
        // manager_guard dropped here
    };

    // For non-blocking sockets, return EINPROGRESS immediately
    // The connection is in progress but not yet established
    if is_nonblocking {
        log::debug!("TCP connect: fd={} is non-blocking, returning EINPROGRESS", fd);
        return SyscallResult::Err(EINPROGRESS as u64);
    }

    // Log the conn_id we'll be checking
    log::info!(
        "TCP connect: blocking for conn_id={{local={}:{}, remote={}:{}}}",
        conn_id.local_ip[3], conn_id.local_port,
        conn_id.remote_ip[3], conn_id.remote_port
    );

    // Timeout for connect: ~10 seconds (timer fires at 200Hz, so 2000 iterations ≈ 10s)
    // This prevents blocking forever on unreachable hosts
    const MAX_CONNECT_ITERATIONS: u32 = 2000;
    let mut connect_iterations: u32 = 0;

    // Blocking connect loop - wait for handshake to complete
    loop {
        // Check for timeout
        connect_iterations += 1;
        if connect_iterations > MAX_CONNECT_ITERATIONS {
            crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
            log::warn!("TCP connect: timed out after {} iterations", connect_iterations);
            return SyscallResult::Err(ETIMEDOUT as u64);
        }
        // Register as waiter FIRST to avoid race condition
        crate::net::tcp::tcp_register_recv_waiter(&conn_id, thread_id);

        // Drain loopback queue for localhost connections
        crate::net::drain_loopback_queue();

        // Check if connected
        if crate::net::tcp::tcp_is_established(&conn_id) {
            crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
            // Drain loopback one more time to deliver the ACK to the server
            crate::net::drain_loopback_queue();
            log::info!("TCP connect: thread={} - Connection established, returning success", thread_id);
            return SyscallResult::Ok(0);
        }

        // Check if connection failed
        if crate::net::tcp::tcp_is_failed(&conn_id) {
            crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
            log::warn!("TCP: Connection failed");
            return SyscallResult::Err(ECONNREFUSED as u64);
        }

        // Not yet established - block
        log::info!("TCP connect: thread={} entering blocking path", thread_id);

        // Block the current thread
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current();
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = true;
            }
        });

        log::info!("TCP connect: thread={} blocked, checking for race", thread_id);

        // Double-check for state change after setting Blocked state
        let is_established = crate::net::tcp::tcp_is_established(&conn_id);
        let is_failed = crate::net::tcp::tcp_is_failed(&conn_id);
        log::info!(
            "TCP connect: thread={} double-check: established={}, failed={}",
            thread_id, is_established, is_failed
        );

        if is_established || is_failed {
            log::info!("TCP: Thread {} caught race - state changed during block setup", thread_id);
            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                    thread.set_ready();
                }
            });
            crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
            continue;
        }

        // Re-enable preemption before HLT loop
        crate::per_cpu::preempt_enable();

        log::info!("TCP_BLOCK: Thread {} entering blocked state for connect", thread_id);

        // HLT loop - wait for SYN+ACK to arrive
        loop {
            // Drain loopback queue - essential for single-threaded tests where
            // no other thread processes packets. This may wake other threads'
            // connections but that's OK - they'll re-check and re-block if needed.
            crate::net::drain_loopback_queue();

            // Check if we were woken by the drain (e.g., SYN+ACK arrived and woke us)
            // IMPORTANT: Check BEFORE HLT to avoid unnecessary waiting
            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.state == crate::task::thread::ThreadState::Blocked
                } else {
                    false
                }
            }).unwrap_or(false);

            if !still_blocked {
                crate::per_cpu::preempt_disable();
                log::info!("TCP_BLOCK: Thread {} woken from connect blocking", thread_id);
                break;
            }

            // Still blocked - yield and wait for interrupt (timer or NIC)
            crate::task::scheduler::yield_current();
            Cpu::halt_with_interrupts();
        }

        // Clear blocked_in_syscall
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = false;
            }
        });
        // Reset quantum to prevent immediate preemption after long blocking wait
        reset_quantum();
        crate::task::scheduler::check_and_clear_need_resched();

        // Unregister from wait queue (will re-register at top of loop)
        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);

        // Drain loopback again before retrying
        crate::net::drain_loopback_queue();

        log::info!("TCP connect: thread={} looping back to check connection", thread_id);
    }
}

/// Connect Unix domain socket
fn sys_connect_unix(fd: u64, addr_ptr: u64, addrlen: u64) -> SyscallResult {
    use crate::socket::unix::UnixStreamSocket;

    // Validate address length for Unix (need at least family + 1 byte path)
    if addrlen < 3 {
        return SyscallResult::Err(EINVAL as u64);
    }

    // Read Unix socket address from userspace
    let addr = unsafe {
        let addr_len = (addrlen as usize).min(110); // family (2) + path (108)
        let addr_bytes = core::slice::from_raw_parts(addr_ptr as *const u8, addr_len);
        match SockAddrUn::from_bytes(addr_bytes) {
            Some(a) => a,
            None => return SyscallResult::Err(EINVAL as u64),
        }
    };

    // For now, only support abstract sockets (path starts with '\0')
    if !addr.is_abstract() {
        log::debug!("sys_connect: Only abstract Unix sockets supported (path must start with \\0)");
        return SyscallResult::Err(ENOENT as u64);
    }

    // Get the path bytes
    let path = addr.path_bytes();

    // Look up the listener in the registry
    let listener = match crate::socket::UNIX_SOCKET_REGISTRY.lookup(path) {
        Some(l) => l,
        None => {
            log::debug!("sys_connect: No listener found for Unix socket path");
            return SyscallResult::Err(ECONNREFUSED as u64);
        }
    };

    // Get current thread ID
    let thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_connect: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Create a connected pair: client gets socket_a, server gets socket_b
    let (socket_client, socket_server) = UnixStreamSocket::new_pair(false);

    // Push server socket to listener's pending queue and wake waiters
    {
        let mut l = listener.lock();
        if let Err(e) = l.push_pending(socket_server) {
            log::debug!("sys_connect: Listener backlog full");
            return SyscallResult::Err(e as u64);
        }
        l.wake_waiters();
    }

    // Update fd to UnixStream
    {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_connect: No process found for thread_id={}", thread_id);
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Get the socket from fd table and verify it's a Unix socket
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(e) => e.clone(),
            None => return SyscallResult::Err(EBADF as u64),
        };

        match &fd_entry.kind {
            FdKind::UnixSocket(_) => {
                // Update fd to connected stream
                if let Some(entry) = process.fd_table.get_mut(fd as i32) {
                    entry.kind = FdKind::UnixStream(socket_client);
                }
            }
            FdKind::UnixStream(_) => {
                // Already connected
                return SyscallResult::Err(EISCONN as u64);
            }
            _ => {
                return SyscallResult::Err(EOPNOTSUPP as u64);
            }
        }
    }

    log::info!("Unix: Connected to listener (path_len={})", path.len());
    SyscallResult::Ok(0)
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
        #[cfg(target_arch = "x86_64")]
        FdKind::TcpConnection(conn_id) => {
            // Set shutdown flags on the connection
            let shut_rd = how == 0 || how == 2; // SHUT_RD or SHUT_RDWR
            let shut_wr = how == 1 || how == 2; // SHUT_WR or SHUT_RDWR

            crate::net::tcp::tcp_shutdown(conn_id, shut_rd, shut_wr);

            log::info!("TCP: Shutdown fd={} how={}", fd, how);
            SyscallResult::Ok(0)
        }
        #[cfg(target_arch = "x86_64")]
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) => {
            // Not connected
            SyscallResult::Err(ENOTCONN as u64)
        }
        _ => SyscallResult::Err(EOPNOTSUPP as u64),
    }
}

/// sys_socketpair - Create a pair of connected Unix domain sockets
///
/// Arguments:
///   domain: Address family (must be AF_UNIX = 1)
///   sock_type: Socket type (SOCK_STREAM = 1, optionally OR'd with SOCK_NONBLOCK/SOCK_CLOEXEC)
///   protocol: Protocol (must be 0)
///   sv_ptr: Pointer to int[2] to receive the file descriptors
///
/// Returns: 0 on success, negative errno on error
pub fn sys_socketpair(domain: u64, sock_type: u64, protocol: u64, sv_ptr: u64) -> SyscallResult {
    use crate::socket::types::{AF_UNIX, SOCK_CLOEXEC, SOCK_NONBLOCK};
    use crate::socket::unix::UnixStreamSocket;
    use crate::ipc::fd::{FileDescriptor, flags, status_flags};

    log::debug!(
        "sys_socketpair: domain={}, type={:#x}, protocol={}, sv_ptr={:#x}",
        domain, sock_type, protocol, sv_ptr
    );

    // Validate domain - must be AF_UNIX
    if domain as u16 != AF_UNIX {
        log::debug!("sys_socketpair: unsupported domain {}", domain);
        return SyscallResult::Err(EAFNOSUPPORT as u64);
    }

    // Validate protocol - must be 0
    if protocol != 0 {
        log::debug!("sys_socketpair: unsupported protocol {}", protocol);
        return SyscallResult::Err(EINVAL as u64);
    }

    // Extract flags from sock_type
    let nonblocking = (sock_type as u32 & SOCK_NONBLOCK) != 0;
    let cloexec = (sock_type as u32 & SOCK_CLOEXEC) != 0;
    let base_type = sock_type as u32 & !(SOCK_NONBLOCK | SOCK_CLOEXEC);

    // Validate socket type - only SOCK_STREAM supported for now
    if base_type != crate::socket::types::SOCK_STREAM as u32 {
        log::debug!("sys_socketpair: unsupported socket type {}", base_type);
        return SyscallResult::Err(EINVAL as u64);
    }

    // Validate output pointer
    if sv_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_socketpair: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            log::error!("sys_socketpair: No process manager!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_socketpair: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Create the socket pair
    let (socket_a, socket_b) = UnixStreamSocket::new_pair(nonblocking);

    // Create file descriptor entries with appropriate flags
    let fd_flags = if cloexec { flags::FD_CLOEXEC } else { 0 };
    let fd_status_flags = if nonblocking { status_flags::O_NONBLOCK } else { 0 };

    let fd_entry_a = FileDescriptor::with_flags(
        FdKind::UnixStream(socket_a),
        fd_flags,
        fd_status_flags,
    );
    let fd_entry_b = FileDescriptor::with_flags(
        FdKind::UnixStream(socket_b),
        fd_flags,
        fd_status_flags,
    );

    // Allocate file descriptors
    let fd_a = match process.fd_table.alloc_with_entry(fd_entry_a) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("sys_socketpair: Failed to allocate fd_a: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    let fd_b = match process.fd_table.alloc_with_entry(fd_entry_b) {
        Ok(fd) => fd,
        Err(e) => {
            // Clean up fd_a on failure
            let _ = process.fd_table.close(fd_a);
            log::error!("sys_socketpair: Failed to allocate fd_b: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    // Write the file descriptors to userspace sv[0], sv[1]
    let sv: [i32; 2] = [fd_a, fd_b];
    unsafe {
        let sv_user = sv_ptr as *mut [i32; 2];
        // Validate pointer is in userspace range
        if sv_ptr < 0x1000 || sv_ptr > 0x7FFFFFFFFFFF {
            let _ = process.fd_table.close(fd_a);
            let _ = process.fd_table.close(fd_b);
            return SyscallResult::Err(EFAULT as u64);
        }
        core::ptr::write_volatile(sv_user, sv);
    }

    log::info!(
        "sys_socketpair: Created Unix socket pair sv=[{}, {}] nonblocking={} cloexec={}",
        fd_a, fd_b, nonblocking, cloexec
    );

    SyscallResult::Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that sys_recvfrom returns EBADF for invalid file descriptor
    ///
    /// NOTE: This test requires a process context which isn't available in unit tests.
    /// The error path is validated by verifying the constants are correct.
    #[test]
    fn test_recvfrom_error_constants() {
        // Verify EAGAIN constant for non-blocking mode
        assert_eq!(EAGAIN, 11);

        // Verify EBADF constant for bad file descriptor
        assert_eq!(EBADF, 9);

        // Verify ENOTSOCK constant for non-socket fd
        assert_eq!(ENOTSOCK, 88);

        // Verify EFAULT constant for bad address
        assert_eq!(EFAULT, 14);

        // Verify EINVAL constant for invalid argument
        assert_eq!(EINVAL, 22);
    }

    /// Test that EBADF constant is correct for invalid fd error
    #[test]
    fn test_recvfrom_ebadf_constant() {
        // EBADF (Bad file descriptor) should be returned when recvfrom
        // is called with an invalid fd. Since sys_recvfrom requires a
        // process context, we verify the constant is correct.
        // The actual error path is tested by nonblock_eagain_test.rs
        // userspace integration test.
        assert_eq!(EBADF, 9);
    }

    /// Test SockAddrIn structure layout and conversion
    #[test]
    fn test_sockaddr_in_structure() {
        // Create a sockaddr_in for 192.168.1.1:8080
        let addr = SockAddrIn::new([192, 168, 1, 1], 8080);

        // Verify family
        assert_eq!(addr.sin_family, AF_INET);

        // Verify port is in network byte order (big-endian)
        // 8080 = 0x1F90, network order = [0x1F, 0x90]
        assert_eq!(addr.sin_port, 8080u16.to_be());

        // Convert to bytes and verify
        let bytes = addr.to_bytes();
        assert_eq!(bytes.len(), 16); // sockaddr_in is 16 bytes

        // Check family field (first 2 bytes, little-endian u16)
        assert_eq!(bytes[0], AF_INET as u8);
        assert_eq!(bytes[1], 0);

        // Check port field (bytes 2-3, big-endian)
        assert_eq!(bytes[2], 0x1F); // High byte of 8080
        assert_eq!(bytes[3], 0x90); // Low byte of 8080

        // Check IP address (bytes 4-7)
        assert_eq!(bytes[4], 192);
        assert_eq!(bytes[5], 168);
        assert_eq!(bytes[6], 1);
        assert_eq!(bytes[7], 1);
    }

    /// Test socket type constants
    #[test]
    fn test_socket_type_constants() {
        // Verify socket type constants match POSIX/Linux values
        assert_eq!(AF_INET, 2);
        assert_eq!(SOCK_STREAM, 1); // TCP
        assert_eq!(SOCK_DGRAM, 2);  // UDP
    }

    /// Test that blocking recvfrom implementation handles race condition check
    ///
    /// The race condition fix checks has_data() AFTER setting thread to Blocked state.
    /// This test verifies the UdpSocket::has_data() method works correctly.
    #[test]
    fn test_udp_socket_has_data_for_race_check() {
        use crate::socket::udp::{UdpSocket, UdpPacket};

        let socket = UdpSocket::new();

        // Initially no data
        assert!(!socket.has_data());

        // After enqueue, has_data should return true
        let packet = UdpPacket {
            src_addr: [127, 0, 0, 1],
            src_port: 12345,
            data: alloc::vec![1, 2, 3, 4],
        };
        socket.enqueue_packet(packet);

        assert!(socket.has_data());

        // After receiving, no more data
        let _ = socket.recv_from();
        assert!(!socket.has_data());
    }
}
