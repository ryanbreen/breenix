//! TCP (Transmission Control Protocol) implementation
//!
//! Implements TCP packet parsing, construction, and connection state machine (RFC 793).

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use super::ipv4::{internet_checksum, Ipv4Packet, PROTOCOL_TCP};

/// TCP header minimum size (without options)
pub const TCP_HEADER_MIN_SIZE: usize = 20;

/// TCP flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpFlags {
    pub fin: bool,
    pub syn: bool,
    pub rst: bool,
    pub psh: bool,
    pub ack: bool,
    pub urg: bool,
}

impl TcpFlags {
    /// Parse flags from the flags byte
    pub fn from_byte(byte: u8) -> Self {
        TcpFlags {
            fin: byte & 0x01 != 0,
            syn: byte & 0x02 != 0,
            rst: byte & 0x04 != 0,
            psh: byte & 0x08 != 0,
            ack: byte & 0x10 != 0,
            urg: byte & 0x20 != 0,
        }
    }

    /// Convert flags to byte
    pub fn to_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.fin { byte |= 0x01; }
        if self.syn { byte |= 0x02; }
        if self.rst { byte |= 0x04; }
        if self.psh { byte |= 0x08; }
        if self.ack { byte |= 0x10; }
        if self.urg { byte |= 0x20; }
        byte
    }

    /// Create SYN flag set
    pub fn syn() -> Self {
        TcpFlags { fin: false, syn: true, rst: false, psh: false, ack: false, urg: false }
    }

    /// Create SYN+ACK flag set
    pub fn syn_ack() -> Self {
        TcpFlags { fin: false, syn: true, rst: false, psh: false, ack: true, urg: false }
    }

    /// Create ACK flag set
    pub fn ack() -> Self {
        TcpFlags { fin: false, syn: false, rst: false, psh: false, ack: true, urg: false }
    }

    /// Create ACK+PSH flag set (for data)
    pub fn ack_psh() -> Self {
        TcpFlags { fin: false, syn: false, rst: false, psh: true, ack: true, urg: false }
    }

    /// Create FIN+ACK flag set
    pub fn fin_ack() -> Self {
        TcpFlags { fin: true, syn: false, rst: false, psh: false, ack: true, urg: false }
    }

    /// Create RST flag set
    pub fn rst() -> Self {
        TcpFlags { fin: false, syn: false, rst: true, psh: false, ack: false, urg: false }
    }
}

/// Parsed TCP header
///
/// All fields are parsed from the TCP header for completeness and protocol conformance.
#[derive(Debug)]
#[allow(dead_code)] // All TCP header fields are part of RFC 793 protocol structure
pub struct TcpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Sequence number
    pub seq_num: u32,
    /// Acknowledgment number
    pub ack_num: u32,
    /// Data offset in 32-bit words (header length)
    pub data_offset: u8,
    /// TCP flags
    pub flags: TcpFlags,
    /// Window size
    pub window_size: u16,
    /// Checksum
    pub checksum: u16,
    /// Urgent pointer
    pub urgent_ptr: u16,
}

impl TcpHeader {
    /// Parse a TCP header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])> {
        if data.len() < TCP_HEADER_MIN_SIZE {
            return None;
        }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let seq_num = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack_num = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        // Data offset is upper 4 bits of byte 12, in 32-bit words
        let data_offset = (data[12] >> 4) & 0x0F;
        let header_len = (data_offset as usize) * 4;

        if header_len < TCP_HEADER_MIN_SIZE || header_len > data.len() {
            return None;
        }

        let flags = TcpFlags::from_byte(data[13]);
        let window_size = u16::from_be_bytes([data[14], data[15]]);
        let checksum = u16::from_be_bytes([data[16], data[17]]);
        let urgent_ptr = u16::from_be_bytes([data[18], data[19]]);

        let payload = &data[header_len..];

        Some((
            TcpHeader {
                src_port,
                dst_port,
                seq_num,
                ack_num,
                data_offset,
                flags,
                window_size,
                checksum,
                urgent_ptr,
            },
            payload,
        ))
    }
}

