//! Socket system call wrappers for Breenix
//!
//! Provides userspace API for UDP sockets.
//!
//! # Example
//!
//! ```rust,ignore
//! use libbreenix::socket::{socket, bind, sendto, SockAddrIn, AF_INET, SOCK_DGRAM};
//!
//! // Create UDP socket
//! let fd = socket(AF_INET, SOCK_DGRAM, 0).expect("socket failed");
//!
//! // Bind to port 12345
//! let addr = SockAddrIn::new([0, 0, 0, 0], 12345);
//! bind(fd, &addr).expect("bind failed");
//!
//! // Send data
//! let dest = SockAddrIn::new([10, 0, 2, 2], 1234);
//! sendto(fd, b"Hello UDP!", &dest).expect("sendto failed");
//! ```

use crate::syscall::{nr, raw};

/// Address family: Unix (local)
pub const AF_UNIX: i32 = 1;

/// Address family: Unix (alias)
pub const AF_LOCAL: i32 = 1;

/// Address family: IPv4
pub const AF_INET: i32 = 2;

/// Socket type: Stream (TCP)
pub const SOCK_STREAM: i32 = 1;

/// Socket type: Datagram (UDP)
pub const SOCK_DGRAM: i32 = 2;

/// Socket flag: Non-blocking
pub const SOCK_NONBLOCK: i32 = 0x800;

/// Socket flag: Close-on-exec
pub const SOCK_CLOEXEC: i32 = 0x80000;

/// Shutdown how: Stop receiving
pub const SHUT_RD: i32 = 0;

/// Shutdown how: Stop sending
pub const SHUT_WR: i32 = 1;

/// Shutdown how: Stop both
pub const SHUT_RDWR: i32 = 2;

/// IPv4 socket address structure (matches kernel sockaddr_in)
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

/// Unix domain socket address structure (matches kernel sockaddr_un)
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
    /// Maximum path length
    pub const PATH_MAX: usize = 108;

    /// Create a new abstract Unix socket address
    ///
    /// Abstract sockets start with '\0' followed by the name.
    /// They don't appear in the filesystem and are automatically
    /// cleaned up when the last reference is closed.
    pub fn abstract_socket(name: &[u8]) -> Self {
        let mut addr = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0; 108],
        };
        // path[0] = 0 for abstract socket
        let copy_len = name.len().min(Self::PATH_MAX - 1);
        addr.path[1..1 + copy_len].copy_from_slice(&name[..copy_len]);
        addr
    }

    /// Create a new Unix socket address from a path
    ///
    /// For filesystem-based sockets (not currently supported).
    pub fn new(path: &[u8]) -> Self {
        let mut addr = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0; 108],
        };
        let copy_len = path.len().min(Self::PATH_MAX);
        addr.path[..copy_len].copy_from_slice(&path[..copy_len]);
        addr
    }

    /// Get the effective length of this address structure for bind/connect
    ///
    /// For abstract sockets, includes family (2) + null byte (1) + name length
    pub fn len(&self) -> usize {
        if self.path[0] == 0 {
            // Abstract socket: find end of name after the leading null
            for i in 1..Self::PATH_MAX {
                if self.path[i] == 0 {
                    return 2 + i; // family (2) + path including leading null
                }
            }
            2 + Self::PATH_MAX
        } else {
            // Regular path: find null terminator
            for i in 0..Self::PATH_MAX {
                if self.path[i] == 0 {
                    return 2 + i + 1; // family (2) + path + null
                }
            }
            2 + Self::PATH_MAX
        }
    }
}

impl Default for SockAddrUn {
    fn default() -> Self {
        SockAddrUn {
            family: AF_UNIX as u16,
            path: [0; 108],
        }
    }
}

impl SockAddrIn {
    /// Create a new socket address
    ///
    /// Port is automatically converted to network byte order.
    pub fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            family: AF_INET as u16,
            port: port.to_be(), // Convert to network byte order
            addr,
            zero: [0; 8],
        }
    }

    /// Get the port in host byte order
    pub fn port_host(&self) -> u16 {
        u16::from_be(self.port)
    }
}

impl Default for SockAddrIn {
    fn default() -> Self {
        SockAddrIn {
            family: AF_INET as u16,
            port: 0,
            addr: [0; 4],
            zero: [0; 8],
        }
    }
}

/// Convert host to network byte order (16-bit)
#[inline]
pub fn htons(x: u16) -> u16 {
    x.to_be()
}

/// Convert network to host byte order (16-bit)
#[inline]
pub fn ntohs(x: u16) -> u16 {
    u16::from_be(x)
}

/// Convert host to network byte order (32-bit)
#[inline]
pub fn htonl(x: u32) -> u32 {
    x.to_be()
}

/// Convert network to host byte order (32-bit)
#[inline]
pub fn ntohl(x: u32) -> u32 {
    u32::from_be(x)
}

/// Create a socket
///
/// # Arguments
/// * `domain` - Address family (AF_INET for IPv4)
/// * `sock_type` - Socket type (SOCK_DGRAM for UDP)
/// * `protocol` - Protocol (0 for default)
///
/// # Returns
/// File descriptor on success, or negative errno on error
pub fn socket(domain: i32, sock_type: i32, protocol: i32) -> Result<i32, i32> {
    let ret = unsafe {
        raw::syscall3(nr::SOCKET, domain as u64, sock_type as u64, protocol as u64)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(ret as i32)
    }
}

