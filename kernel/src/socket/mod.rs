//! Socket subsystem for Breenix
//!
//! Provides file descriptor infrastructure and socket management.

pub mod types;
pub mod udp;

use alloc::boxed::Box;
use spin::Mutex;

use crate::process::process::ProcessId;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 64;

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

/// Types of file descriptors
#[derive(Debug)]
pub enum FdKind {
    /// Standard input
    Stdin,
    /// Standard output
    Stdout,
    /// Standard error
    Stderr,
    /// UDP socket
    UdpSocket(Box<udp::UdpSocket>),
}

/// File descriptor entry
#[derive(Debug)]
pub struct FileDescriptor {
    /// Type of this file descriptor
    pub kind: FdKind,
    /// Flags (O_NONBLOCK, etc.) - part of API, not yet used
    pub _flags: u32,
}

impl FileDescriptor {
    /// Create a new file descriptor
    pub fn new(kind: FdKind, flags: u32) -> Self {
        FileDescriptor { kind, _flags: flags }
    }
}

/// File descriptor table for a process
pub struct FdTable {
    /// Fixed-size table of file descriptors
    table: [Option<FileDescriptor>; MAX_FDS],
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

impl FdTable {
    /// Create a new FD table with stdin/stdout/stderr pre-allocated
    pub fn new() -> Self {
        // Initialize with all None
        const NONE: Option<FileDescriptor> = None;
        let mut table = [NONE; MAX_FDS];

        // Pre-allocate standard file descriptors
        table[0] = Some(FileDescriptor::new(FdKind::Stdin, 0));
        table[1] = Some(FileDescriptor::new(FdKind::Stdout, 0));
        table[2] = Some(FileDescriptor::new(FdKind::Stderr, 0));

        FdTable { table }
    }

    /// Allocate a new file descriptor, returning its number
    pub fn alloc(&mut self, fd: FileDescriptor) -> Option<u32> {
        // Find first free slot (starting from 3 to skip stdin/stdout/stderr)
        for i in 3..MAX_FDS {
            if self.table[i].is_none() {
                self.table[i] = Some(fd);
                return Some(i as u32);
            }
        }
        None // No free slots
    }

    /// Get a reference to a file descriptor
    pub fn get(&self, fd: u32) -> Option<&FileDescriptor> {
        if (fd as usize) < MAX_FDS {
            self.table[fd as usize].as_ref()
        } else {
            None
        }
    }

    /// Get a mutable reference to a file descriptor
    pub fn get_mut(&mut self, fd: u32) -> Option<&mut FileDescriptor> {
        if (fd as usize) < MAX_FDS {
            self.table[fd as usize].as_mut()
        } else {
            None
        }
    }

    /// Close a file descriptor (part of API, not yet used)
    #[allow(dead_code)]
    pub fn close(&mut self, fd: u32) -> Result<(), i32> {
        if (fd as usize) < MAX_FDS {
            if self.table[fd as usize].is_some() {
                self.table[fd as usize] = None;
                Ok(())
            } else {
                Err(crate::syscall::errno::EBADF) // Bad file descriptor
            }
        } else {
            Err(crate::syscall::errno::EBADF)
        }
    }
}

impl core::fmt::Debug for FdTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let open_count = self.table.iter().filter(|fd| fd.is_some()).count();
        f.debug_struct("FdTable")
            .field("open_fds", &open_count)
            .finish()
    }
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
