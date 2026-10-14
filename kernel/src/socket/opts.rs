//! Socket options: the SOL_SOCKET options every protocol shares,
//! TCP_NODELAY, and IP_TTL for IPv4 sockets.
//!
//! The values and errors follow Linux, which fixes what POSIX leaves open: a
//! buffer size is stored doubled and read back doubled, a timeout is a 16-byte
//! struct timeval whose microseconds must be below a million (EDOM), an int
//! option shorter than 4 bytes is EINVAL, and an option no layer knows is
//! ENOPROTOOPT. An option this module does not know at SOL_SOCKET, and any
//! other level, goes to the protocol (SO_PEERCRED is AF_UNIX's). AF_UNIX has
//! no protocol options, so any level but SOL_SOCKET is EOPNOTSUPP there.

use super::object::{OptBuf, Socket};
use crate::net::ipv4::DEFAULT_TTL;
use crate::syscall::errno::{EDOM, EINVAL, ENOPROTOOPT, EOPNOTSUPP};

pub const SOL_SOCKET: i32 = 1;
pub const SO_REUSEADDR: i32 = 2;
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_BROADCAST: i32 = 6;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVBUF: i32 = 8;
pub const SO_KEEPALIVE: i32 = 9;
pub const SO_LINGER: i32 = 13;
pub const SO_PEERCRED: i32 = 17;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_ACCEPTCONN: i32 = 30;
pub const SO_PROTOCOL: i32 = 38;
pub const SO_DOMAIN: i32 = 39;

pub const IPPROTO_IP: i32 = 0;
pub const IP_TTL: i32 = 2;

pub const IPPROTO_TCP: i32 = 6;
pub const TCP_NODELAY: i32 = 1;

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
const SOCK_STREAM: u16 = 1;

/// Linux's net.core.rmem_max and wmem_max: the largest size a caller may ask
/// for before doubling.
const BUF_MAX: usize = 212_992;
/// Linux's SOCK_MIN_RCVBUF and SOCK_MIN_SNDBUF on a 64-bit kernel.
const RCVBUF_MIN: usize = 2_304;
const SNDBUF_MIN: usize = 4_608;
/// Linux's net.core.rmem_default and wmem_default.
const BUF_DEFAULT: usize = 212_992;

const NS_PER_SEC: u64 = 1_000_000_000;
const US_PER_SEC: i64 = 1_000_000;

/// The options a socket has set.
#[derive(Clone)]
pub struct SockOpts {
    pub reuseaddr: bool,
    pub broadcast: bool,
    pub keepalive: bool,
    pub nodelay: bool,
    /// Some(seconds) when SO_LINGER is on.
    pub linger: Option<u32>,
    pub rcvbuf: usize,
    pub sndbuf: usize,
    /// None means no timeout; Some(0) fails at once, as Linux treats a
    /// negative tv_sec.
    pub rcvtimeo_ns: Option<u64>,
    pub sndtimeo_ns: Option<u64>,
    /// IP_TTL: the time to live of the IPv4 packets the socket sends.
    pub ttl: u8,
}

impl SockOpts {
    pub const fn new() -> Self {
        SockOpts {
            reuseaddr: false,
            broadcast: false,
            keepalive: false,
            nodelay: false,
            linger: None,
            rcvbuf: BUF_DEFAULT,
            sndbuf: BUF_DEFAULT,
            rcvtimeo_ns: None,
            sndtimeo_ns: None,
            ttl: DEFAULT_TTL,
        }
    }
}

impl Default for SockOpts {
    fn default() -> Self {
        Self::new()
    }
}

fn is_tcp(sk: &Socket) -> bool {
    sk.family == AF_INET && sk.kind == SOCK_STREAM
}

/// Linux's AF_UNIX sockets have no protocol options: every level but
/// SOL_SOCKET is EOPNOTSUPP for them, to setsockopt and getsockopt alike.
fn unix_protocol_level(sk: &Socket, level: i32) -> bool {
    sk.family == AF_UNIX && level != SOL_SOCKET
}

