//! Socket subsystem for Breenix
//!
//! Provides socket management and networking infrastructure.

pub mod types;
pub mod udp;
pub mod unix;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
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

/// Ephemeral port range start (IANA recommendation)
const EPHEMERAL_PORT_START: u16 = 49152;
/// Ephemeral port range end
const EPHEMERAL_PORT_END: u16 = 65535;

/// Global socket registry - maps ports to sockets for incoming packet dispatch
pub struct SocketRegistry {
    /// UDP port bindings: port -> (pid, socket_handle)
    udp_ports: spin::Mutex<alloc::collections::BTreeMap<u16, (ProcessId, SocketHandle)>>,
    /// Next ephemeral port to try (simple rotating counter)
    next_ephemeral: spin::Mutex<u16>,
}

impl SocketRegistry {
    /// Create a new socket registry
    pub const fn new() -> Self {
        SocketRegistry {
            udp_ports: spin::Mutex::new(alloc::collections::BTreeMap::new()),
            next_ephemeral: spin::Mutex::new(EPHEMERAL_PORT_START),
        }
    }

    /// Allocate an ephemeral port
    fn alloc_ephemeral_port(&self, ports: &alloc::collections::BTreeMap<u16, (ProcessId, SocketHandle)>) -> Option<u16> {
        let mut next = self.next_ephemeral.lock();
        let start = *next;

        // Search for an available port, wrapping around if necessary
        loop {
            let port = *next;
            *next = if *next >= EPHEMERAL_PORT_END {
                EPHEMERAL_PORT_START
            } else {
                *next + 1
            };

            if !ports.contains_key(&port) {
                return Some(port);
            }

            // If we've wrapped around to the start, no ports available
            if *next == start {
                return None;
            }
        }
    }

    /// Bind a UDP port to a socket
    /// If port is 0, allocates an ephemeral port and returns it
    pub fn bind_udp(&self, port: u16, pid: ProcessId, handle: SocketHandle) -> Result<u16, i32> {
        let mut ports = self.udp_ports.lock();

        let actual_port = if port == 0 {
            // Allocate ephemeral port
            self.alloc_ephemeral_port(&ports)
                .ok_or(crate::syscall::errno::EADDRINUSE)?
        } else {
            // Use specified port
            if ports.contains_key(&port) {
                return Err(crate::syscall::errno::EADDRINUSE);
            }
            port
        };

        ports.insert(actual_port, (pid, handle));
        Ok(actual_port)
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

// ============================================================================
// Unix Domain Socket Registry
// ============================================================================

/// Registry for Unix domain socket listeners
///
/// Maps abstract paths to listeners so that connect() can find them.
/// This is the in-memory equivalent of the filesystem for abstract sockets.
pub struct UnixSocketRegistry {
    /// Map from path bytes to listener
    listeners: Mutex<BTreeMap<Vec<u8>, Arc<Mutex<unix::UnixListener>>>>,
}

impl UnixSocketRegistry {
    /// Create a new empty registry
    pub const fn new() -> Self {
        UnixSocketRegistry {
            listeners: Mutex::new(BTreeMap::new()),
        }
    }

    /// Register a listener at a path
    ///
    /// Returns EADDRINUSE if the path is already bound.
    pub fn bind(&self, path: Vec<u8>, listener: Arc<Mutex<unix::UnixListener>>) -> Result<(), i32> {
        let mut listeners = self.listeners.lock();
        if listeners.contains_key(&path) {
            return Err(crate::syscall::errno::EADDRINUSE);
        }
        listeners.insert(path, listener);
        Ok(())
    }

    /// Look up a listener by path
    pub fn lookup(&self, path: &[u8]) -> Option<Arc<Mutex<unix::UnixListener>>> {
        self.listeners.lock().get(path).cloned()
    }

    /// Remove a listener from the registry
    pub fn unbind(&self, path: &[u8]) {
        self.listeners.lock().remove(path);
    }

    /// Check if a path is bound
    pub fn is_bound(&self, path: &[u8]) -> bool {
        self.listeners.lock().contains_key(path)
    }
}

/// Global Unix socket registry instance
pub static UNIX_SOCKET_REGISTRY: UnixSocketRegistry = UnixSocketRegistry::new();
