//! Linux socket address ABI, copied through the fault-tolerant userptr boundary.

use crate::syscall::errno::{EAFNOSUPPORT, EINVAL, ENOMEM};
use crate::syscall::userptr::{copy_from_user, copy_to_user, read_user_bytes, write_user_prefix};
use alloc::vec::Vec;

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
const UNIX_LEN: usize = 110;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SockAddr {
    Unspec,
    UnspecInet { addr: [u8; 4], port: u16 },
    Inet { addr: [u8; 4], port: u16 },
    Unix(UnixName),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnixName {
    Unnamed,
    Path(Vec<u8>),
    Abstract(Vec<u8>),
}

fn owned_name(bytes: &[u8]) -> Result<Vec<u8>, i32> {
    let mut name = Vec::new();
    name.try_reserve_exact(bytes.len()).map_err(|_| ENOMEM)?;
    name.extend_from_slice(bytes);
    Ok(name)
}

fn decode(bytes: &[u8], family: u16) -> Result<SockAddr, i32> {
    if bytes.len() < 2 {
        return Err(EINVAL);
    }
    let supplied = u16::from_ne_bytes([bytes[0], bytes[1]]);
    match family {
        AF_INET => {
            if supplied == 0 && bytes.len() < 16 {
                return Ok(SockAddr::Unspec);
            }
            if bytes.len() < 16 {
                return Err(EINVAL);
            }
            match supplied {
                0 => Ok(SockAddr::UnspecInet {
                    addr: [bytes[4], bytes[5], bytes[6], bytes[7]],
                    port: u16::from_be_bytes([bytes[2], bytes[3]]),
                }),
                AF_INET => Ok(SockAddr::Inet {
                    addr: [bytes[4], bytes[5], bytes[6], bytes[7]],
                    port: u16::from_be_bytes([bytes[2], bytes[3]]),
                }),
                _ => Err(EAFNOSUPPORT),
            }
        }
        AF_UNIX => {
            if bytes.len() > UNIX_LEN {
                return Err(EINVAL);
            }
            if supplied == 0 {
                return Ok(SockAddr::Unspec);
            }
            if supplied != AF_UNIX {
                return Err(EINVAL);
            }
            let path = &bytes[2..];
            Ok(SockAddr::Unix(if path.is_empty() {
                UnixName::Unnamed
            } else if path[0] == 0 {
                // Abstract names are length-delimited, including embedded/trailing NULs.
                UnixName::Abstract(owned_name(&path[1..])?)
            } else {
                let n = path.iter().position(|b| *b == 0).unwrap_or(path.len());
                UnixName::Path(owned_name(&path[..n])?)
            }))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

/// Read a socket address for `family`; no user address is dereferenced directly.
pub fn read_sockaddr(ptr: u64, len: u32, family: u16) -> Result<SockAddr, i32> {
    if !(2..=128).contains(&len) {
        return Err(EINVAL);
    }
    let count = len as usize;
    let mut bytes = [0u8; 128];
    read_user_bytes(bytes.as_mut_ptr(), ptr, count).map_err(|e| e as i32)?;
    decode(&bytes[..count], family)
}

fn encode(addr: &SockAddr) -> Result<([u8; UNIX_LEN + 1], usize), i32> {
    let mut bytes = [0u8; UNIX_LEN + 1];
    let family = if matches!(addr, SockAddr::UnspecInet { .. }) {
        0u16
    } else {
        AF_INET
    };
    let len = match addr {
        SockAddr::Unspec => 2,
        SockAddr::Inet { addr, port } | SockAddr::UnspecInet { addr, port } => {
            bytes[..2].copy_from_slice(&family.to_ne_bytes());
            bytes[2..4].copy_from_slice(&port.to_be_bytes());
            bytes[4..8].copy_from_slice(addr);
            16
        }
        SockAddr::Unix(name) => {
            bytes[..2].copy_from_slice(&AF_UNIX.to_ne_bytes());
            match name {
                UnixName::Unnamed => 2,
                UnixName::Path(path) => {
                    if path.len() > 108 || path.contains(&0) {
                        return Err(EINVAL);
                    }
                    bytes[2..2 + path.len()].copy_from_slice(path);
                    // Linux includes the terminator even for a full 108-byte input path.
                    3 + path.len()
                }
                UnixName::Abstract(name) => {
                    if name.len() > 107 {
                        return Err(EINVAL);
                    }
                    bytes[3..3 + name.len()].copy_from_slice(name);
                    3 + name.len()
                }
            }
        }
    };
    Ok((bytes, len))
}

/// Write the caller's available prefix, then the complete address length.
pub fn write_sockaddr(addr: &SockAddr, ptr: u64, len_ptr: u64) -> Result<(), i32> {
    let available = copy_from_user(len_ptr as *const i32).map_err(|e| e as i32)?;
    if available < 0 {
        return Err(EINVAL);
    }
    let (bytes, real) = encode(addr)?;
    let count = (available as usize).min(real);
    if count != 0 && write_user_prefix(ptr, bytes.as_ptr(), count) != count {
        return Err(crate::syscall::errno::EFAULT);
    }
    copy_to_user(len_ptr as *mut u32, &(real as u32)).map_err(|e| e as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inet_order_family_and_length() {
        let addr = SockAddr::Inet {
            addr: [192, 0, 2, 7],
            port: 8080,
        };
        let (mut bytes, n) = encode(&addr).unwrap();
        assert_eq!(n, 16);
        assert_eq!(&bytes[2..4], &[0x1f, 0x90]);
        assert_eq!(decode(&bytes[..n], AF_INET), Ok(addr));
        assert_eq!(decode(&bytes[..15], AF_INET), Err(EINVAL));
        bytes[..2].copy_from_slice(&AF_UNIX.to_ne_bytes());
        assert_eq!(decode(&bytes[..n], AF_INET), Err(EAFNOSUPPORT));
        bytes[..2].fill(0);
        assert_eq!(
            decode(&bytes[..n], AF_INET),
            Ok(SockAddr::UnspecInet {
                addr: [192, 0, 2, 7],
                port: 8080
            })
        );
    }
    #[test]
    fn unix_names_preserve_abstract_length() {
        for name in [
            UnixName::Unnamed,
            UnixName::Path(b"/tmp/socket".to_vec()),
            UnixName::Abstract(b"a\0b\0".to_vec()),
        ] {
            let addr = SockAddr::Unix(name);
            let (bytes, n) = encode(&addr).unwrap();
            assert_eq!(decode(&bytes[..n], AF_UNIX), Ok(addr));
        }
        assert_ne!(
            decode(&[1, 0, 0, b'a'], AF_UNIX),
            decode(&[1, 0, 0, b'a', 0], AF_UNIX)
        );
        assert_eq!(decode(&[0; 111], AF_UNIX), Err(EINVAL));
    }
}