/// IP_TTL's value. As Linux's do_ip_setsockopt reads it: an int when optlen
/// holds one, otherwise a single unsigned byte, and EINVAL for no bytes. -1
/// restores the default; anything else outside 1..=255 is EINVAL.
fn ttl_value(val: &[u8]) -> Result<u8, i32> {
    let value = match val.len() {
        0 => return Err(EINVAL),
        1..=3 => i32::from(val[0]),
        _ => int_value(val)?,
    };
    match value {
        -1 => Ok(DEFAULT_TTL),
        1..=255 => Ok(value as u8),
        _ => Err(EINVAL),
    }
}

fn int_value(val: &[u8]) -> Result<i32, i32> {
    if val.len() < 4 {
        return Err(EINVAL);
    }
    Ok(i32::from_ne_bytes([val[0], val[1], val[2], val[3]]))
}

/// A size given to SO_RCVBUF or SO_SNDBUF: Linux reads it as unsigned, caps
/// it, and stores twice that, at least `min`.
fn doubled(value: i32, min: usize) -> usize {
    ((value as u32 as usize).min(BUF_MAX) * 2).max(min)
}

/// SO_RCVTIMEO and SO_SNDTIMEO: a struct timeval of two 64-bit fields.
fn timeout_value(val: &[u8]) -> Result<Option<u64>, i32> {
    if val.len() < 16 {
        return Err(EINVAL);
    }
    let sec = i64::from_ne_bytes(val[0..8].try_into().map_err(|_| EINVAL)?);
    let usec = i64::from_ne_bytes(val[8..16].try_into().map_err(|_| EINVAL)?);
    if !(0..US_PER_SEC).contains(&usec) {
        return Err(EDOM);
    }
    if sec < 0 {
        return Ok(Some(0));
    }
    if sec == 0 && usec == 0 {
        return Ok(None);
    }
    Ok(Some(
        (sec as u64)
            .saturating_mul(NS_PER_SEC)
            .saturating_add(usec as u64 * 1_000),
    ))
}

fn put_timeout(out: &mut OptBuf, timeout: Option<u64>) {
    let ns = timeout.unwrap_or(0);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&((ns / NS_PER_SEC) as i64).to_ne_bytes());
    bytes[8..].copy_from_slice(&((ns % NS_PER_SEC / 1_000) as i64).to_ne_bytes());
    out.put(&bytes);
}

/// The bytes of optval setsockopt reads for `level` and `name`, given
/// `optlen`: as on Linux, none when optlen is below an int's 4, the int alone
/// when optlen is below the option's own size (the option then fails with
/// EINVAL), and otherwise exactly the option's size, so a value that ends at
/// a page boundary is never read past. A level this module leaves to the
/// protocol reads up to 16 bytes.
pub fn value_len(level: i32, name: i32, optlen: usize) -> usize {
    let size = match (level, name) {
        (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO) => 16,
        (SOL_SOCKET, SO_LINGER) => 8,
        (SOL_SOCKET, _) | (IPPROTO_TCP, _) => 4,
        // Linux takes a one-byte IP_TTL as well as an int.
        (IPPROTO_IP, IP_TTL) => return if optlen < 4 { optlen.min(1) } else { 4 },
        _ => return optlen.min(16),
    };
    if optlen < 4 {
        0
    } else if optlen < size {
        4
    } else {
        size
    }
}

/// Linux returns IP_TTL as one byte when the caller supplies less than an int.
pub fn result_len(level: i32, name: i32, room: usize, len: usize) -> usize {
    if level == IPPROTO_IP && name == IP_TTL && room < 4 {
        room.min(1)
    } else {
        room.min(len)
    }
}

/// setsockopt(sk, level, name, val). An option stored here is then handed
/// to the protocol's `option_set`, so it reaches state the protocol already
/// has.
pub fn setsockopt(sk: &Socket, level: i32, name: i32, val: &[u8]) -> Result<(), i32> {
    if store(sk, level, name, val)? {
        sk.ops().option_set(sk, level, name);
    }
    Ok(())
}

