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
    #[allow(dead_code)] // Public API for future fcntl O_NONBLOCK support
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
/// Unit tests here validate local UDP socket behavior but cannot exercise the
/// syscall-level blocking path (requires multi-threaded execution). We
/// simulate blocked threads by manually setting thread state; integration
/// tests via telnetd and DNS in Docker provide full path coverage.
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

    pub fn test_register_waiter_duplicate_prevention() {
        log::info!("=== TEST: register_waiter prevents duplicates ===");

        let socket = UdpSocket::new();
        let thread_id = 42;

        socket.register_waiter(thread_id);
        socket.register_waiter(thread_id);

        let waiting = socket.waiting_threads.lock();
        assert_eq!(waiting.len(), 1);

        log::info!("=== TEST PASSED: register_waiter prevents duplicates ===");
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

    pub fn test_unregister_nonexistent_waiter() {
        log::info!("=== TEST: unregister_waiter ignores missing thread ID ===");

        let socket = UdpSocket::new();

        socket.unregister_waiter(999);

        let waiting = socket.waiting_threads.lock();
        assert_eq!(waiting.len(), 0);

        log::info!("=== TEST PASSED: unregister_waiter ignores missing thread ID ===");
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

    pub fn test_blocking_recvfrom_blocks_and_wakes() {
        log::info!("=== TEST: blocking recvfrom blocks and wakes ===");

        let socket = UdpSocket::new();
        let idle_thread = make_thread(1, ThreadState::Ready);
        scheduler::init(idle_thread);

        // This test simulates blocking by manually marking a thread as blocked.
        // True syscall-level blocking needs multi-threaded execution and is
        // covered by telnetd and DNS integration tests in Docker. This test
        // verifies the WAKE path, not the BLOCK path.
        let blocked_thread_id = 2;
        let blocked_thread = make_thread(blocked_thread_id, ThreadState::Blocked);
        let added = scheduler::with_scheduler(|sched| {
            sched.add_thread(blocked_thread);
            if let Some(thread) = sched.get_thread_mut(blocked_thread_id) {
                thread.state = ThreadState::Blocked;
                thread.blocked_in_syscall = true;
            }
            sched.remove_from_ready_queue(blocked_thread_id);
        });
        assert_eq!(added.is_some(), true);

        socket.register_waiter(blocked_thread_id);

        let state_blocked = scheduler::with_scheduler(|sched| {
            sched.get_thread(blocked_thread_id).map(|thread| {
                thread.state == ThreadState::Blocked && thread.blocked_in_syscall
            })
        });
        assert_eq!(state_blocked, Some(Some(true)));

        let packet = UdpPacket {
            src_addr: [10, 0, 2, 15],
            src_port: 4321,
            data: alloc::vec![0xaa],
        };
        socket.enqueue_packet(packet);

        assert_eq!(socket.waiting_threads.lock().is_empty(), true);

        let state_ready = scheduler::with_scheduler(|sched| {
            sched.get_thread(blocked_thread_id).map(|thread| thread.state == ThreadState::Ready)
        });
        assert_eq!(state_ready, Some(Some(true)));

        log::info!("=== TEST PASSED: blocking recvfrom blocks and wakes ===");
    }

    /// Test that multiple blocked threads are all woken when packet arrives
    pub fn test_enqueue_packet_wakes_multiple_waiters() {
        log::info!("=== TEST: enqueue_packet wakes multiple waiters ===");

        let socket = UdpSocket::new();
        let idle_thread = make_thread(1, ThreadState::Ready);
        scheduler::init(idle_thread);

        // Create multiple blocked threads
        let thread_ids = [10u64, 11, 12];
        for &tid in &thread_ids {
            let thread = make_thread(tid, ThreadState::Blocked);
            scheduler::with_scheduler(|sched| {
                sched.add_thread(thread);
                if let Some(t) = sched.get_thread_mut(tid) {
                    t.state = ThreadState::Blocked;
                }
                sched.remove_from_ready_queue(tid);
            });
            socket.register_waiter(tid);
        }

        // Verify all are registered
        assert_eq!(socket.waiting_threads.lock().len(), 3);

        // Enqueue packet - should wake all
        let packet = UdpPacket {
            src_addr: [10, 0, 2, 15],
            src_port: 4321,
            data: alloc::vec![0xaa, 0xbb],
        };
        socket.enqueue_packet(packet);

        // Verify wait queue is empty (all unregistered during wake)
        assert_eq!(socket.waiting_threads.lock().is_empty(), true);

        // Verify all threads are now Ready
        for &tid in &thread_ids {
            let is_ready = scheduler::with_scheduler(|sched| {
                sched.get_thread(tid).map(|t| t.state == ThreadState::Ready)
            });
            assert_eq!(is_ready, Some(Some(true)), "Thread {} should be Ready", tid);
        }

        log::info!("=== TEST PASSED: enqueue_packet wakes multiple waiters ===");
    }

    /// Test nonblocking mode flag behavior
    pub fn test_nonblocking_mode_flag() {
        log::info!("=== TEST: nonblocking mode flag ===");

        let mut socket = UdpSocket::new();

        // Default should be blocking
        assert_eq!(socket.nonblocking, false);

        // Set to nonblocking
        socket.set_nonblocking(true);
        assert_eq!(socket.nonblocking, true);
        assert!(socket.recv_from().is_none());

        // Set back to blocking
        socket.set_nonblocking(false);
        assert_eq!(socket.nonblocking, false);

        log::info!("=== TEST PASSED: nonblocking mode flag ===");
    }

    /// Test recv_from properly dequeues packets
    pub fn test_recv_from_dequeues_packets() {
        log::info!("=== TEST: recv_from dequeues packets in FIFO order ===");

        let socket = UdpSocket::new();

        // Enqueue multiple packets
        for i in 0..3u8 {
            let packet = UdpPacket {
                src_addr: [192, 168, 1, i],
                src_port: 1000 + i as u16,
                data: alloc::vec![i, i + 1],
            };
            socket.enqueue_packet(packet);
        }

        assert_eq!(socket.rx_queue.lock().len(), 3);

        // Receive first packet
        let pkt1 = socket.recv_from();
        assert!(pkt1.is_some());
        let pkt1 = pkt1.unwrap();
        assert_eq!(pkt1.src_addr, [192, 168, 1, 0]);
        assert_eq!(pkt1.src_port, 1000);
        assert_eq!(pkt1.data, alloc::vec![0, 1]);

        // Receive second packet
        let pkt2 = socket.recv_from();
        assert!(pkt2.is_some());
        let pkt2 = pkt2.unwrap();
        assert_eq!(pkt2.src_addr, [192, 168, 1, 1]);
        assert_eq!(pkt2.src_port, 1001);

        // Receive third packet
        let pkt3 = socket.recv_from();
        assert!(pkt3.is_some());
        assert_eq!(pkt3.unwrap().src_addr, [192, 168, 1, 2]);

        // Queue should be empty now
        assert!(!socket.has_data());
        assert!(socket.recv_from().is_none());

        log::info!("=== TEST PASSED: recv_from dequeues packets in FIFO order ===");
    }

    /// Test has_data() reflects receive queue state.
    ///
    /// This checks has_data() tracks queue state for the blocking recvfrom path.
    /// True concurrent race testing would require multi-threaded execution, which
    /// is not feasible in kernel unit tests. The actual race protection relies on
    /// sys_recvfrom implementing the double-check pattern correctly.
    pub fn test_has_data_reflects_queue_state() {
        log::info!("=== TEST: has_data reflects queue state ===");

        let socket = UdpSocket::new();

        // Simulate the blocking path: first check shows no data
        assert!(!socket.has_data());

        // Simulate: we would set thread to Blocked here (in actual syscall)
        // Then a packet arrives during the race window
        let packet = UdpPacket {
            src_addr: [127, 0, 0, 1],
            src_port: 53,
            data: alloc::vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        socket.enqueue_packet(packet);

        // The double-check: has_data() should now return true
        // This is the check that prevents the race condition
        assert!(socket.has_data());

        // The fix: if has_data() returns true in double-check,
        // thread unblocks itself and retries receive
        let received = socket.recv_from();
        assert!(received.is_some());
        assert_eq!(received.unwrap().data, alloc::vec![0xDE, 0xAD, 0xBE, 0xEF]);

        log::info!("=== TEST PASSED: has_data reflects queue state ===");
    }

    /// Test RX queue overflow behavior (drops oldest packet)
    pub fn test_rx_queue_overflow() {
        log::info!("=== TEST: RX queue overflow drops oldest ===");

        let socket = UdpSocket::new();

        // Fill queue to MAX_RX_QUEUE_SIZE (32)
        for i in 0..MAX_RX_QUEUE_SIZE {
            let i = i as u8;
            let packet = UdpPacket {
                src_addr: [10, 0, 0, i],
                src_port: i as u16,
                data: alloc::vec![i],
            };
            socket.enqueue_packet(packet);
        }

        assert_eq!(socket.rx_queue.lock().len(), MAX_RX_QUEUE_SIZE);

        // Add one more - should drop oldest (i=0)
        let packet = UdpPacket {
            src_addr: [10, 0, 0, 99],
            src_port: 99,
            data: alloc::vec![99],
        };
        socket.enqueue_packet(packet);

        // Still MAX_RX_QUEUE_SIZE packets
        assert_eq!(socket.rx_queue.lock().len(), MAX_RX_QUEUE_SIZE);

        // First packet should now be i=1 (i=0 was dropped)
        let first = socket.recv_from().unwrap();
        assert_eq!(first.src_addr, [10, 0, 0, 1]);
        assert_eq!(first.data, alloc::vec![1]);

        log::info!("=== TEST PASSED: RX queue overflow drops oldest ===");
    }
}
