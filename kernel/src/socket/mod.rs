//! Socket subsystem for Breenix
//!
//! Provides socket management and networking infrastructure.

pub mod addr;
pub mod object;
pub mod opts;
pub mod tcp;
pub mod types;
pub mod udp;
pub mod unix;
pub mod wait;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

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

pub use udp::{udp_ports_lookup_refused, SOCKET_REGISTRY};

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