/// Store a shared option: true when stored here, false when the protocol
/// took it.
fn store(sk: &Socket, level: i32, name: i32, val: &[u8]) -> Result<bool, i32> {
    if unix_protocol_level(sk, level) {
        return Err(EOPNOTSUPP);
    }
    if level == IPPROTO_TCP {
        return set_tcp(sk, name, val);
    }
    if level == IPPROTO_IP && name == IP_TTL && sk.family == AF_INET {
        let ttl = ttl_value(val)?;
        sk.common.with_opts(|o| o.ttl = ttl);
        return Ok(true);
    }
    if level != SOL_SOCKET {
        return sk.ops().setsockopt(sk, level, name, val).map(|()| false);
    }
    let common = &sk.common;
    match name {
        SO_REUSEADDR | SO_BROADCAST | SO_KEEPALIVE => {
            let on = int_value(val)? != 0;
            common.with_opts(|o| match name {
                SO_REUSEADDR => o.reuseaddr = on,
                SO_BROADCAST => o.broadcast = on,
                _ => o.keepalive = on,
            });
            Ok(true)
        }
        SO_RCVBUF => {
            let size = doubled(int_value(val)?, RCVBUF_MIN);
            common.with_opts(|o| o.rcvbuf = size);
            Ok(true)
        }
        SO_SNDBUF => {
            let size = doubled(int_value(val)?, SNDBUF_MIN);
            common.with_opts(|o| o.sndbuf = size);
            Ok(true)
        }
        SO_RCVTIMEO | SO_SNDTIMEO => {
            let timeout = timeout_value(val)?;
            common.with_opts(|o| {
                if name == SO_RCVTIMEO {
                    o.rcvtimeo_ns = timeout;
                } else {
                    o.sndtimeo_ns = timeout;
                }
            });
            Ok(true)
        }
        SO_LINGER => {
            if val.len() < 8 {
                return Err(EINVAL);
            }
            let onoff = i32::from_ne_bytes([val[0], val[1], val[2], val[3]]);
            // Linux reads l_linger as unsigned: a negative one is a long
            // linger, never the abortive close that zero asks for.
            let seconds = u32::from_ne_bytes([val[4], val[5], val[6], val[7]]);
            let linger = (onoff != 0).then_some(seconds);
            common.with_opts(|o| o.linger = linger);
            Ok(true)
        }
        // Read-only options.
        SO_TYPE | SO_PROTOCOL | SO_DOMAIN | SO_ERROR | SO_ACCEPTCONN | SO_PEERCRED => {
            int_value(val)?;
            Err(ENOPROTOOPT)
        }
        _ => {
            int_value(val)?;
            sk.ops().setsockopt(sk, level, name, val).map(|()| false)
        }
    }
}

fn set_tcp(sk: &Socket, name: i32, val: &[u8]) -> Result<bool, i32> {
    if !is_tcp(sk) {
        return Err(ENOPROTOOPT);
    }
    match name {
        TCP_NODELAY => {
            let on = int_value(val)? != 0;
            sk.common.with_opts(|o| o.nodelay = on);
            Ok(true)
        }
        _ => sk.ops().setsockopt(sk, IPPROTO_TCP, name, val).map(|()| false),
    }
}

