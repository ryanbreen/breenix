//! SSH binary packet protocol (RFC 4253 §6)
//!
//! [`Framer`] owns padding, sequence numbers and packet protection, with no
//! socket: a packet is sealed in place into one contiguous frame, and
//! received bytes accumulate in one buffer from which complete packets are
//! taken, several per receive when the peer sent several. [`PacketIo`] moves
//! those bytes over the connection: one write per packet, and receives as
//! large as the buffer has room for.

use std::collections::VecDeque;

use crate::crypto::rand::Csprng;
use crate::errno::Errno;
use crate::error::Error;
use crate::socket;
use crate::types::Fd;

use super::cipher::SshCipher;

/// Maximum SSH packet length accepted from a peer (256 KB, generous for interactive sessions).
const MAX_PACKET_SIZE: usize = 256 * 1024;

/// Minimum padding length per RFC 4253.
const MIN_PADDING: usize = 4;

/// Stop pulling more relay traffic once the socket's userspace backlog is full.
const RELAY_HIGH_WATER: usize = 64 * 1024;

/// Room offered to each receive: enough for two full 32 KiB channel packets.
const RECEIVE_ROOM: usize = 64 * 1024 + 512;

/// Packet framing and protection for both directions of one connection.
pub struct Framer {
    rng: Csprng,
    seq_send: u32,
    seq_recv: u32,
    cipher_send: Option<SshCipher>,
    cipher_recv: Option<SshCipher>,
    /// Received bytes are `rx[rx_start..rx_end]`; the rest is free room.
    rx: Vec<u8>,
    rx_start: usize,
    rx_end: usize,
    /// The length of the packet at `rx_start`, once its header was read.
    rx_length: Option<usize>,
}

impl Framer {
    pub fn new(rng: Csprng) -> Self {
        Self {
            rng,
            seq_send: 0,
            seq_recv: 0,
            cipher_send: None,
            cipher_recv: None,
            rx: Vec::new(),
            rx_start: 0,
            rx_end: 0,
            rx_length: None,
        }
    }

    pub fn set_cipher_send(&mut self, cipher: SshCipher) {
        self.cipher_send = Some(cipher);
    }

    pub fn set_cipher_recv(&mut self, cipher: SshCipher) {
        self.cipher_recv = Some(cipher);
    }

    /// Test support: behave as if `packets` more packets had passed each
    /// way under the current keys, without sealing them.
    #[doc(hidden)]
    pub fn advance_sequence_numbers(&mut self, packets: u32) {
        self.seq_send = self.seq_send.wrapping_add(packets);
        self.seq_recv = self.seq_recv.wrapping_add(packets);
    }

    /// Strict KEX: both sequence numbers restart at 0 once NEWKEYS has
    /// been sent and received.
    pub fn reset_sequence_numbers(&mut self) {
        self.seq_send = 0;
        self.seq_recv = 0;
    }

    /// Append one complete wire packet for `payload` to `out`.
    pub fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        let (block, aad, tag_len) = match &self.cipher_send {
            Some(cipher) => (cipher.block_size(), cipher.length_is_aad(), cipher.tag_len()),
            None => (8, false, 0),
        };
        // The padded body (and for non-AEAD ciphers the length field too)
        // must fill whole cipher blocks, with at least MIN_PADDING bytes.
        let unpadded = if aad { 1 + payload.len() } else { 4 + 1 + payload.len() };
        let mut padding = block - unpadded % block;
        if padding < MIN_PADDING {
            padding += block;
        }
        let length = 1 + payload.len() + padding;

        let start = out.len();
        out.reserve(4 + length + tag_len);
        out.extend_from_slice(&(length as u32).to_be_bytes());
        out.push(padding as u8);
        out.extend_from_slice(payload);
        let padding_start = out.len();
        out.resize(padding_start + padding + tag_len, 0);
        self.rng.fill(&mut out[padding_start..padding_start + padding]);