/// Build a TCP packet (header + payload)
pub fn build_tcp_packet(
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) -> Vec<u8> {
    let header_len = TCP_HEADER_MIN_SIZE; // No options for now
    let data_offset = (header_len / 4) as u8;

    let mut packet = Vec::with_capacity(header_len + payload.len());

    // Source port
    packet.extend_from_slice(&src_port.to_be_bytes());
    // Destination port
    packet.extend_from_slice(&dst_port.to_be_bytes());
    // Sequence number
    packet.extend_from_slice(&seq_num.to_be_bytes());
    // Acknowledgment number
    packet.extend_from_slice(&ack_num.to_be_bytes());
    // Data offset (4 bits) + reserved (4 bits)
    packet.push((data_offset << 4) | 0);
    // Flags
    packet.push(flags.to_byte());
    // Window size
    packet.extend_from_slice(&window_size.to_be_bytes());
    // Checksum (placeholder, will be calculated)
    packet.extend_from_slice(&0u16.to_be_bytes());
    // Urgent pointer
    packet.extend_from_slice(&0u16.to_be_bytes());

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Calculate TCP checksum with pseudo-header
pub fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_packet: &[u8]) -> u16 {
    // Build pseudo-header for checksum calculation
    let mut pseudo_header = Vec::with_capacity(12 + tcp_packet.len());

    // Source IP
    pseudo_header.extend_from_slice(&src_ip);
    // Destination IP
    pseudo_header.extend_from_slice(&dst_ip);
    // Zero
    pseudo_header.push(0);
    // Protocol (TCP = 6)
    pseudo_header.push(PROTOCOL_TCP);
    // TCP length
    pseudo_header.extend_from_slice(&(tcp_packet.len() as u16).to_be_bytes());
    // TCP header + data
    pseudo_header.extend_from_slice(tcp_packet);

    internet_checksum(&pseudo_header)
}

/// Build a TCP packet with correct checksum
pub fn build_tcp_packet_with_checksum(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = build_tcp_packet(src_port, dst_port, seq_num, ack_num, flags, window_size, payload);

    // Calculate checksum
    let checksum = tcp_checksum(src_ip, dst_ip, &packet);

    // Insert checksum at offset 16-17
    packet[16] = (checksum >> 8) as u8;
    packet[17] = (checksum & 0xFF) as u8;

    packet
}

/// TCP connection state (RFC 793)
///
/// All variants are part of the complete TCP state machine as defined in RFC 793.
/// Some states may not be actively used yet as the implementation matures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // RFC 793 state machine - all states are part of the complete protocol
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// Connection identifier (4-tuple)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId {
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
}

/// TCP connection state
pub struct TcpConnection {
    pub id: ConnectionId,
    pub state: TcpState,
    /// Our sequence number (next byte to send)
    pub send_next: u32,
    /// Initial send sequence number (RFC 793 - needed for retransmission and RST validation)
    #[allow(dead_code)] // Part of RFC 793 state machine, needed for future retransmission logic
    pub send_initial: u32,
    /// Send unacknowledged (oldest unacked seq)
    pub send_unack: u32,
    /// Remote's sequence number (next byte expected)
    pub recv_next: u32,
    /// Initial receive sequence number
    pub recv_initial: u32,
    /// Our window size
    pub recv_window: u16,
    /// Remote's window size
    pub send_window: u16,
    /// Pending data to receive
    pub rx_buffer: VecDeque<u8>,
    /// Pending data to send (for future send buffering/retransmission)
    #[allow(dead_code)] // Part of TCP API, needed for send buffering when window is full
    pub tx_buffer: VecDeque<u8>,
    /// Maximum segment size
    pub mss: u16,
    /// Process ID that owns this connection (for cleanup on process exit)
    #[allow(dead_code)] // Needed for connection ownership tracking
    pub owner_pid: crate::process::process::ProcessId,
    /// True if SHUT_WR was called (no more sending)
    pub send_shutdown: bool,
    /// True if SHUT_RD was called (no more receiving)
    pub recv_shutdown: bool,
    /// Reference count for fork() support - connection is only closed when last fd is closed
    pub refcount: core::sync::atomic::AtomicUsize,
    /// Threads waiting for recv data or connection state change (connect/recv blocking)
    pub waiting_threads: Mutex<Vec<u64>>,
}

impl TcpConnection {
    /// Create a new connection in SYN_SENT state (for connect)
    pub fn new_outgoing(
        id: ConnectionId,
        initial_seq: u32,
        owner_pid: crate::process::process::ProcessId,
    ) -> Self {
        TcpConnection {
            id,
            state: TcpState::SynSent,
            send_next: initial_seq.wrapping_add(1), // SYN consumes a sequence number
            send_initial: initial_seq,
            send_unack: initial_seq,
            recv_next: 0,
            recv_initial: 0,
            recv_window: 65535,
            send_window: 0,
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
            mss: 1460, // Default MSS for Ethernet
            owner_pid,
            send_shutdown: false,
            recv_shutdown: false,
            refcount: core::sync::atomic::AtomicUsize::new(1),
            waiting_threads: Mutex::new(Vec::new()),
        }
    }

