//! UDP socket implementation
//!
//! Provides datagram socket functionality for UDP protocol.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use super::types::SockAddrIn;
use super::{alloc_socket_handle, SocketHandle, SOCKET_REGISTRY};
use crate::process::process::ProcessId;

/// Maximum number of packets to queue per socket
const MAX_RX_QUEUE_SIZE: usize = 32;

/// A received UDP packet
#[derive(Debug)]
pub struct UdpPacket {
    /// Source IP address
    pub src_addr: [u8; 4],
    /// Source port
    pub src_port: u16,
    /// Packet payload data
    pub data: Vec<u8>,
}

/// UDP socket state
pub struct UdpSocket {
    /// Unique handle for this socket
    pub handle: SocketHandle,
    /// Local address (if bound)
    pub local_addr: Option<[u8; 4]>,
    /// Local port (if bound)
    pub local_port: Option<u16>,
    /// Whether the socket is bound
    pub bound: bool,
    /// Receive queue for incoming packets (protected for interrupt-safe access)
    pub rx_queue: Mutex<VecDeque<UdpPacket>>,
    /// Thread IDs blocked waiting for data on this socket
    pub waiting_threads: Mutex<Vec<u64>>,
    /// Non-blocking mode flag (default false = blocking)
    pub nonblocking: bool,
}

impl core::fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UdpSocket")
            .field("handle", &self.handle)
            .field("local_addr", &self.local_addr)
            .field("local_port", &self.local_port)
            .field("bound", &self.bound)
            .field("nonblocking", &self.nonblocking)
            .finish()
    }
}

impl UdpSocket {
    /// Create a new UDP socket
    pub fn new() -> Self {
        UdpSocket {
            handle: alloc_socket_handle(),
            local_addr: None,
            local_port: None,
            bound: false,
            rx_queue: Mutex::new(VecDeque::new()),
            waiting_threads: Mutex::new(Vec::new()),
            nonblocking: false, // Default to blocking (POSIX standard)
        }
    }

    /// Bind the socket to a local address and port
    /// If port is 0, an ephemeral port will be allocated
    pub fn bind(&mut self, pid: ProcessId, addr: [u8; 4], port: u16) -> Result<u16, i32> {
        if self.bound {
            return Err(crate::syscall::errno::EINVAL); // Already bound
        }

        // Register in global socket registry (returns allocated port if port was 0)
        let actual_port = SOCKET_REGISTRY.bind_udp(port, pid, self.handle)?;

        self.local_addr = Some(addr);
        self.local_port = Some(actual_port);
        self.bound = true;

        log::debug!(
            "UDP: Socket {:?} bound to {}.{}.{}.{}:{} (requested: {})",
            self.handle,
            addr[0], addr[1], addr[2], addr[3],
            actual_port,
            port
        );

        Ok(actual_port)
    }

    /// Receive a packet from the queue
    pub fn recv_from(&mut self) -> Option<UdpPacket> {
        self.rx_queue.lock().pop_front()
    }

    /// Enqueue a received packet (called from softirq context)
    ///
    /// After enqueuing, wakes all blocked threads so they can receive the packet.
    ///
    /// CRITICAL: This is called from softirq context which runs in irq_exit() BEFORE
    /// returning to the preempted code. If syscall code holds waiting_threads lock
    /// and we try to acquire it here, we would deadlock. The syscall code MUST
    /// disable interrupts while holding waiting_threads lock to prevent this.
    pub fn enqueue_packet(&self, packet: UdpPacket) {
        // Enqueue the packet
        {
            let mut queue = self.rx_queue.lock();
            // Drop oldest if queue is full
            if queue.len() >= MAX_RX_QUEUE_SIZE {
                queue.pop_front();
                log::warn!("UDP: RX queue full, dropped oldest packet");
            }
            queue.push_back(packet);
        }

        // Wake ALL blocked threads (they'll race to receive)
        // We MUST wake threads reliably - use regular lock, not try_lock.
        // Softirq context is safe for blocking since interrupts are enabled.
        let readers: alloc::vec::Vec<u64> = {
            let mut waiting = self.waiting_threads.lock();
            waiting.drain(..).collect()
        };

        if !readers.is_empty() {
            crate::task::scheduler::with_scheduler(|sched| {
                for thread_id in &readers {
                    sched.unblock(*thread_id);
                }
            });
            // Trigger reschedule so woken threads run soon
            crate::task::scheduler::set_need_resched();
        }
    }

    /// Check if there are packets available to receive (part of API)
    #[allow(dead_code)]
    pub fn has_data(&self) -> bool {
        !self.rx_queue.lock().is_empty()
    }

    /// Get the socket's local address (part of API)
    #[allow(dead_code)]
    pub fn local_addr(&self) -> Option<SockAddrIn> {
        if let (Some(addr), Some(port)) = (self.local_addr, self.local_port) {
            Some(SockAddrIn::new(addr, port))
        } else {
            None
        }
    }

