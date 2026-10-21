//! UDP endpoints and a bounded address/port registry, independent of fd tables.
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

use super::addr::SockAddr;
use super::object::{ProtoOps, RecvMsg, SendMsg, Socket, SocketProto};
use super::wait::wait_until;
use crate::syscall::errno::{
    EACCES, EADDRINUSE, EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EDESTADDRREQ, EINVAL, EMSGSIZE,
    ENETUNREACH, ENOBUFS, ENOTCONN, EOPNOTSUPP, EPIPE,
};

const ANY: [u8; 4] = [0; 4];
const MSG_OOB: u32 = 1;
const MSG_PEEK: u32 = 2;
const MSG_TRUNC: u32 = 0x20;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_DONTROUTE: u32 = 4;
const MSG_MORE: u32 = 0x8000;
const FIRST_EPHEMERAL: u16 = 49152;
const DATAGRAM_OVERHEAD: usize = core::mem::size_of::<UdpPacket>();

pub struct UdpPacket {
    pub src_addr: [u8; 4],
    pub src_port: u16,
    pub data: Vec<u8>,
}

struct UdpCork {
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    data: Vec<u8>,
}
enum SendPlan {
    Immediate {
        src: [u8; 4],
        dst: [u8; 4],
        src_port: u16,
        dst_port: u16,
    },
    Buffered,
    Flush(UdpCork),
}

pub struct UdpState {
    local: Option<([u8; 4], u16)>,
    peer: Option<([u8; 4], u16)>,
    source: Option<[u8; 4]>,
    autobound: bool,
    // Holds the original allocation identity across deferred Socket release.
    owner: Weak<Socket>,
    rx: VecDeque<UdpPacket>,
    rx_bytes: usize,
    cork: Option<UdpCork>,
}
impl UdpState {
    const fn new() -> Self {
        Self {
            local: None,
            peer: None,
            source: None,
            autobound: false,
            owner: Weak::new(),
            rx: VecDeque::new(),
            rx_bytes: 0,
            cork: None,
        }
    }
    fn local_ip(&self) -> [u8; 4] {
        let addr = self.local.map_or(ANY, |(addr, _)| addr);
        if addr == ANY {
            self.source.unwrap_or(ANY)
        } else {
            addr
        }
    }
    fn matches(&self, dst: [u8; 4], src: [u8; 4], src_port: u16) -> bool {
        let local = self.local_ip();
        (local == ANY || local == dst) && self.peer.is_none_or(|peer| peer == (src, src_port))
    }
}

pub struct UdpSock {
    state: Mutex<UdpState>,
}

/// Protect every per-endpoint hold against ARM64 IRQ-return softirq and the
/// x86 preemptible loopback pump. Registry precedes this lock; opts is a leaf.
pub fn with_locked_masked<R>(udp: &UdpSock, f: impl FnOnce(&mut UdpState) -> R) -> R {
    #[cfg(target_arch = "x86_64")]
    type Cpu = crate::arch_impl::x86_64::X86Cpu;
    #[cfg(target_arch = "aarch64")]
    type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;
    use crate::arch_impl::traits::CpuOps;
    Cpu::without_interrupts(|| f(&mut udp.state.lock()))
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) fn queued_matching(udp: &UdpSock, payload: &[u8]) -> u64 {
    crate::socket::udp::with_locked_masked(udp, |state| {
        state.rx.iter().filter(|p| p.data == payload).count() as u64
    })
}
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) fn clear_queue(udp: &UdpSock) {
    crate::socket::udp::with_locked_masked(udp, |state| {
        state.rx.clear();
        state.rx_bytes = 0;
    });
}
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) fn with_ports_masked<R>(f: impl FnOnce() -> R) -> R {
    with_registry(|_registry| f())
}

