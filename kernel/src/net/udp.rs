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
        "UDP: Received packet from {}.{}.{}.{}:{} -> port {} ({} bytes)",
        ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3],
        header.src_port,
        header.dst_port,
        payload.len()
    );

    // Look up socket by destination port
    if let Some((pid, _handle)) = crate::socket::SOCKET_REGISTRY.lookup_udp(header.dst_port) {
        // Deliver packet to the socket
        deliver_to_socket(pid, header.dst_port, ip.src_ip, header.src_port, payload);
    } else {
        // No socket listening on this port
        // Could send ICMP port unreachable, but we'll just drop for now
        log::debug!(
            "UDP: No socket listening on port {}, dropping packet",
            header.dst_port
        );
    }
}

/// Deliver a UDP packet to a socket's receive queue
fn deliver_to_socket(
    pid: crate::process::process::ProcessId,
    dst_port: u16,
    src_addr: [u8; 4],
    src_port: u16,
    payload: &[u8],
) {
    use crate::ipc::fd::FdKind;
    use crate::socket::udp::UdpPacket;

    // Access process manager with interrupts disabled to prevent deadlock
    let result = crate::process::with_process_manager(|manager| {
        // Find the process
        let process = match manager.get_process_mut(pid) {
            Some(p) => p,
            None => {
                log::warn!("UDP: Process {:?} not found for port {}", pid, dst_port);
                return;
            }
        };

        // Find the socket in the process's fd_table
        // We need to iterate through all FDs to find the one with matching port
        for fd_num in 3..crate::ipc::fd::MAX_FDS {
            if let Some(fd_entry) = process.fd_table.get(fd_num as i32) {
                if let FdKind::UdpSocket(socket_ref) = &fd_entry.kind {
                    let socket = socket_ref.lock();
                    if socket.local_port == Some(dst_port) {
                        // Found the socket! Enqueue the packet
                        let packet = UdpPacket {
                            src_addr,
                            src_port,
                            data: payload.to_vec(),
                        };
                        socket.enqueue_packet(packet);
                        log::debug!("UDP: Delivered packet to socket on port {}", dst_port);
                        return;
                    }
                }
            }
        }

        log::warn!("UDP: Socket for port {} not found in process {:?} fd_table", dst_port, pid);
    });

    if result.is_none() {
        log::warn!("UDP: Failed to access process manager for packet delivery");
    }
}