        if let Some(cipher) = self.cipher_send.as_mut() {
            let (packet, tag) = out[start..].split_at_mut(4 + length);
            if let Err(error) = cipher.seal(self.seq_send, packet, tag) {
                out.truncate(start);
                return Err(error);
            }
        }
        self.seq_send = self.seq_send.wrapping_add(1);
        Ok(())
    }

    /// Free room for received bytes. Fill a prefix, then call `received`.
    pub fn receive_buffer(&mut self) -> &mut [u8] {
        if self.rx_start == self.rx_end {
            self.rx_start = 0;
            self.rx_end = 0;
        } else if self.rx_start > 0 && self.rx.len() - self.rx_end < RECEIVE_ROOM {
            self.rx.copy_within(self.rx_start..self.rx_end, 0);
            self.rx_end -= self.rx_start;
            self.rx_start = 0;
        }
        let pending = self.rx_length.map_or(0, |length| 4 + length + self.recv_tag_len());
        let room = RECEIVE_ROOM.max(pending.saturating_sub(self.rx_end - self.rx_start));
        if self.rx.len() < self.rx_end + room {
            self.rx.resize(self.rx_end + room, 0);
        }
        &mut self.rx[self.rx_end..]
    }

    /// Record `n` received bytes, stored at the start of the slice that
    /// receive_buffer returned.
    pub fn received(&mut self, n: usize) {
        self.rx_end += n;
        assert!(self.rx_end <= self.rx.len());
    }

    fn recv_tag_len(&self) -> usize {
        self.cipher_recv.as_ref().map_or(0, SshCipher::tag_len)
    }

    /// Read the header of the next packet if enough bytes have arrived.
    /// Returns the bytes the whole packet needs, header included.
    fn next_packet_size(&mut self) -> Result<Option<usize>, Error> {
        if self.rx_length.is_none() {
            let head = self.cipher_recv.as_ref().map_or(4, SshCipher::length_bytes);
            if self.rx_end - self.rx_start < head {
                return Ok(None);
            }
            let bytes = &mut self.rx[self.rx_start..self.rx_start + head];
            let length = match self.cipher_recv.as_mut() {
                Some(cipher) => cipher.packet_length(self.seq_recv, bytes),
                None => u32::from_be_bytes(bytes[..4].try_into().unwrap()),
            } as usize;
            let (alignment, aad) = match &self.cipher_recv {
                Some(cipher) => (cipher.block_size(), cipher.length_is_aad()),
                None => (8, false),
            };
            let aligned = if aad { length } else { 4 + length };
            if length > MAX_PACKET_SIZE || length < 1 + MIN_PADDING || aligned % alignment != 0 {
                return Err(Error::Os(Errno::EMSGSIZE));
            }
            self.rx_length = Some(length);
        }
        Ok(self.rx_length.map(|length| 4 + length + self.recv_tag_len()))
    }

    /// Whether a complete packet is buffered, so `open` returns it without
    /// another receive. A malformed header counts as ready: `open` reports it.
    pub fn packet_ready(&mut self) -> bool {
        match self.next_packet_size() {
            Ok(Some(size)) => self.rx_end - self.rx_start >= size,
            Ok(None) => false,
            Err(_) => true,
        }
    }

    /// Take the next complete packet's payload from the received bytes.
    pub fn open(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let Some(size) = self.next_packet_size()? else { return Ok(None) };
        if self.rx_end - self.rx_start < size {
            return Ok(None);
        }
        let length = self.rx_length.take().unwrap();
        let frame = &mut self.rx[self.rx_start..self.rx_start + size];
        let (packet, tag) = frame.split_at_mut(4 + length);
        if let Some(cipher) = self.cipher_recv.as_mut() {
            cipher.open(self.seq_recv, packet, tag)?;
        }
        let padding = packet[4] as usize;
        if padding < MIN_PADDING || padding >= length {
            return Err(Error::Os(Errno::EBADMSG));
        }
        let payload = packet[5..4 + length - padding].to_vec();
        self.rx_start += size;
        self.seq_recv = self.seq_recv.wrapping_add(1);
        Ok(Some(payload))
    }
}

/// SSH packet reader/writer over a connected socket.
pub struct PacketIo {
    fd: Fd,
    framer: Framer,
    /// Reused for each sealed packet, so sends stop allocating once it has grown.
    send_buffer: Vec<u8>,
    relay_flags: Option<i32>,
    outgoing: VecDeque<u8>,
}

impl PacketIo {
    /// Create a new packet I/O handler for an SSH connection.
    pub fn new(fd: Fd) -> Self {
        Self {
            fd,
            framer: Framer::new(Csprng::new()),
            send_buffer: Vec::new(),
            relay_flags: None,
            outgoing: VecDeque::new(),
        }
    }

    /// Install encryption for outgoing packets.
    pub fn set_cipher_send(&mut self, cipher: SshCipher) {
        self.framer.set_cipher_send(cipher);
    }

    /// Install encryption for incoming packets.
    pub fn set_cipher_recv(&mut self, cipher: SshCipher) {
        self.framer.set_cipher_recv(cipher);
    }

    /// Strict KEX: restart both sequence numbers after NEWKEYS.
    pub fn reset_sequence_numbers(&mut self) {
        self.framer.reset_sequence_numbers();
    }

    /// Read exactly `n` bytes from the socket.
    fn read_exact(&self, buf: &mut [u8]) -> Result<(), Error> {
        let mut offset = 0;
        while offset < buf.len() {
            let n = socket::recv(self.fd, &mut buf[offset..])?;
            if n == 0 {
                return Err(Error::Os(Errno::ECONNRESET));
            }
            offset += n;
        }
        Ok(())
    }

    /// Write all bytes to the socket.
    fn write_all(fd: Fd, buf: &[u8]) -> Result<(), Error> {
        let mut offset = 0;
        while offset < buf.len() {
            let n = socket::send(fd, &buf[offset..])?;
            if n == 0 {
                return Err(Error::Os(Errno::EIO));
            }
            offset += n;
        }
        Ok(())
    }