    /// Create a new connection in LISTEN state (for accept)
    #[allow(dead_code)] // Part of TCP server API, used when implementing server-side accept
    pub fn new_listening(
        local_ip: [u8; 4],
        local_port: u16,
        owner_pid: crate::process::process::ProcessId,
    ) -> Self {
        TcpConnection {
            id: ConnectionId {
                local_ip,
                local_port,
                remote_ip: [0; 4],
                remote_port: 0,
            },
            state: TcpState::Listen,
            send_next: 0,
            send_initial: 0,
            send_unack: 0,
            recv_next: 0,
            recv_initial: 0,
            recv_window: 65535,
            send_window: 0,
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
            mss: 1460,
            owner_pid,
            send_shutdown: false,
            recv_shutdown: false,
            refcount: core::sync::atomic::AtomicUsize::new(1),
            waiting_threads: Mutex::new(Vec::new()),
        }
    }
}

/// Pending connection (from SYN received, waiting for accept)
pub struct PendingConnection {
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub recv_initial: u32,
    pub send_initial: u32,
    /// True if the final ACK of the 3-way handshake has been received
    pub ack_received: bool,
    /// Data received before accept() was called (buffered here until connection is created)
    pub early_data: Vec<u8>,
    /// Next expected sequence number for early data
    pub recv_next: u32,
}

/// Listening socket info
pub struct ListenSocket {
    /// Local IP address for this listening socket (for binding to specific interfaces)
    #[allow(dead_code)] // Part of socket bind API, needed for interface-specific listening
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub backlog: usize,
    pub pending: VecDeque<PendingConnection>,
    pub owner_pid: crate::process::process::ProcessId,
    /// Threads waiting for incoming connections (accept() blocking)
    pub waiting_threads: Mutex<Vec<u64>>,
}

/// Global TCP connection table
pub static TCP_CONNECTIONS: Mutex<BTreeMap<ConnectionId, TcpConnection>> = Mutex::new(BTreeMap::new());

/// Global listening socket table (by local port)
pub static TCP_LISTENERS: Mutex<BTreeMap<u16, ListenSocket>> = Mutex::new(BTreeMap::new());

/// Sequence number generator (simple counter, should be more random in production)
static SEQ_COUNTER: Mutex<u32> = Mutex::new(0x12345678);

/// Generate a new initial sequence number
pub fn generate_isn() -> u32 {
    let mut counter = SEQ_COUNTER.lock();
    let isn = *counter;
    *counter = counter.wrapping_add(64000); // Increment by a large amount
    isn
}

/// Handle an incoming TCP packet
pub fn handle_tcp(ip: &Ipv4Packet, data: &[u8]) {
    let (header, payload) = match TcpHeader::parse(data) {
        Some(h) => h,
        None => {
            log::warn!("TCP: Failed to parse header");
            return;
        }
    };

    log::debug!(
        "TCP: Received {} bytes from {}.{}.{}.{}:{} -> port {} seq={} ack={} flags={:?}",
        payload.len(),
        ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3],
        header.src_port,
        header.dst_port,
        header.seq_num,
        header.ack_num,
        header.flags
    );

    let config = super::config();
    let conn_id = ConnectionId {
        local_ip: config.ip_addr,
        local_port: header.dst_port,
        remote_ip: ip.src_ip,
        remote_port: header.src_port,
    };

    // First, check for an existing connection
    {
        let mut connections = TCP_CONNECTIONS.lock();
        if let Some(conn) = connections.get_mut(&conn_id) {
            handle_tcp_for_connection(conn, &header, payload, &config);
            return;
        }
    }

    // No existing connection - check for listening socket
    {
        let mut listeners = TCP_LISTENERS.lock();
        if let Some(listener) = listeners.get_mut(&header.dst_port) {
            if header.flags.syn && !header.flags.ack {
                // SYN received on listening socket - add to pending queue
                handle_syn_for_listener(listener, ip.src_ip, &header, &config);
            } else if header.flags.ack && !header.flags.syn {
                // ACK received - this completes the 3-way handshake
                // Find matching pending connection, mark it as ready, and buffer any data
                for pending in listener.pending.iter_mut() {
                    if pending.remote_ip == ip.src_ip && pending.remote_port == header.src_port {
                        // Verify ACK number matches our SYN+ACK (send_initial + 1)
                        if header.ack_num == pending.send_initial.wrapping_add(1) {
                            pending.ack_received = true;
                            log::debug!("TCP: ACK received, handshake complete for pending connection");
                        }
                        // Buffer any data that arrived with this packet
                        if !payload.is_empty() && header.seq_num == pending.recv_next {
                            pending.early_data.extend_from_slice(payload);
                            pending.recv_next = pending.recv_next.wrapping_add(payload.len() as u32);
                            log::debug!("TCP: Buffered {} bytes of early data for pending connection", payload.len());
                        }
                        break;
                    }
                }
            } else {
                log::debug!("TCP: Ignoring packet on listening socket");
            }
            return;
        }
    }

    // No connection and no listener - send RST
    log::debug!("TCP: No socket for port {}, sending RST", header.dst_port);
    send_rst(&config, ip.src_ip, &header);
}