impl UdpSock {
    pub fn new() -> Result<Self, i32> {
        Ok(Self {
            state: Mutex::new(UdpState::new()),
        })
    }
    pub(crate) fn detach(&mut self) -> Self {
        Self {
            state: Mutex::new(core::mem::replace(self.state.get_mut(), UdpState::new())),
        }
    }
    fn ensure_bound(&self, sk: &Arc<Socket>) -> Result<u16, i32> {
        if let Some(port) =
            crate::socket::udp::with_locked_masked(self, |s| s.local.map(|(_, p)| p))
        {
            return Ok(port);
        }
        self.reserve(sk, ANY, 0, true)
    }
    fn reserve(
        &self,
        sk: &Arc<Socket>,
        addr: [u8; 4],
        port: u16,
        automatic: bool,
    ) -> Result<u16, i32> {
        // Any upgrades acquired under the registry survive until it is released:
        // last-Arc Drop may unbind and must never run holding the registry.
        let mut held = Vec::new();
        let result = with_registry(|registry| {
            crate::socket::udp::with_locked_masked(self, |state| {
                if let Some((_, p)) = state.local {
                    return if automatic { Ok(p) } else { Err(EINVAL) };
                }
                registry.prune();
                let actual_port = if port == 0 {
                    // Randomized start and odd stride visit every ephemeral port once.
                    let random = crate::random::next_u64();
                    let start = random as u16 & 16383;
                    let stride = ((random >> 16) as u16 & 16383) | 1;
                    let mut found = None;
                    for n in 0..=16383u16 {
                        let candidate =
                            FIRST_EPHEMERAL + start.wrapping_add(n.wrapping_mul(stride)) % 16384;
                        if !registry.occupied(candidate) {
                            found = Some(candidate);
                            break;
                        }
                    }
                    found.ok_or(if automatic { EAGAIN } else { EADDRINUSE })?
                } else {
                    let range = registry.range(port);
                    held.try_reserve(range.len()).map_err(|_| ENOBUFS)?;
                    for binding in &registry.ports[range] {
                        if !(addr == ANY || binding.addr == ANY || binding.addr == addr) {
                            continue;
                        }
                        if let Some(socket) = binding.owner.upgrade() {
                            let reusable = sk.common.reuseaddr() && socket.common.reuseaddr();
                            held.push(socket);
                            if !reusable {
                                return Err(EADDRINUSE);
                            }
                        }
                    }
                    port
                };
                registry.ports.try_reserve(1).map_err(|_| ENOBUFS)?;
                let owner = Arc::downgrade(sk);
                let index = registry.ports.partition_point(|b| b.port <= actual_port);
                registry.ports.insert(
                    index,
                    Binding {
                        addr,
                        port: actual_port,
                        owner: owner.clone(),
                    },
                );
                registry.mark(actual_port, true);
                state.autobound = automatic;
                state.local = Some((addr, actual_port));
                state.owner = owner;
                Ok(actual_port)
            })
        });
        drop(held);
        result
    }
    fn enqueue(&self, sk: &Socket, dst: [u8; 4], src: [u8; 4], src_port: u16, data: &[u8]) -> bool {
        let accepted = crate::socket::udp::with_locked_masked(self, |state| {
            let charge = data.len() + DATAGRAM_OVERHEAD;
            if !state.matches(dst, src, src_port) || state.rx_bytes > sk.common.rcvbuf() {
                return false;
            }
            let mut owned = Vec::new();
            if owned.try_reserve_exact(data.len()).is_err() || state.rx.try_reserve(1).is_err() {
                return false;
            }
            owned.extend_from_slice(data);
            state.rx.push_back(UdpPacket {
                src_addr: src,
                src_port,
                data: owned,
            });
            state.rx_bytes += charge;
            true
        });
        if accepted {
            static DELIVERY_NOTICE: crate::log_notice::Node = crate::log_notice::Node::new();
            static NOTICED: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            let stage_port = with_locked_masked(self, |s| s.local.is_some_and(|(_, p)| p == 54321));
            if stage_port && !NOTICED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                DELIVERY_NOTICE.post_again("UDP: Delivered packet to socket on port 54321");
            }
            sk.common.notify();
        }
        accepted
    }
}
impl Default for UdpSock {
    fn default() -> Self {
        Self {
            state: Mutex::new(UdpState::new()),
        }
    }
}