/// Bind a socket to a local IPv4 address
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `addr` - Local address to bind to
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn bind(fd: i32, addr: &SockAddrIn) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall3(
            nr::BIND,
            fd as u64,
            addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Bind a Unix domain socket to an address
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `addr` - Unix socket address to bind to
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn bind_unix(fd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall3(
            nr::BIND,
            fd as u64,
            addr as *const SockAddrUn as u64,
            addr.len() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Send data to a destination address
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `buf` - Data to send
/// * `dest_addr` - Destination address
///
/// # Returns
/// Number of bytes sent on success, or negative errno on error
pub fn sendto(fd: i32, buf: &[u8], dest_addr: &SockAddrIn) -> Result<usize, i32> {
    let ret = unsafe {
        raw::syscall6(
            nr::SENDTO,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
            0, // flags
            dest_addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(ret as usize)
    }
}

/// Receive data from a socket
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `buf` - Buffer to receive into
/// * `src_addr` - Optional buffer to receive source address
///
/// # Returns
/// Number of bytes received on success, or negative errno on error
pub fn recvfrom(fd: i32, buf: &mut [u8], src_addr: Option<&mut SockAddrIn>) -> Result<usize, i32> {
    let (addr_ptr, addrlen_ptr) = match src_addr {
        Some(addr) => {
            // We need a mutable length variable
            static mut ADDRLEN: u32 = core::mem::size_of::<SockAddrIn>() as u32;
            unsafe {
                ADDRLEN = core::mem::size_of::<SockAddrIn>() as u32;
                (
                    addr as *mut SockAddrIn as u64,
                    &raw mut ADDRLEN as *mut u32 as u64,
                )
            }
        }
        None => (0u64, 0u64),
    };

    let ret = unsafe {
        raw::syscall6(
            nr::RECVFROM,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0, // flags
            addr_ptr,
            addrlen_ptr,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(ret as usize)
    }
}

/// Connect a socket to a remote address (TCP)
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `addr` - Remote address to connect to
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn connect(fd: i32, addr: &SockAddrIn) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall3(
            nr::CONNECT,
            fd as u64,
            addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Connect a Unix domain socket to a server
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `addr` - Unix socket address to connect to
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn connect_unix(fd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall3(
            nr::CONNECT,
            fd as u64,
            addr as *const SockAddrUn as u64,
            addr.len() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Mark a socket as listening for connections (TCP)
///
/// # Arguments
/// * `fd` - Socket file descriptor (must be bound)
/// * `backlog` - Maximum pending connections (usually 128)
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn listen(fd: i32, backlog: i32) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall2(nr::LISTEN, fd as u64, backlog as u64)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Accept a connection on a listening socket (TCP)
///
/// # Arguments
/// * `fd` - Listening socket file descriptor
/// * `addr` - Optional buffer to receive client address
///
/// # Returns
/// New socket file descriptor for the connection on success, or negative errno on error
pub fn accept(fd: i32, addr: Option<&mut SockAddrIn>) -> Result<i32, i32> {
    let (addr_ptr, addrlen_ptr) = match addr {
        Some(a) => {
            static mut ADDRLEN: u32 = core::mem::size_of::<SockAddrIn>() as u32;
            unsafe {
                ADDRLEN = core::mem::size_of::<SockAddrIn>() as u32;
                (
                    a as *mut SockAddrIn as u64,
                    &raw mut ADDRLEN as *mut u32 as u64,
                )
            }
        }
        None => (0u64, 0u64),
    };

    let ret = unsafe {
        raw::syscall3(nr::ACCEPT, fd as u64, addr_ptr, addrlen_ptr)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(ret as i32)
    }
}

/// Shutdown a socket connection (TCP)
///
/// # Arguments
/// * `fd` - Socket file descriptor
/// * `how` - SHUT_RD (stop receiving), SHUT_WR (stop sending), or SHUT_RDWR (both)
///
/// # Returns
/// 0 on success, or negative errno on error
pub fn shutdown(fd: i32, how: i32) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall2(nr::SHUTDOWN, fd as u64, how as u64)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Create a pair of connected Unix domain sockets
///
/// # Arguments
/// * `domain` - Address family (must be AF_UNIX)
/// * `sock_type` - Socket type (SOCK_STREAM, optionally OR'd with SOCK_NONBLOCK, SOCK_CLOEXEC)
/// * `protocol` - Protocol (must be 0)
///
/// # Returns
/// Tuple of two file descriptors (sv[0], sv[1]) on success, or negative errno on error
pub fn socketpair(domain: i32, sock_type: i32, protocol: i32) -> Result<(i32, i32), i32> {
    let mut sv: [i32; 2] = [0, 0];
    let ret = unsafe {
        raw::syscall4(
            nr::SOCKETPAIR,
            domain as u64,
            sock_type as u64,
            protocol as u64,
            sv.as_mut_ptr() as u64,
        )
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok((sv[0], sv[1]))
    }
}