/// Handle TCP packet for an established connection
fn handle_tcp_for_connection(
    conn: &mut TcpConnection,
    header: &TcpHeader,
    payload: &[u8],
    config: &super::NetConfig,
) {
    match conn.state {
        TcpState::SynSent => {
            // We sent SYN, expecting SYN+ACK
            if header.flags.syn && header.flags.ack {
                // Verify ACK is for our SYN
                if header.ack_num == conn.send_next {
                    conn.recv_initial = header.seq_num;
                    conn.recv_next = header.seq_num.wrapping_add(1);
                    conn.send_window = header.window_size;
                    conn.send_unack = header.ack_num;
                    conn.state = TcpState::Established;

                    log::info!(
                        "TCP: Connection established (client) conn_id={{local={}:{}, remote={}:{}}}",
                        conn.id.local_ip[3], conn.id.local_port,
                        conn.id.remote_ip[3], conn.id.remote_port
                    );

                    // Send ACK
                    send_tcp_packet(
                        config,
                        conn.id.remote_ip,
                        conn.id.local_port,
                        conn.id.remote_port,
                        conn.send_next,
                        conn.recv_next,
                        TcpFlags::ack(),
                        conn.recv_window,
                        &[],
                    );

                    // Wake threads blocked in connect()
                    wake_connection_waiters(conn);
                }
            } else if header.flags.rst {
                log::info!("TCP: Connection refused (RST received)");
                conn.state = TcpState::Closed;
                // Wake threads blocked in connect() so they see the failure
                wake_connection_waiters(conn);
            }
        }
        TcpState::SynReceived => {
            // We sent SYN+ACK, expecting ACK
            if header.flags.ack && !header.flags.syn {
                if header.ack_num == conn.send_next {
                    conn.state = TcpState::Established;
                    conn.send_unack = header.ack_num;
                    log::info!("TCP: Connection established (server)");
                }
            } else if header.flags.rst {
                conn.state = TcpState::Closed;
            }
        }
        TcpState::Established => {
            // Handle incoming data
            if header.flags.ack {
                // Update send window
                conn.send_unack = header.ack_num;
                conn.send_window = header.window_size;
            }

            // Process incoming data
            if !payload.is_empty() && header.seq_num == conn.recv_next {
                conn.rx_buffer.extend(payload);
                conn.recv_next = conn.recv_next.wrapping_add(payload.len() as u32);

                log::debug!("TCP: Received {} bytes of data", payload.len());

                // Send ACK
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv()
                wake_connection_waiters(conn);
            }

            // Handle FIN
            if header.flags.fin {
                conn.recv_next = conn.recv_next.wrapping_add(1); // FIN consumes sequence
                conn.state = TcpState::CloseWait;

                log::debug!("TCP: Received FIN, moving to CLOSE_WAIT");

                // Send ACK for FIN
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv() so they see EOF
                wake_connection_waiters(conn);
            }

            if header.flags.rst {
                log::info!("TCP: Connection reset by peer");
                conn.state = TcpState::Closed;
                // Wake threads blocked in recv() so they see the error
                wake_connection_waiters(conn);
            }
        }
        TcpState::FinWait1 => {
            // In FinWait1, we've sent FIN but can still receive data
            if header.flags.ack {
                conn.send_unack = header.ack_num;
                // Our FIN was ACKed, move to FinWait2
                if !header.flags.fin {
                    conn.state = TcpState::FinWait2;
                }
            }

            // Process incoming data (half-close: we can still receive)
            if !payload.is_empty() && header.seq_num == conn.recv_next {
                conn.rx_buffer.extend(payload);
                conn.recv_next = conn.recv_next.wrapping_add(payload.len() as u32);

                log::debug!("TCP: Received {} bytes of data in FinWait1", payload.len());

                // Send ACK for data
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv()
                wake_connection_waiters(conn);
            }

            // Handle FIN from peer (simultaneous close or peer closing after we did)
            if header.flags.fin {
                conn.recv_next = conn.recv_next.wrapping_add(1);
                conn.state = TcpState::TimeWait;

                // Send ACK for FIN
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv() so they see EOF
                wake_connection_waiters(conn);
            }
        }
        TcpState::FinWait2 => {
            // In FinWait2, our FIN was ACKed but peer hasn't sent FIN yet
            // We can still receive data (half-close)
            if !payload.is_empty() && header.seq_num == conn.recv_next {
                conn.rx_buffer.extend(payload);
                conn.recv_next = conn.recv_next.wrapping_add(payload.len() as u32);

                log::debug!("TCP: Received {} bytes of data in FinWait2", payload.len());

                // Send ACK for data
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv()
                wake_connection_waiters(conn);
            }

            // Handle FIN from peer
            if header.flags.fin {
                conn.recv_next = conn.recv_next.wrapping_add(1);
                conn.state = TcpState::TimeWait;

                // Send ACK for FIN
                send_tcp_packet(
                    config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::ack(),
                    conn.recv_window,
                    &[],
                );

                // Wake threads blocked in recv() so they see EOF
                wake_connection_waiters(conn);
            }
        }
        TcpState::CloseWait => {
            // Waiting for application to close
        }
        TcpState::LastAck => {
            if header.flags.ack {
                conn.state = TcpState::Closed;
                log::debug!("TCP: Connection closed");
            }
        }
        TcpState::TimeWait => {
            // Wait for 2MSL then close (simplified: just mark as closed)
            conn.state = TcpState::Closed;
        }
        _ => {}
    }
}

