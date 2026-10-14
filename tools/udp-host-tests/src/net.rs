#[derive(Debug)]
pub enum TxError {
    NoRoute,
    NoBufs,
    MsgSize,
}
impl TxError {
    pub fn errno(self) -> i32 {
        match self {
            Self::NoRoute => 101,
            Self::NoBufs => 105,
            Self::MsgSize => 90,
        }
    }
}
pub fn net_lock_guard() {}
pub struct Config {
    pub ip_addr: [u8; 4],
    pub gateway: [u8; 4],
    pub subnet_mask: [u8; 4],
}
pub fn config() -> Config {
    Config {
        ip_addr: [10, 0, 2, 15],
        gateway: [10, 0, 2, 2],
        subnet_mask: [255, 255, 255, 0],
    }
}
pub fn is_local_addr(addr: [u8; 4]) -> bool {
    is_local_addr_in(&config(), addr)
}
pub fn is_broadcast_addr(addr: [u8; 4]) -> bool {
    is_broadcast_addr_in(&config(), addr)
}
pub fn is_local_addr_in(cfg: &Config, addr: [u8; 4]) -> bool {
    addr[0] == 127 || addr == cfg.ip_addr
}
pub fn is_broadcast_addr_in(cfg: &Config, addr: [u8; 4]) -> bool {
    addr == [255; 4] || (0..4).all(|i| addr[i] == cfg.ip_addr[i] | !cfg.subnet_mask[i])
}
pub fn source_ip_in(config: &Config, dst: [u8; 4]) -> [u8; 4] {
    if dst[0] == 127 {
        [127, 0, 0, 1]
    } else {
        config.ip_addr
    }
}
#[path = "../../../kernel/src/net/ethernet.rs"]
pub mod ethernet;
#[path = "../../../kernel/src/net/icmp.rs"]
pub mod icmp;
#[path = "../../../kernel/src/net/ipv4.rs"]
pub mod ipv4;
#[path = "../../../kernel/src/net/udp.rs"]
pub mod udp;
pub mod tcp {
    pub fn handle_tcp(_: &super::ipv4::Ipv4Packet, _: &[u8]) {
        unreachable!()
    }
}
/// No DHCP exchange runs here. The kernel's client claims a datagram only while
/// it awaits a reply, so with none awaited it claims nothing.
pub mod dhcp {
    pub fn receive(_: &super::ipv4::Ipv4Packet) -> bool {
        false
    }
}
pub fn ip_output(src: [u8; 4], dst: [u8; 4], protocol: u8, data: &[u8]) -> Result<(), TxError> {
    let packet = ipv4::Ipv4Packet::try_build(src, dst, protocol, data)?;
    if is_local_addr(dst) || is_broadcast_addr(dst) {
        let ip = ipv4::Ipv4Packet::parse(&packet).unwrap();
        ipv4::handle_loopback_ipv4(&ip);
    }
    Ok(())
}
pub fn drain_loopback_queue() {}
