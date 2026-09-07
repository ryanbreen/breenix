//! Socket subsystem for Breenix
//!
//! Provides socket management and networking infrastructure.

pub mod types;
pub mod udp;
pub mod unix;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
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

/// #908: NetRx-route try-lock refusals on the separate UDP port registry.
/// A relaxed diagnostic counter, not a synchronization point.
static UDP_PORTS_LOOKUP_REFUSED: AtomicU64 = AtomicU64::new(0);

/// Read the refusal count for the boot oracle and future diagnostics.
pub fn udp_ports_lookup_refused() -> u64 {
    UDP_PORTS_LOOKUP_REFUSED.load(Ordering::Relaxed)
}

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
    fn alloc_ephemeral_port(
        &self,
        ports: &alloc::collections::BTreeMap<u16, (ProcessId, SocketHandle)>,
    ) -> Option<u16> {
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

    /// Acquire `udp_ports` with interrupts masked for the whole hold.
    ///
    /// #908: mirrors socket/udp.rs::with_locked_masked for this separate
    /// inner registry lock. Its private map is only acquired here and by
    /// try_lookup_udp. Masking at this boundary covers bind_udp and unbind_udp,
    /// including UdpSocket::Drop from close_extracted_fds outside the PM lock.
    /// The bounded ephemeral scan, nested next_ephemeral lock and allocating
    /// map insertion run inside this mask.
    /// `unbind_udp`'s map removal can also allocate/deallocate --
    /// `BTreeMap::remove` may rebalance or merge nodes back to the
    /// allocator. Neither is a deadlock risk: the global heap allocator
    /// (`kernel/src/memory/heap.rs`) masks interrupts around its own lock
    /// during alloc/dealloc, the same nested-mask pattern this
    /// primitive already relies on.
    ///
    /// The NetRx IRQ route uses try_lookup_udp instead: CLAUDE.md requires
    /// try-lock/defer in interrupt context rather than waiting on a peer's
    /// masked hold. The boot oracle calls this production primitive directly.
    pub(crate) fn with_udp_ports_masked<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut alloc::collections::BTreeMap<u16, (ProcessId, SocketHandle)>) -> R,
    {
        #[cfg(target_arch = "x86_64")]
        type Cpu = crate::arch_impl::x86_64::X86Cpu;
        #[cfg(target_arch = "aarch64")]
        type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;
        use crate::arch_impl::traits::CpuOps;

        Cpu::without_interrupts(|| f(&mut self.udp_ports.lock()))
    }

    /// Bind a UDP port; port 0 allocates an ephemeral port.
    /// #908: the whole registry hold runs under with_udp_ports_masked.
    pub fn bind_udp(&self, port: u16, pid: ProcessId, handle: SocketHandle) -> Result<u16, i32> {
        self.with_udp_ports_masked(|ports| {
            let actual_port = if port == 0 {
                // Allocate ephemeral port
                self.alloc_ephemeral_port(ports)
                    .ok_or(crate::syscall::errno::EADDRINUSE)?
            } else {
                if ports.contains_key(&port) {
                    return Err(crate::syscall::errno::EADDRINUSE);
                }
                port
            };

            ports.insert(actual_port, (pid, handle));
            Ok(actual_port)
        })
    }

    /// Unbind a UDP port.
    /// #908: UdpSocket::Drop reaches this through both masked Process::terminate
    /// and unmasked close_extracted_fds; masking here protects both callers.
    pub fn unbind_udp(&self, port: u16) {
        self.with_udp_ports_masked(|ports| {
            ports.remove(&port);
        });
    }

    /// IRQ-safe lookup: refuse a contended thread-side registry hold.
    /// #908: handle_udp calls this before deliver_to_socket enters the PM mask.
    /// Contention drops the best-effort UDP datagram and increments a relaxed
    /// diagnostic counter. None means contention; Some(None) means no binding;
    /// Some(Some(..)) identifies the bound socket.
    /// claim-lint:ok: #908 specifies these 3 Option return cases.
    pub fn try_lookup_udp(&self, port: u16) -> Option<Option<(ProcessId, SocketHandle)>> {
        match self.udp_ports.try_lock() {
            Some(guard) => Some(guard.get(&port).copied()),
            None => {
                UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
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