/// Handle SYN packet for a listening socket
fn handle_syn_for_listener(
    listener: &mut ListenSocket,
    src_ip: [u8; 4],
    header: &TcpHeader,
    config: &super::NetConfig,
) {
    if listener.pending.len() >= listener.backlog {
        log::warn!("TCP: Backlog full, dropping SYN");
        return;
    }

    let send_isn = generate_isn();

    // Add to pending queue
    listener.pending.push_back(PendingConnection {
        remote_ip: src_ip,
        remote_port: header.src_port,
        recv_initial: header.seq_num,
        send_initial: send_isn,
        ack_received: false,
        early_data: Vec::new(),
        recv_next: header.seq_num.wrapping_add(1), // +1 for SYN
    });

    log::debug!("TCP: SYN received, sending SYN+ACK");

    // Send SYN+ACK
    send_tcp_packet(
        config,
        src_ip,
        listener.local_port,
        header.src_port,
        send_isn,
        header.seq_num.wrapping_add(1),
        TcpFlags::syn_ack(),
        65535,
        &[],
    );

    // Wake threads blocked in accept() - connection is now pending
    wake_accept_waiters(listener);
}

/// Send a RST packet
fn send_rst(config: &super::NetConfig, dst_ip: [u8; 4], header: &TcpHeader) {
    let seq = if header.flags.ack { header.ack_num } else { 0 };
    let ack = header.seq_num.wrapping_add(1);

    let mut flags = TcpFlags::rst();
    if !header.flags.ack {
        flags.ack = true;
    }

    send_tcp_packet(
        config,
        dst_ip,
        header.dst_port,
        header.src_port,
        seq,
        ack,
        flags,
        0,
        &[],
    );
}

/// Send a TCP packet
pub fn send_tcp_packet(
    config: &super::NetConfig,
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) {
    let packet = build_tcp_packet_with_checksum(
        config.ip_addr,
        dst_ip,
        src_port,
        dst_port,
        seq_num,
        ack_num,
        flags,
        window_size,
        payload,
    );

    if let Err(e) = super::send_ipv4(dst_ip, PROTOCOL_TCP, &packet) {
        log::warn!("TCP: Failed to send packet: {}", e);
    }
}