    /// Send an SSH packet with the given payload: sealed in place, then
    /// written (or queued during a relay) as one buffer.
    pub fn send_packet(&mut self, payload: &[u8]) -> Result<(), Error> {
        self.send_buffer.clear();
        self.framer.seal(payload, &mut self.send_buffer)?;
        if self.relay_flags.is_some() {
            self.outgoing.extend(self.send_buffer.iter());
        } else {
            Self::write_all(self.fd, &self.send_buffer)?;
        }
        Ok(())
    }

    /// Receive an SSH packet and return the payload.
    pub fn recv_packet(&mut self) -> Result<Vec<u8>, Error> {
        loop {
            if let Some(packet) = self.read_packet()? {
                return Ok(packet);
            }
            let mut fds = [crate::io::PollFd::new(self.fd, crate::io::poll_events::POLLIN)];
            match crate::io::poll(&mut fds, -1) {
                Ok(_) | Err(Error::Os(Errno::EINTR)) => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Whether a complete packet is already buffered. A caller that polls
    /// the socket before receiving must not wait while this is true: those
    /// bytes have left the socket.
    pub fn packet_ready(&mut self) -> bool {
        self.framer.packet_ready()
    }

    /// Read only bytes already available, retaining a fragmented packet so
    /// the relay can keep draining process output and delivering process input.
    pub fn try_recv_packet(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.relay_flags.is_some() || self.framer.packet_ready() {
            return self.read_packet();
        }
        let flags = crate::io::fcntl_getfl(self.fd)? as i32;
        crate::io::fcntl_setfl(self.fd, flags | crate::io::status_flags::O_NONBLOCK)?;
        let result = self.read_packet();
        crate::io::fcntl_setfl(self.fd, flags)?;
        result
    }

    /// Queue writes during the process relay so TCP backpressure cannot block
    /// reads from the remote peer or the terminal.
    pub fn start_relay(&mut self) -> Result<(), Error> {
        let flags = crate::io::fcntl_getfl(self.fd)? as i32;
        crate::io::fcntl_setfl(self.fd, flags | crate::io::status_flags::O_NONBLOCK)?;
        self.relay_flags = Some(flags);
        Ok(())
    }

    pub fn relay_output_pending(&self) -> bool {
        !self.outgoing.is_empty()
    }

    pub fn relay_can_queue(&self) -> bool {
        self.outgoing.len() < RELAY_HIGH_WATER
    }

    pub fn flush_relay(&mut self) -> Result<(), Error> {
        while !self.outgoing.is_empty() {
            let bytes = self.outgoing.as_slices().0;
            match socket::send(self.fd, bytes) {
                Ok(0) => return Err(Error::Os(Errno::EIO)),
                Ok(n) => { self.outgoing.drain(..n); }
                Err(Error::Os(Errno::EAGAIN | Errno::EINTR)) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn finish_relay(&mut self) -> Result<(), Error> {
        if self.relay_output_pending() {
            return Err(Error::Os(Errno::EAGAIN));
        }
        if let Some(flags) = self.relay_flags.take() {
            crate::io::fcntl_setfl(self.fd, flags)?;
        }
        Ok(())
    }

    /// The next buffered packet, receiving more bytes until one is complete.
    /// On a nonblocking socket it returns early, without a packet, when the
    /// receive would block.
    fn read_packet(&mut self) -> Result<Option<Vec<u8>>, Error> {
        loop {
            if let Some(packet) = self.framer.open()? {
                return Ok(Some(packet));
            }
            match socket::recv(self.fd, self.framer.receive_buffer()) {
                Ok(0) => return Err(Error::Os(Errno::ECONNRESET)),
                Ok(n) => self.framer.received(n),
                Err(Error::Os(Errno::EINTR)) => {}
                Err(Error::Os(Errno::EAGAIN)) => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    /// Read a line terminated by \r\n (for version string exchange).
    /// Bytes are read one at a time so the first packet's bytes stay in the socket.
    pub fn read_line(&self) -> Result<String, Error> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            self.read_exact(&mut byte)?;
            line.push(byte[0]);
            if line.len() >= 2 && line[line.len() - 2] == b'\r' && line[line.len() - 1] == b'\n' {
                // Remove \r\n
                line.pop();
                line.pop();
                return Ok(String::from_utf8_lossy(&line).into_owned());
            }
            if line.len() > 255 {
                return Err(Error::Os(Errno::EMSGSIZE));
            }
        }
    }

    /// Write a line with \r\n termination (for version string exchange).
    pub fn write_line(&self, line: &str) -> Result<(), Error> {
        let mut buf = Vec::with_capacity(line.len() + 2);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\r');
        buf.push(b'\n');
        Self::write_all(self.fd, &buf)
    }

    /// Get the underlying file descriptor.
    pub fn fd(&self) -> Fd {
        self.fd
    }
}