    /// Get the local port (if bound)
    pub fn local_port(&self) -> Option<u16> {
        self.local_port
    }

    /// Register a thread as waiting for data on this socket
    pub fn register_waiter(&self, thread_id: u64) {
        let mut waiting = self.waiting_threads.lock();
        if !waiting.contains(&thread_id) {
            waiting.push(thread_id);
            log::trace!("UDP: Thread {} registered as waiter", thread_id);
        }
    }

    /// Unregister a thread from waiting for data
    pub fn unregister_waiter(&self, thread_id: u64) {
        let mut waiting = self.waiting_threads.lock();
        waiting.retain(|&id| id != thread_id);
    }

    /// Set non-blocking mode for this socket
    pub fn set_nonblocking(&mut self, nonblocking: bool) {
        self.nonblocking = nonblocking;
    }
}

impl Default for UdpSocket {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        // Unbind from registry when socket is dropped
        if let Some(port) = self.local_port {
            SOCKET_REGISTRY.unbind_udp(port);
            log::debug!("UDP: Socket {:?} unbound from port {}", self.handle, port);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::string::String;
    use crate::task::scheduler;
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use x86_64::VirtAddr;

    fn dummy_entry() {}

    fn make_thread(id: u64, state: ThreadState) -> Box<Thread> {
        let mut thread = Thread::new_with_id(
            id,
            String::from("udp-test-thread"),
            dummy_entry,
            VirtAddr::new(0x1000),
            VirtAddr::new(0x800),
            VirtAddr::new(0),
            ThreadPrivilege::Kernel,
        );
        thread.state = state;
        Box::new(thread)
    }

    pub fn test_register_waiter_adds_thread() {
        log::info!("=== TEST: register_waiter adds thread ID ===");

        let socket = UdpSocket::new();
        let thread_id = 42;

        socket.register_waiter(thread_id);

        let waiting = socket.waiting_threads.lock();
        assert_eq!(waiting.contains(&thread_id), true);
        assert_eq!(waiting.len(), 1);

        log::info!("=== TEST PASSED: register_waiter adds thread ID ===");
    }

    pub fn test_unregister_waiter_removes_thread() {
        log::info!("=== TEST: unregister_waiter removes thread ID ===");

        let socket = UdpSocket::new();
        let thread_id = 42;
        let other_thread_id = 7;

        socket.register_waiter(thread_id);
        socket.register_waiter(other_thread_id);
        socket.unregister_waiter(thread_id);

        let waiting = socket.waiting_threads.lock();
        assert_eq!(waiting.contains(&thread_id), false);
        assert_eq!(waiting.contains(&other_thread_id), true);
        assert_eq!(waiting.len(), 1);

        log::info!("=== TEST PASSED: unregister_waiter removes thread ID ===");
    }

    pub fn test_has_data_after_enqueue_packet() {
        log::info!("=== TEST: has_data reflects enqueue_packet ===");

        let socket = UdpSocket::new();
        assert_eq!(socket.has_data(), false);

        let packet = UdpPacket {
            src_addr: [127, 0, 0, 1],
            src_port: 1234,
            data: alloc::vec![1, 2, 3],
        };
        socket.enqueue_packet(packet);

        assert_eq!(socket.has_data(), true);

        log::info!("=== TEST PASSED: has_data reflects enqueue_packet ===");
    }

    pub fn test_enqueue_packet_wakes_blocked_threads() {
        log::info!("=== TEST: enqueue_packet wakes blocked threads ===");

        let socket = UdpSocket::new();
        let idle_thread = make_thread(1, ThreadState::Ready);
        scheduler::init(idle_thread);

        let blocked_thread_id = 2;
        let blocked_thread = make_thread(blocked_thread_id, ThreadState::Blocked);
        let added = scheduler::with_scheduler(|sched| {
            sched.add_thread(blocked_thread);
            if let Some(thread) = sched.get_thread_mut(blocked_thread_id) {
                thread.state = ThreadState::Blocked;
            }
            sched.remove_from_ready_queue(blocked_thread_id);
        });
        assert_eq!(added.is_some(), true);

        socket.register_waiter(blocked_thread_id);

        let packet = UdpPacket {
            src_addr: [10, 0, 2, 15],
            src_port: 4321,
            data: alloc::vec![0xaa],
        };
        socket.enqueue_packet(packet);

        assert_eq!(socket.waiting_threads.lock().is_empty(), true);

        let state_ready = scheduler::with_scheduler(|sched| {
            sched
                .get_thread(blocked_thread_id)
                .map(|thread| thread.state == ThreadState::Ready)
                .unwrap_or(false)
        });
        assert_eq!(state_ready, Some(true));

        log::info!("=== TEST PASSED: enqueue_packet wakes blocked threads ===");
    }
}