/// Initiate a TCP connection (called from connect syscall)
pub fn tcp_connect(
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    owner_pid: crate::process::process::ProcessId,
) -> Result<ConnectionId, &'static str> {
    let config = super::config();

    // Normalize loopback addresses (127.x.x.x) to our own IP
    // This ensures connection lookups work when SYN-ACK replies come from our IP
    let effective_remote = if remote_ip[0] == 127 {
        config.ip_addr
    } else {
        remote_ip
    };

    let conn_id = ConnectionId {
        local_ip: config.ip_addr,
        local_port,
        remote_ip: effective_remote,
        remote_port,
    };

    let isn = generate_isn();
    let conn = TcpConnection::new_outgoing(conn_id, isn, owner_pid);

    // Add connection to table
    {
        let mut connections = TCP_CONNECTIONS.lock();
        if connections.contains_key(&conn_id) {
            return Err("Connection already exists");
        }
        connections.insert(conn_id, conn);
    }

    // Send SYN
    send_tcp_packet(
        &config,
        remote_ip,
        local_port,
        remote_port,
        isn,
        0,
        TcpFlags::syn(),
        65535,
        &[],
    );

    log::info!("TCP: Connecting to {}.{}.{}.{}:{}",
        remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3], remote_port);

    Ok(conn_id)
}

/// Start listening on a port (called from listen syscall)
pub fn tcp_listen(
    local_port: u16,
    backlog: usize,
    owner_pid: crate::process::process::ProcessId,
) -> Result<(), &'static str> {
    let config = super::config();

    let mut listeners = TCP_LISTENERS.lock();
    if listeners.contains_key(&local_port) {
        return Err("Port already in use");
    }

    listeners.insert(local_port, ListenSocket {
        local_ip: config.ip_addr,
        local_port,
        backlog,
        pending: VecDeque::new(),
        owner_pid,
        waiting_threads: Mutex::new(Vec::new()),
    });

    log::info!("TCP: Listening on port {}", local_port);

    Ok(())
}

/// Accept a pending connection (called from accept syscall)
pub fn tcp_accept(local_port: u16) -> Option<ConnectionId> {
    let config = super::config();

    let pending = {
        let mut listeners = TCP_LISTENERS.lock();
        let listener = listeners.get_mut(&local_port)?;
        listener.pending.pop_front()?
    };

    let conn_id = ConnectionId {
        local_ip: config.ip_addr,
        local_port,
        remote_ip: pending.remote_ip,
        remote_port: pending.remote_port,
    };

    // Get owner PID from listener
    let owner_pid = {
        let listeners = TCP_LISTENERS.lock();
        listeners.get(&local_port)?.owner_pid
    };

    // Create connection - state depends on whether ACK was already received
    let mut conn = TcpConnection::new_outgoing(conn_id, pending.send_initial, owner_pid);
    if pending.ack_received {
        // 3-way handshake complete, connection is established
        conn.state = TcpState::Established;
        // send_next should be incremented past our SYN+ACK
        conn.send_next = pending.send_initial.wrapping_add(1);
        conn.send_unack = conn.send_next;
        log::info!("TCP: Connection established (server, ACK already received)");
    } else {
        // Still waiting for ACK
        conn.state = TcpState::SynReceived;
    }
    conn.recv_initial = pending.recv_initial;
    // Use the recv_next from pending, which accounts for any early data
    conn.recv_next = pending.recv_next;
    // Copy any early data that arrived before accept()
    if !pending.early_data.is_empty() {
        for byte in pending.early_data.iter() {
            conn.rx_buffer.push_back(*byte);
        }
        log::debug!("TCP: Copied {} bytes of early data to connection rx_buffer", pending.early_data.len());
    }

    let mut connections = TCP_CONNECTIONS.lock();
    connections.insert(conn_id, conn);

    log::debug!("TCP: Accepted connection from {}.{}.{}.{}:{}",
        pending.remote_ip[0], pending.remote_ip[1], pending.remote_ip[2], pending.remote_ip[3],
        pending.remote_port);

    Some(conn_id)
}

/// Send data on a connection
pub fn tcp_send(conn_id: &ConnectionId, data: &[u8]) -> Result<usize, &'static str> {
    let config = super::config();

    let mut connections = TCP_CONNECTIONS.lock();
    let conn = connections.get_mut(conn_id).ok_or("Connection not found")?;

    if conn.send_shutdown {
        return Err("Connection shutdown for writing");
    }

    if conn.state != TcpState::Established {
        return Err("Connection not established");
    }

    // For simplicity, send all data in one segment (up to MSS)
    let send_len = data.len().min(conn.mss as usize);

    send_tcp_packet(
        &config,
        conn.id.remote_ip,
        conn.id.local_port,
        conn.id.remote_port,
        conn.send_next,
        conn.recv_next,
        TcpFlags::ack_psh(),
        conn.recv_window,
        &data[..send_len],
    );

    conn.send_next = conn.send_next.wrapping_add(send_len as u32);

    Ok(send_len)
}

