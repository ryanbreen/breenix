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
    /// Non-blocking mode flag (not yet used)
    pub _nonblocking: bool,
}

impl core::fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UdpSocket")
            .field("handle", &self.handle)
            .field("local_addr", &self.local_addr)
            .field("local_port", &self.local_port)
            .field("bound", &self.bound)
            .field("_nonblocking", &self._nonblocking)
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
            _nonblocking: true, // Start non-blocking for simplicity
        }
    }

    /// Bind the socket to a local address and port
    pub fn bind(&mut self, pid: ProcessId, addr: [u8; 4], port: u16) -> Result<(), i32> {
        if self.bound {
            return Err(crate::syscall::errno::EINVAL); // Already bound
        }

        // Register in global socket registry
        SOCKET_REGISTRY.bind_udp(port, pid, self.handle)?;

        self.local_addr = Some(addr);
        self.local_port = Some(port);
        self.bound = true;

        log::debug!(
            "UDP: Socket {:?} bound to {}.{}.{}.{}:{}",
            self.handle,
            addr[0], addr[1], addr[2], addr[3],
            port
        );

        Ok(())
    }

    /// Receive a packet from the queue
    pub fn recv_from(&mut self) -> Option<UdpPacket> {
        self.rx_queue.lock().pop_front()
    }

    /// Enqueue a received packet (called from interrupt context)
    pub fn enqueue_packet(&self, packet: UdpPacket) {
        let mut queue = self.rx_queue.lock();
        // Drop oldest if queue is full
        if queue.len() >= MAX_RX_QUEUE_SIZE {
            queue.pop_front();
            log::warn!("UDP: RX queue full, dropped oldest packet");
        }
        queue.push_back(packet);
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