struct Binding {
    addr: [u8; 4],
    port: u16,
    owner: Weak<Socket>,
}
pub struct SocketRegistry {
    // Sorted by port: RX visits only this destination's bindings.
    ports: Vec<Binding>,
    used: [u64; 1024],
}
impl SocketRegistry {
    fn range(&self, port: u16) -> core::ops::Range<usize> {
        self.ports.partition_point(|b| b.port < port)
            ..self.ports.partition_point(|b| b.port <= port)
    }
    fn occupied(&self, port: u16) -> bool {
        self.used[port as usize / 64] & (1 << (port % 64)) != 0
    }
    fn mark(&mut self, port: u16, used: bool) {
        let mask = 1 << (port % 64);
        if used {
            self.used[port as usize / 64] |= mask;
        } else {
            self.used[port as usize / 64] &= !mask;
        }
    }
    fn prune(&mut self) {
        self.ports.retain(|b| b.owner.strong_count() != 0);
        self.used.fill(0);
        for b in &self.ports {
            self.used[b.port as usize / 64] |= 1 << (b.port % 64);
        }
    }
}
pub static SOCKET_REGISTRY: Mutex<SocketRegistry> = Mutex::new(SocketRegistry {
    ports: Vec::new(),
    used: [0; 1024],
});
/// Registry insertions and remove operations may allocate/deallocate; memory/heap.rs
/// masks its allocator lock. The outer IRQ mask covers the entire map hold.
fn with_registry<R>(f: impl FnOnce(&mut SocketRegistry) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut SOCKET_REGISTRY.lock()))
}

// A contended receive never blocks an interrupt or softirq.
static UDP_PORTS_LOOKUP_REFUSED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub fn udp_ports_lookup_refused() -> u64 {
    UDP_PORTS_LOOKUP_REFUSED.load(core::sync::atomic::Ordering::Relaxed)
}

/// What `deliver` did with one datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Queued on the matching socket, or dropped by a full receive queue or
    /// memory pressure, which draws no ICMP error.
    Taken,
    /// No socket is bound to the destination: the sender is owed ICMP port
    /// unreachable.
    NoReceiver,
    /// The registry was held on another CPU, so no socket was looked at. The
    /// datagram is untouched: a caller that keeps it can offer it again.
    RegistryBusy,
}

fn receivers(dst: [u8; 4], port: u16) -> Result<Vec<Weak<Socket>>, Delivery> {
    crate::arch_without_interrupts(|| {
        let registry = match SOCKET_REGISTRY.try_lock() {
            Some(registry) => registry,
            None => {
                UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                return Err(Delivery::RegistryBusy);
            }
        };
        let mut out = Vec::new();
        let range = registry.range(port);
        out.try_reserve(range.len()).map_err(|_| Delivery::Taken)?;
        for binding in &registry.ports[range] {
            if binding.addr == ANY || binding.addr == dst {
                out.push(binding.owner.clone());
            }
        }
        Ok(out)
    })
}