/// Receive data from a connection
pub fn tcp_recv(conn_id: &ConnectionId, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut connections = TCP_CONNECTIONS.lock();
    let conn = connections.get_mut(conn_id).ok_or("Connection not found")?;

    // Check recv_shutdown flag - if set, return EOF immediately
    if conn.recv_shutdown {
        return Ok(0); // EOF - user called SHUT_RD
    }

    if conn.rx_buffer.is_empty() {
        // Return EOF if connection is closing/closed
        if matches!(conn.state, TcpState::CloseWait | TcpState::Closed | TcpState::TimeWait) {
            return Ok(0); // EOF
        }
        return Err("No data available");
    }

    let read_len = buf.len().min(conn.rx_buffer.len());
    for i in 0..read_len {
        buf[i] = conn.rx_buffer.pop_front().unwrap();
    }

    Ok(read_len)
}

/// Check if a connection is established
pub fn tcp_is_established(conn_id: &ConnectionId) -> bool {
    let connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get(conn_id) {
        let is_established = conn.state == TcpState::Established;
        if !is_established {
            log::debug!(
                "TCP_IS_ESTABLISHED: conn_id={{local={}:{}, remote={}:{}}} found but state={:?}",
                conn_id.local_ip[3], conn_id.local_port,
                conn_id.remote_ip[3], conn_id.remote_port,
                conn.state
            );
        }
        is_established
    } else {
        // Log the conn_id we're looking for and what's actually in the map
        log::warn!(
            "TCP_IS_ESTABLISHED: conn_id={{local={}:{}, remote={}:{}}} NOT FOUND (total connections: {})",
            conn_id.local_ip[3], conn_id.local_port,
            conn_id.remote_ip[3], conn_id.remote_port,
            connections.len()
        );
        // Also log what connections DO exist for debugging
        for (k, v) in connections.iter() {
            log::warn!(
                "  existing: local={}:{}, remote={}:{}, state={:?}",
                k.local_ip[3], k.local_port,
                k.remote_ip[3], k.remote_port,
                v.state
            );
        }
        false
    }
}

/// Check if a connection failed (reset or closed during handshake)
pub fn tcp_is_failed(conn_id: &ConnectionId) -> bool {
    let connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get(conn_id) {
        matches!(conn.state, TcpState::Closed | TcpState::TimeWait)
    } else {
        true // Connection not found = failed
    }
}

/// Shutdown part of a full-duplex connection
/// shut_rd: stop receiving
/// shut_wr: stop sending (also sends FIN to remote)
pub fn tcp_shutdown(conn_id: &ConnectionId, shut_rd: bool, shut_wr: bool) {
    let config = super::config();

    let mut connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get_mut(conn_id) {
        if shut_rd {
            conn.recv_shutdown = true;
        }
        if shut_wr && !conn.send_shutdown {
            conn.send_shutdown = true;
            // Send FIN to signal we're done sending
            if conn.state == TcpState::Established {
                send_tcp_packet(
                    &config,
                    conn.id.remote_ip,
                    conn.id.local_port,
                    conn.id.remote_port,
                    conn.send_next,
                    conn.recv_next,
                    TcpFlags::fin_ack(),
                    conn.recv_window,
                    &[],
                );
                conn.send_next = conn.send_next.wrapping_add(1);
                conn.state = TcpState::FinWait1;
            }
        }
    }
}

/// Increment reference count on a TCP connection (called during fork)
pub fn tcp_add_ref(conn_id: &ConnectionId) {
    let connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get(conn_id) {
        conn.refcount.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
}

/// Close a connection (decrement refcount, only actually close when last reference dropped)
pub fn tcp_close(conn_id: &ConnectionId) -> Result<(), &'static str> {
    let config = super::config();

    let mut connections = TCP_CONNECTIONS.lock();
    let conn = connections.get_mut(conn_id).ok_or("Connection not found")?;

    // Decrement reference count
    let old_count = conn.refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);

    // Only actually close when last reference is dropped
    if old_count > 1 {
        return Ok(());
    }

    // Last reference - actually close the connection
    match conn.state {
        TcpState::Established => {
            // Send FIN
            send_tcp_packet(
                &config,
                conn.id.remote_ip,
                conn.id.local_port,
                conn.id.remote_port,
                conn.send_next,
                conn.recv_next,
                TcpFlags::fin_ack(),
                conn.recv_window,
                &[],
            );
            conn.send_next = conn.send_next.wrapping_add(1); // FIN consumes sequence
            conn.state = TcpState::FinWait1;
        }
        TcpState::CloseWait => {
            // Send FIN
            send_tcp_packet(
                &config,
                conn.id.remote_ip,
                conn.id.local_port,
                conn.id.remote_port,
                conn.send_next,
                conn.recv_next,
                TcpFlags::fin_ack(),
                conn.recv_window,
                &[],
            );
            conn.send_next = conn.send_next.wrapping_add(1);
            conn.state = TcpState::LastAck;
        }
        TcpState::Closed => {
            // Already closed, remove from table
            connections.remove(conn_id);
        }
        _ => {
            // Other states - just mark as closed
            conn.state = TcpState::Closed;
        }
    }

    Ok(())
}

