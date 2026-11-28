//! The network layer around TCP: one interface at 10.0.2.15, IP output that
//! `harness` records, and the IPv4 and ICMP types TCP names.

// The kernel's ICMP calls handle_path_mtu; no ICMP runs here.
#[allow(dead_code)]
#[path = "../../../kernel/src/net/tcp.rs"]
pub mod tcp;
#[path = "../../../kernel/src/net/tcp_ident.rs"]
mod tcp_ident;

pub const LOCAL_IP: [u8; 4] = [10, 0, 2, 15];

pub struct NetConfig {
    pub ip_addr: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
}

pub fn config() -> NetConfig {
    NetConfig {
        ip_addr: LOCAL_IP,
        subnet_mask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
    }
}

pub fn net_lock_guard() {}

pub fn source_ip_in(config: &NetConfig, dst: [u8; 4]) -> [u8; 4] {
    if dst[0] == 127 {
        [127, 0, 0, 1]
    } else {
        config.ip_addr
    }
}

pub fn local_ip_of_inbound(config: &NetConfig, dst: [u8; 4]) -> [u8; 4] {
    if dst[0] == 127 {
        dst
    } else {
        config.ip_addr
    }
}

pub fn current_packet_src_mac() -> [u8; 6] {
    [0x52, 0x55, 10, 0, 2, 2]
}

/// IP output: the segment is recorded, or refused while the harness says so.
pub fn send_ipv4_from(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    _protocol: u8,
    _ttl: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    crate::harness::ip_output(src_ip, dst_ip, payload)
}

/// Link output of an IPv4 packet that `ipv4::Ipv4Packet::try_build` made.
pub fn send_ethernet(_dst_mac: &[u8; 6], _ethertype: u16, packet: &[u8]) -> Result<(), &'static str> {
    if packet.len() < ipv4::HEADER_LEN {
        return Err("Invalid IPv4 packet");
    }
    let src = [packet[12], packet[13], packet[14], packet[15]];
    let dst = [packet[16], packet[17], packet[18], packet[19]];
    crate::harness::ip_output(src, dst, &packet[ipv4::HEADER_LEN..])
}

pub fn drain_loopback_queue() {}

pub mod ethernet {
    pub const ETHERTYPE_IPV4: u16 = 0x0800;
}

pub mod ipv4 {
    pub const PROTOCOL_TCP: u8 = 6;
    pub const DEFAULT_TTL: u8 = 64;
    pub const HEADER_LEN: usize = 20;

    /// The addresses of a received packet, which is all TCP reads of one.
    pub struct Ipv4Packet<'a> {
        pub src_ip: [u8; 4],
        pub dst_ip: [u8; 4],
        pub payload: &'a [u8],
    }

    impl Ipv4Packet<'_> {
        /// A header with the addresses, then `data`; nothing reads its other fields.
        pub fn try_build(
            src_ip: [u8; 4],
            dst_ip: [u8; 4],
            protocol: u8,
            ttl: u8,
            data: &[u8],
        ) -> Result<Vec<u8>, ()> {
            let mut packet = vec![0u8; HEADER_LEN];
            packet[0] = 0x45;
            packet[8] = ttl;
            packet[9] = protocol;
            packet[12..16].copy_from_slice(&src_ip);
            packet[16..20].copy_from_slice(&dst_ip);
            packet.extend_from_slice(data);
            Ok(packet)
        }
    }
}

pub mod icmp {
    /// TCP's view of an ICMP fragmentation-needed report. These checks send none.
    pub struct TcpMtuQuote {
        pub src: [u8; 4],
        pub dst: [u8; 4],
        pub src_port: u16,
        pub dst_port: u16,
    }

    impl TcpMtuQuote {
        #[allow(dead_code)] // called only by tcp::handle_path_mtu
        pub(crate) fn smaller_mss(&self, _mss: u16, _first_unacked: u32, _outstanding: usize) -> Option<u16> {
            None
        }
    }
}