/// Unicast selects the most specific (peer, local address) binding, and the
/// most recently bound among equal reuse bindings, matching Linux. Broadcast
/// fans out. A full receive queue drops this input, not a previously queued one.
pub fn deliver(dst: [u8; 4], port: u16, src: [u8; 4], src_port: u16, data: &[u8]) -> Delivery {
    let sockets = match receivers(dst, port) {
        Ok(sockets) => sockets,
        Err(outcome) => return outcome,
    };
    let config = crate::net::config();
    let broadcast = crate::net::is_broadcast_addr_in(&config, dst);
    let mut selected: Option<(Arc<Socket>, u8)> = None;
    let mut found = false;
    for weak in sockets.into_iter().rev() {
        let Some(sk) = weak.upgrade() else {
            continue;
        };
        let SocketProto::Udp(udp) = &sk.proto else {
            continue;
        };
        let score = crate::socket::udp::with_locked_masked(udp, |s| {
            if s.local.is_none() || !s.matches(dst, src, src_port) {
                None
            } else {
                Some(u8::from(s.peer.is_some()) * 2 + u8::from(s.local_ip() != ANY))
            }
        });
        let Some(score) = score else {
            continue;
        };
        found = true;
        if broadcast {
            udp.enqueue(&sk, dst, src, src_port, data);
        } else if selected.as_ref().is_none_or(|(_, old)| score > *old) {
            selected = Some((sk, score));
        }
    }
    if let Some((sk, _)) = selected {
        if let SocketProto::Udp(udp) = &sk.proto {
            udp.enqueue(&sk, dst, src, src_port, data);
        }
    }
    if found {
        Delivery::Taken
    } else {
        Delivery::NoReceiver
    }
}

pub fn udp_icmp_error(
    src: [u8; 4],
    src_port: u16,
    dst: [u8; 4],
    dst_port: u16,
    errno: i32,
) -> Delivery {
    let sockets = match receivers(src, src_port) {
        Ok(sockets) => sockets,
        Err(outcome) => return outcome,
    };
    let mut selected: Option<(Arc<Socket>, bool)> = None;
    for weak in sockets.into_iter().rev() {
        let Some(sk) = weak.upgrade() else {
            continue;
        };
        let SocketProto::Udp(udp) = &sk.proto else {
            continue;
        };
        let score = with_locked_masked(udp, |s| {
            if s.peer == Some((dst, dst_port)) && (s.local_ip() == src || s.local_ip() == ANY) {
                Some(s.local.is_some_and(|(addr, _)| addr != ANY))
            } else {
                None
            }
        });
        if let Some(score) = score {
            if selected.as_ref().is_none_or(|(_, old)| score > *old) {
                selected = Some((sk, score));
            }
        }
    }
    if let Some((sk, _)) = selected {
        sk.common.set_error(errno);
        sk.common.notify();
    }
    Delivery::Taken
}

