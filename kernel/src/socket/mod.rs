//! Socket subsystem for Breenix
//!
//! Provides socket management and networking infrastructure.

pub mod types;
pub mod udp;

use spin::Mutex;

use crate::process::process::ProcessId;

/// Socket handle - unique identifier for a socket in the global registry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketHandle(u64);

impl SocketHandle {
    /// Create a new socket handle
    pub fn new(id: u64) -> Self {
        SocketHandle(id)
    }

    /// Get the raw ID (not yet used, but part of API)
    #[allow(dead_code)]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Counter for generating unique socket handles
static NEXT_SOCKET_HANDLE: Mutex<u64> = Mutex::new(0);

/// Allocate a new unique socket handle
pub fn alloc_socket_handle() -> SocketHandle {
    let mut next = NEXT_SOCKET_HANDLE.lock();
    let handle = SocketHandle::new(*next);
    *next += 1;
    handle
}

/// Global socket registry - maps ports to sockets for incoming packet dispatch
pub struct SocketRegistry {
    /// UDP port bindings: port -> (pid, socket_handle)
    udp_ports: spin::Mutex<alloc::collections::BTreeMap<u16, (ProcessId, SocketHandle)>>,
}

impl SocketRegistry {
    /// Create a new socket registry
    pub const fn new() -> Self {
        SocketRegistry {
            udp_ports: spin::Mutex::new(alloc::collections::BTreeMap::new()),
        }
    }

    /// Bind a UDP port to a socket
    pub fn bind_udp(&self, port: u16, pid: ProcessId, handle: SocketHandle) -> Result<(), i32> {
        let mut ports = self.udp_ports.lock();
        if ports.contains_key(&port) {
            Err(crate::syscall::errno::EADDRINUSE) // Address already in use
        } else {
            ports.insert(port, (pid, handle));
            Ok(())
        }
    }

    /// Unbind a UDP port
    pub fn unbind_udp(&self, port: u16) {
        self.udp_ports.lock().remove(&port);
    }

    /// Look up which socket owns a UDP port
    pub fn lookup_udp(&self, port: u16) -> Option<(ProcessId, SocketHandle)> {
        self.udp_ports.lock().get(&port).copied()
    }
}

/// Global socket registry instance
pub static SOCKET_REGISTRY: SocketRegistry = SocketRegistry::new();
