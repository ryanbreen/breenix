//! Socket address types
//!
//! POSIX-compatible socket address structures.

/// Address family: IPv4
pub const AF_INET: u16 = 2;

/// Socket type: Datagram (UDP)
pub const SOCK_DGRAM: u16 = 2;

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
