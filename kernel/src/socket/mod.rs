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
