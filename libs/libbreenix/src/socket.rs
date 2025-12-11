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

/// Address family: IPv4
pub const AF_INET: i32 = 2;

/// Socket type: Datagram (UDP)
pub const SOCK_DGRAM: i32 = 2;

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

/// Bind a socket to a local address
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
