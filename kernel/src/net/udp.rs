//! UDP (User Datagram Protocol) implementation
//!
//! Implements UDP packet parsing and construction (RFC 768).

use alloc::vec::Vec;

use super::ipv4::{internet_checksum, Ipv4Packet};

/// UDP header size
pub const UDP_HEADER_SIZE: usize = 8;

/// Parsed UDP header
#[derive(Debug)]
pub struct UdpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Length (header + data) - stored but not used after parsing
    pub _length: u16,
    /// Checksum - stored but not used after parsing
    pub _checksum: u16,
}

impl UdpHeader {
    /// Parse a UDP header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])> {
        if data.len() < UDP_HEADER_SIZE {
            return None;
        }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let length = u16::from_be_bytes([data[4], data[5]]);
        let checksum = u16::from_be_bytes([data[6], data[7]]);

        // Validate length
        if (length as usize) < UDP_HEADER_SIZE || (length as usize) > data.len() {
            return None;
        }

        let payload = &data[UDP_HEADER_SIZE..(length as usize)];

        Some((
            UdpHeader {
                src_port,
                dst_port,
                _length: length,
                _checksum: checksum,
            },
            payload,
        ))
    }
}

/// Build a UDP packet (header + payload)
pub fn build_udp_packet(src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let length = (UDP_HEADER_SIZE + payload.len()) as u16;

    let mut packet = Vec::with_capacity(length as usize);

    // Source port
    packet.extend_from_slice(&src_port.to_be_bytes());
    // Destination port
    packet.extend_from_slice(&dst_port.to_be_bytes());
    // Length
    packet.extend_from_slice(&length.to_be_bytes());
    // Checksum (0 = disabled for now, valid per RFC 768)
    packet.extend_from_slice(&0u16.to_be_bytes());

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Calculate UDP checksum with pseudo-header
#[allow(dead_code)]
pub fn udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp_packet: &[u8]) -> u16 {
    // Build pseudo-header for checksum calculation
    let mut pseudo_header = Vec::with_capacity(12 + udp_packet.len());

    // Source IP
    pseudo_header.extend_from_slice(&src_ip);
    // Destination IP
    pseudo_header.extend_from_slice(&dst_ip);
    // Zero
    pseudo_header.push(0);
    // Protocol (UDP = 17)
    pseudo_header.push(17);
    // UDP length
    pseudo_header.extend_from_slice(&(udp_packet.len() as u16).to_be_bytes());
    // UDP header + data
    pseudo_header.extend_from_slice(udp_packet);

    internet_checksum(&pseudo_header)
}

/// Handle an incoming UDP packet
pub fn handle_udp(ip: &Ipv4Packet, data: &[u8]) {
    let (header, payload) = match UdpHeader::parse(data) {
        Some(h) => h,
        None => {
            log::warn!("UDP: Failed to parse header");
            return;
        }
    };

    log::debug!(
        "UDP driver received packet from {}.{}.{}.{}:{} -> port {} ({} bytes)",
        ip.src_ip[0],
        ip.src_ip[1],
        ip.src_ip[2],
        ip.src_ip[3],
        header.src_port,
        header.dst_port,
        payload.len()
    );

    // The registry identifies the socket, rather than a descriptor's original
    // PID: fork/dup may leave the only descriptor in another process. Borrow
    // under PM so close cannot drop the last reference on this IRQ path.
    if let Some(Some((_, handle))) = crate::socket::SOCKET_REGISTRY.try_lookup_udp(header.dst_port) {
        crate::process::with_process_manager(|manager| {
            for (_, process) in manager.iter_processes() {
                for fd in 0..crate::ipc::fd::MAX_FDS {
                    let Some(entry) = process.fd_table.get(fd as i32) else { continue };
                    let crate::ipc::fd::FdKind::Socket(sk) = &entry.kind else { continue };
                    if let crate::socket::object::SocketProto::Udp(udp) = &sk.proto {
                        let endpoint = &udp.inner;
                        let delivered = crate::socket::udp::with_locked_masked(endpoint, |socket| {
                            if socket.handle == handle {
                                socket.enqueue_packet(crate::socket::udp::UdpPacket {
                                    src_addr: ip.src_ip,
                                    src_port: header.src_port,
                                    data: payload.to_vec(),
                                });
                                // The datagram is on the socket's RX queue. This
                                // path may be the NetRx softirq, so the line is a
                                // lock-free notice, once per socket, not a log call.
                                socket.delivery_notice.post(
                                    "UDP: Delivered packet to socket on port ",
                                    u64::from(header.dst_port),
                                );
                                true
                            } else {
                                false
                            }
                        });
                        if delivered {
                            // PM is held: the wake goes through the deferred path.
                            sk.common.notify_deferred();
                            return;
                        }
                    }
                }
            }
        });
    }
}