/// getsockopt(sk, level, name): the value, at most 16 bytes, in `out`.
pub fn getsockopt(sk: &Socket, level: i32, name: i32, out: &mut OptBuf) -> Result<(), i32> {
    if unix_protocol_level(sk, level) {
        return Err(EOPNOTSUPP);
    }
    if level == IPPROTO_TCP {
        if !is_tcp(sk) {
            return Err(ENOPROTOOPT);
        }
        return match name {
            TCP_NODELAY => {
                out.put_int(sk.common.nodelay() as i32);
                Ok(())
            }
            _ => sk.ops().getsockopt(sk, level, name, out),
        };
    }
    if level == IPPROTO_IP && name == IP_TTL && sk.family == AF_INET {
        out.put_int(i32::from(sk.common.ttl()));
        return Ok(());
    }
    if level != SOL_SOCKET {
        return sk.ops().getsockopt(sk, level, name, out);
    }
    let common = &sk.common;
    match name {
        SO_TYPE => out.put_int(i32::from(sk.kind)),
        SO_PROTOCOL => out.put_int(i32::from(sk.protocol)),
        SO_DOMAIN => out.put_int(i32::from(sk.family)),
        SO_ERROR => out.put_int(common.take_error()),
        SO_ACCEPTCONN => out.put_int(sk.ops().is_listening(sk) as i32),
        SO_REUSEADDR => out.put_int(common.reuseaddr() as i32),
        SO_BROADCAST => out.put_int(common.broadcast() as i32),
        SO_KEEPALIVE => out.put_int(common.keepalive() as i32),
        SO_RCVBUF => out.put_int(common.rcvbuf() as i32),
        SO_SNDBUF => out.put_int(common.sndbuf() as i32),
        SO_RCVTIMEO => put_timeout(out, common.with_opts(|o| o.rcvtimeo_ns)),
        SO_SNDTIMEO => put_timeout(out, common.with_opts(|o| o.sndtimeo_ns)),
        SO_LINGER => {
            let linger = common.linger();
            let mut bytes = [0u8; 8];
            bytes[..4].copy_from_slice(&(linger.is_some() as i32).to_ne_bytes());
            bytes[4..].copy_from_slice(&(linger.unwrap_or(0) as i32).to_ne_bytes());
            out.put(&bytes);
        }
        _ => return sk.ops().getsockopt(sk, level, name, out),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_sizes_double_with_linux_limits() {
        assert_eq!(doubled(32_768, RCVBUF_MIN), 65_536);
        assert_eq!(doubled(1, RCVBUF_MIN), RCVBUF_MIN);
        assert_eq!(doubled(-1, SNDBUF_MIN), BUF_MAX * 2);
    }

    #[test]
    fn timeouts_follow_linux() {
        let tv = |sec: i64, usec: i64| {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&sec.to_ne_bytes());
            b[8..].copy_from_slice(&usec.to_ne_bytes());
            b
        };
        assert_eq!(timeout_value(&tv(0, 250_000)), Ok(Some(250_000_000)));
        assert_eq!(timeout_value(&tv(0, 1_000_000)), Err(EDOM));
        assert_eq!(timeout_value(&tv(0, -1)), Err(EDOM));
        assert_eq!(timeout_value(&tv(0, 0)), Ok(None));
        assert_eq!(timeout_value(&tv(-1, 0)), Ok(Some(0)));
        assert_eq!(timeout_value(&tv(0, 1)[..15]), Err(EINVAL));
    }

    #[test]
    fn setsockopt_reads_only_the_option() {
        assert_eq!(value_len(SOL_SOCKET, SO_REUSEADDR, 64), 4);
        assert_eq!(value_len(SOL_SOCKET, SO_REUSEADDR, 3), 0);
        assert_eq!(value_len(SOL_SOCKET, SO_RCVTIMEO, 64), 16);
        assert_eq!(value_len(SOL_SOCKET, SO_RCVTIMEO, 8), 4);
        assert_eq!(value_len(SOL_SOCKET, SO_LINGER, 8), 8);
        assert_eq!(value_len(IPPROTO_TCP, TCP_NODELAY, 64), 4);
        assert_eq!(value_len(0, 1, 64), 16);
        assert_eq!(value_len(0, 1, 2), 2);
        assert_eq!(value_len(IPPROTO_IP, IP_TTL, 64), 4);
        assert_eq!(value_len(IPPROTO_IP, IP_TTL, 1), 1);
        for room in 0..8 {
            let expected = if room < 4 { room.min(1) } else { 4 };
            assert_eq!(value_len(IPPROTO_IP, IP_TTL, room), expected);
            assert_eq!(result_len(IPPROTO_IP, IP_TTL, room, 4), expected);
        }
    }

    #[test]
    fn ttl_follows_linux() {
        assert_eq!(ttl_value(&42i32.to_ne_bytes()), Ok(42));
        assert_eq!(ttl_value(&255i32.to_ne_bytes()), Ok(255));
        assert_eq!(ttl_value(&(-1i32).to_ne_bytes()), Ok(DEFAULT_TTL));
        assert_eq!(ttl_value(&0i32.to_ne_bytes()), Err(EINVAL));
        assert_eq!(ttl_value(&256i32.to_ne_bytes()), Err(EINVAL));
        assert_eq!(ttl_value(&(-2i32).to_ne_bytes()), Err(EINVAL));
        assert_eq!(ttl_value(&[7]), Ok(7));
        assert_eq!(ttl_value(&[0]), Err(EINVAL));
        assert_eq!(ttl_value(&[]), Err(EINVAL));
    }
}
