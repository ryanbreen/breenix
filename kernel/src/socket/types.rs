//! Socket address types
//!
//! POSIX-compatible socket address structures.

/// Address family: Unix (local)
pub const AF_UNIX: u16 = 1;

/// Address family: Unix (alias for AF_UNIX)
#[allow(dead_code)]
pub const AF_LOCAL: u16 = 1;

/// Address family: IPv4
pub const AF_INET: u16 = 2;

/// Socket type: Stream (TCP)
pub const SOCK_STREAM: u16 = 1;

/// Socket type: Datagram (UDP)
pub const SOCK_DGRAM: u16 = 2;

/// Socket flag: Non-blocking mode
pub const SOCK_NONBLOCK: u32 = 0x800;

/// Socket flag: Close-on-exec
pub const SOCK_CLOEXEC: u32 = 0x80000;

/// IPv4 socket address structure (matches Linux sockaddr_in)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SockAddrIn {
    /// Address family (AF_INET = 2)
    pub family: u16,
    /// Port number (network byte order - big endian)
    pub port: u16,
    /// IPv4 address
    pub addr: [u8; 4],
    /// Padding to match sockaddr size
    pub zero: [u8; 8],
}

impl SockAddrIn {
    /// Create a new socket address
    pub fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            family: AF_INET,
            port: port.to_be(), // Convert to network byte order
            addr,
            zero: [0; 8],
        }
    }

    /// Get the port in host byte order
    pub fn port_host(&self) -> u16 {
        u16::from_be(self.port)
    }

    /// Create from raw bytes (for parsing from userspace)
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }

        Some(SockAddrIn {
            family: u16::from_ne_bytes([bytes[0], bytes[1]]),
            port: u16::from_ne_bytes([bytes[2], bytes[3]]),
            addr: [bytes[4], bytes[5], bytes[6], bytes[7]],
            zero: [
                bytes[8], bytes[9], bytes[10], bytes[11],
                bytes[12], bytes[13], bytes[14], bytes[15],
            ],
        })
    }

    /// Convert to bytes (for writing to userspace)
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        let family_bytes = self.family.to_ne_bytes();
        let port_bytes = self.port.to_ne_bytes();

        bytes[0] = family_bytes[0];
        bytes[1] = family_bytes[1];
        bytes[2] = port_bytes[0];
        bytes[3] = port_bytes[1];
        bytes[4..8].copy_from_slice(&self.addr);
        bytes[8..16].copy_from_slice(&self.zero);

        bytes
    }
}

impl Default for SockAddrIn {
    fn default() -> Self {
        SockAddrIn {
            family: AF_INET,
            port: 0,
            addr: [0; 4],
            zero: [0; 8],
        }
    }
}

/// Unix domain socket address structure (matches Linux sockaddr_un)
#[repr(C)]
#[derive(Clone)]
pub struct SockAddrUn {
    /// Address family (AF_UNIX = 1)
    pub family: u16,
    /// Socket path (null-terminated, up to 108 bytes)
    /// For abstract sockets, path[0] is '\0' and the name follows
    pub path: [u8; 108],
}

impl SockAddrUn {
    /// Maximum path length (excluding null terminator for normal paths)
    pub const PATH_MAX: usize = 108;

    /// Create a new Unix socket address from a filesystem path
    ///
    /// For filesystem-based sockets (not yet implemented - only abstract sockets supported).
    #[allow(dead_code)] // Public API for future filesystem socket support
    pub fn new(path_str: &[u8]) -> Self {
        let mut addr = SockAddrUn {
            family: AF_UNIX,
            path: [0; 108],
        };
        let copy_len = path_str.len().min(Self::PATH_MAX);
        addr.path[..copy_len].copy_from_slice(&path_str[..copy_len]);
        addr
    }

    /// Create from raw bytes (for parsing from userspace)
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        // Minimum is 2 bytes for family + at least 1 byte of path
        if bytes.len() < 3 {
            return None;
        }

        let family = u16::from_ne_bytes([bytes[0], bytes[1]]);
        if family != AF_UNIX {
            return None;
        }

        let mut addr = SockAddrUn {
            family,
            path: [0; 108],
        };

        let path_len = (bytes.len() - 2).min(Self::PATH_MAX);
        addr.path[..path_len].copy_from_slice(&bytes[2..2 + path_len]);

        Some(addr)
    }

    /// Check if this is an abstract socket (path starts with '\0')
    pub fn is_abstract(&self) -> bool {
        self.path[0] == 0 && self.path_len() > 0
    }

    /// Get the effective path length (excluding trailing nulls for regular paths)
    pub fn path_len(&self) -> usize {
        if self.path[0] == 0 {
            // Abstract socket: find first non-null after initial null,
            // then find end of name
            for i in 1..Self::PATH_MAX {
                if self.path[i] == 0 {
                    return i;
                }
            }
            Self::PATH_MAX
        } else {
            // Regular path: find null terminator
            for i in 0..Self::PATH_MAX {
                if self.path[i] == 0 {
                    return i;
                }
            }
            Self::PATH_MAX
        }
    }

    /// Get the path as bytes (for abstract sockets, includes leading '\0')
    pub fn path_bytes(&self) -> &[u8] {
        &self.path[..self.path_len()]
    }
}

impl Default for SockAddrUn {
    fn default() -> Self {
        SockAddrUn {
            family: AF_UNIX,
            path: [0; 108],
        }
    }
}

impl core::fmt::Debug for SockAddrUn {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_abstract() {
            write!(f, "SockAddrUn(abstract: {:?})", &self.path[1..self.path_len()])
        } else {
            write!(f, "SockAddrUn(path: {:?})", &self.path[..self.path_len()])
        }
    }
}