impl ProtoOps for UdpSock {
    fn notifies(&self) -> bool {
        true
    }
    fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        if matches!(addr, SockAddr::UnspecInet { addr, .. } if *addr != ANY) {
            return Err(EAFNOSUPPORT);
        }
        let (SockAddr::Inet { addr, port } | SockAddr::UnspecInet { addr, port }) = addr else {
            return Err(EAFNOSUPPORT);
        };
        let config = crate::net::config();
        if *addr != ANY
            && !crate::net::is_local_addr_in(&config, *addr)
            && !crate::net::is_broadcast_addr_in(&config, *addr)
        {
            return Err(EADDRNOTAVAIL);
        } // EADDRNOTAVAIL
        if *port != 0 && *port < 1024 {
            crate::process::privilege::require(crate::process::privilege::Operation::LowPort)
                .map_err(|errno| errno as i32)?;
        }
        if crate::socket::udp::with_locked_masked(self, |s| s.local.is_some()) {
            return Err(EINVAL);
        }
        self.reserve(sk, *addr, *port, false)?;
        sk.common.notify();
        Ok(())
    }
    fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, _nonblock: bool) -> Result<(), i32> {
        let config = crate::net::config();
        match addr {
            SockAddr::Unspec | SockAddr::UnspecInet { .. } => {
                let automatic = with_locked_masked(self, |s| s.autobound);
                if automatic {
                    with_registry(|registry| {
                        with_locked_masked(self, |s| {
                            registry.ports.retain(|b| !Weak::ptr_eq(&b.owner, &s.owner));
                            registry.prune();
                            s.local = None;
                            s.autobound = false;
                            s.cork = None;
                        });
                    });
                }
                crate::socket::udp::with_locked_masked(self, |s| {
                    s.peer = None;
                    s.source = None;
                });
            }
            SockAddr::Inet { addr, port } => {
                let dst = if *addr == ANY { [127, 0, 0, 1] } else { *addr };
                if crate::net::is_broadcast_addr_in(&config, dst) && !sk.common.broadcast() {
                    return Err(EACCES);
                }
                self.ensure_bound(sk)?;
                let source = crate::net::source_ip_in(&config, dst);
                crate::socket::udp::with_locked_masked(self, |s| {
                    s.peer = Some((dst, *port));
                    s.source = Some(source);
                });
            }
            _ => return Err(EAFNOSUPPORT),
        }
        sk.common.notify();
        Ok(())
    }
    fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
        if msg.flags & MSG_OOB != 0 {
            return Err(EOPNOTSUPP);
        }
        if msg.data.len() > 65507 {
            return Err(EMSGSIZE);
        }
        if sk.common.shut_wr() {
            return Err(EPIPE);
        }
        let pending = sk.common.take_error();
        if pending != 0 {
            return Err(pending);
        }
        self.ensure_bound(sk)?;
        let config = crate::net::config();
        // Resolve the peer, route and any existing cork in one state hold so
        // concurrent send/connect calls cannot append using a stale destination.
        let plan = crate::socket::udp::with_locked_masked(self, |s| {
            let dest = if let Some(cork) = &s.cork {
                (cork.dst, cork.dst_port)
            } else {
                match &msg.dest {
                    Some(SockAddr::Inet { addr, port } | SockAddr::UnspecInet { addr, port }) => {
                        if *port == 0 {
                            return Err(EINVAL);
                        }
                        (*addr, *port)
                    }
                    Some(_) => return Err(EAFNOSUPPORT),
                    None => s.peer.ok_or(EDESTADDRREQ)?, // EDESTADDRREQ
                }
            };
            let dst = if dest.0 == ANY {
                [127, 0, 0, 1]
            } else {
                dest.0
            };
            if msg.flags & MSG_DONTROUTE != 0
                && !crate::net::is_local_addr_in(&config, dst)
                && !crate::net::is_broadcast_addr_in(&config, dst)
                && !(0..4).all(|i| {
                    dst[i] & config.subnet_mask[i] == config.ip_addr[i] & config.subnet_mask[i]
                })
            {
                return Err(ENETUNREACH); // ENETUNREACH: no directly connected route.
            }
            if crate::net::is_broadcast_addr_in(&config, dst) && !sk.common.broadcast() {
                return Err(EACCES);
            }
            let port = s.local.expect("autobound before send").1;
            let local = s.local_ip();
            let src = if local == ANY {
                crate::net::source_ip_in(&config, dst)
            } else {
                local
            };
            if src[0] == 127 && dst[0] != 127 && dst != config.ip_addr {
                return Err(EINVAL);
            }
            if s.cork.is_none() && msg.flags & MSG_MORE == 0 {
                return Ok(SendPlan::Immediate {
                    src,
                    dst,
                    src_port: port,
                    dst_port: dest.1,
                });
            }
            let mut cork = s.cork.take().unwrap_or(UdpCork {
                src,
                dst,
                src_port: port,
                dst_port: dest.1,
                data: Vec::new(),
            });
            // Linux discards an unfinished cork if the combined datagram overflows.
            if msg.data.len() > 65507 - cork.data.len() {
                return Err(EMSGSIZE);
            }
            cork.data.try_reserve(msg.data.len()).map_err(|_| ENOBUFS)?;
            cork.data.extend_from_slice(msg.data);
            if msg.flags & MSG_MORE != 0 {
                s.cork = Some(cork);
                Ok(SendPlan::Buffered)
            } else {
                Ok(SendPlan::Flush(cork))
            }
        })?;
        let (src, dst, src_port, dst_port, data) = match &plan {
            SendPlan::Buffered => return Ok(msg.data.len()),
            SendPlan::Immediate {
                src,
                dst,
                src_port,
                dst_port,
            } => (*src, *dst, *src_port, *dst_port, msg.data),
            SendPlan::Flush(cork) => (
                cork.src,
                cork.dst,
                cork.src_port,
                cork.dst_port,
                cork.data.as_slice(),
            ),
        };
        let packet = crate::net::udp::try_build_udp_packet(src, dst, src_port, dst_port, data)
            .map_err(crate::net::TxError::errno)?;
        crate::net::ip_output(
            src,
            dst,
            crate::net::ipv4::PROTOCOL_UDP,
            sk.common.ttl(),
            &packet,
        )
        .map_err(crate::net::TxError::errno)?;
        crate::net::drain_loopback_queue();
        Ok(msg.data.len())
    }
    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
        if msg.flags & MSG_OOB != 0 {
            return Err(EOPNOTSUPP);
        }
        if msg.flags & 0x2000 != 0 {
            return Err(EAGAIN);
        } // MSG_ERRQUEUE: no extended error queue.
        let deadline = sk.common.rcv_deadline();
        wait_until(
            sk,
            msg.nonblock || msg.flags & MSG_DONTWAIT != 0,
            deadline,
            || {
                let pending = sk.common.take_error();
                if pending != 0 {
                    return Some(Err(pending));
                }
                let result = crate::socket::udp::with_locked_masked(self, |s| {
                    let Some(packet) = s.rx.front() else {
                        return if sk.common.shut_rd() {
                            Some(Ok(0))
                        } else {
                            None
                        };
                    };
                    let len = packet.data.len();
                    let n = len.min(msg.cap);
                    if msg.data.try_reserve(n).is_err() {
                        return Some(Err(crate::syscall::errno::ENOMEM));
                    }
                    msg.data.extend_from_slice(&packet.data[..n]);
                    msg.from = Some(SockAddr::Inet {
                        addr: packet.src_addr,
                        port: packet.src_port,
                    });
                    if n < len {
                        msg.out_flags |= MSG_TRUNC;
                    }
                    if msg.flags & MSG_PEEK == 0 {
                        s.rx.pop_front();
                        s.rx_bytes -= len + DATAGRAM_OVERHEAD;
                    }
                    Some(Ok(if msg.flags & MSG_TRUNC != 0 { len } else { n }))
                });
                result
            },
        )
    }
    fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32> {
        if how > 2 {
            return Err(EINVAL);
        }
        sk.common.set_shut(how);
        let connected = crate::socket::udp::with_locked_masked(self, |s| s.peer.is_some());
        sk.common.notify();
        if connected {
            Ok(())
        } else {
            Err(ENOTCONN)
        }
    }
    fn local_name(&self, _sk: &Socket) -> SockAddr {
        crate::socket::udp::with_locked_masked(self, |s| SockAddr::Inet {
            addr: s.local_ip(),
            port: s.local.map_or(0, |(_, p)| p),
        })
    }
    fn peer_name(&self, _sk: &Socket) -> Result<SockAddr, i32> {
        crate::socket::udp::with_locked_masked(self, |s| {
            s.peer
                .map(|(addr, port)| SockAddr::Inet { addr, port })
                .ok_or(ENOTCONN)
        })
    }
    fn poll(&self, sk: &Socket) -> u16 {
        let readable = crate::socket::udp::with_locked_masked(self, |s| !s.rx.is_empty());
        let mut bits = 4; // POLLOUT
        if readable || sk.common.shut_rd() {
            bits |= 1;
            if sk.common.shut_rd() {
                bits |= 0x2000;
            }
        }
        if sk.common.peek_error() != 0 {
            bits |= 8;
        }
        if sk.common.shut_rd() && sk.common.shut_wr() {
            bits |= 16;
        }
        bits
    }
    fn release(&self, _sk: &Socket) {
        with_registry(|registry| {
            crate::socket::udp::with_locked_masked(self, |s| {
                registry.ports.retain(|b| !Weak::ptr_eq(&b.owner, &s.owner));
                registry.prune();
                s.local = None;
                s.autobound = false;
                s.peer = None;
                s.source = None;
                s.cork = None;
                s.rx.clear();
                s.rx_bytes = 0;
            });
        });
    }
}