/// Check if there's a pending connection to accept
pub fn tcp_has_pending(local_port: u16) -> bool {
    let listeners = TCP_LISTENERS.lock();
    listeners.get(&local_port)
        .map(|l| !l.pending.is_empty())
        .unwrap_or(false)
}

/// Get connection state for debugging and introspection
#[allow(dead_code)] // Part of TCP debugging API
pub fn tcp_get_state(conn_id: &ConnectionId) -> Option<TcpState> {
    let connections = TCP_CONNECTIONS.lock();
    connections.get(conn_id).map(|c| c.state)
}

// ============================================================================
// Blocking I/O support - waiter registration and wakeup
// ============================================================================

/// Register a thread as waiting for incoming connections on a listening socket (accept)
pub fn tcp_register_accept_waiter(local_port: u16, thread_id: u64) {
    let listeners = TCP_LISTENERS.lock();
    if let Some(listener) = listeners.get(&local_port) {
        let mut waiting = listener.waiting_threads.lock();
        if !waiting.contains(&thread_id) {
            waiting.push(thread_id);
            log::trace!("TCP: Thread {} registered as accept waiter on port {}", thread_id, local_port);
        }
    }
}

/// Unregister a thread from waiting for incoming connections
pub fn tcp_unregister_accept_waiter(local_port: u16, thread_id: u64) {
    let listeners = TCP_LISTENERS.lock();
    if let Some(listener) = listeners.get(&local_port) {
        let mut waiting = listener.waiting_threads.lock();
        waiting.retain(|&id| id != thread_id);
    }
}

/// Register a thread as waiting for data/state change on a connection (recv/connect)
pub fn tcp_register_recv_waiter(conn_id: &ConnectionId, thread_id: u64) {
    let connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get(conn_id) {
        let mut waiting = conn.waiting_threads.lock();
        if !waiting.contains(&thread_id) {
            waiting.push(thread_id);
            log::trace!("TCP: Thread {} registered as recv waiter", thread_id);
        }
    }
}

/// Unregister a thread from waiting for data on a connection
pub fn tcp_unregister_recv_waiter(conn_id: &ConnectionId, thread_id: u64) {
    let connections = TCP_CONNECTIONS.lock();
    if let Some(conn) = connections.get(conn_id) {
        let mut waiting = conn.waiting_threads.lock();
        waiting.retain(|&id| id != thread_id);
    }
}

/// Check if a connection has data available for recv
pub fn tcp_has_data(conn_id: &ConnectionId) -> bool {
    let connections = TCP_CONNECTIONS.lock();
    connections.get(conn_id)
        .map(|c| !c.rx_buffer.is_empty())
        .unwrap_or(false)
}

/// Wake all threads waiting on a listening socket (called when SYN arrives)
fn wake_accept_waiters(listener: &ListenSocket) {
    let readers: Vec<u64> = {
        let mut waiting = listener.waiting_threads.lock();
        waiting.drain(..).collect()
    };

    if !readers.is_empty() {
        crate::task::scheduler::with_scheduler(|sched| {
            for thread_id in &readers {
                sched.unblock(*thread_id);
            }
        });
        crate::task::scheduler::set_need_resched();
        log::debug!("TCP: Woke {} accept waiters", readers.len());
    }
}

/// Wake all threads waiting on a connection (called when data arrives or state changes)
fn wake_connection_waiters(conn: &TcpConnection) {
    let readers: Vec<u64> = {
        let mut waiting = conn.waiting_threads.lock();
        waiting.drain(..).collect()
    };

    if !readers.is_empty() {
        crate::task::scheduler::with_scheduler(|sched| {
            for thread_id in &readers {
                sched.unblock(*thread_id);
            }
        });
        crate::task::scheduler::set_need_resched();
        log::debug!("TCP: Woke {} connection waiters", readers.len());
    }
}
