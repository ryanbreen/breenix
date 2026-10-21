//! SSH binary packet protocol (RFC 4253 §6)
//!
//! Handles reading and writing SSH packets over a TCP stream, including
//! padding, sequence numbers, and (after key exchange) encryption and MAC.

use crate::crypto::rand::Csprng;
use crate::errno::Errno;
use crate::error::Error;
use crate::socket;
use crate::types::Fd;

use super::cipher::SshCipher;

/// Maximum SSH packet payload size (256 KB, generous for interactive sessions).
const MAX_PACKET_SIZE: usize = 256 * 1024;

/// Minimum padding length per RFC 4253.
const MIN_PADDING: usize = 4;

/// SSH packet reader/writer with sequence tracking.
pub struct PacketIo {
    fd: Fd,
    rng: Csprng,
    /// Sequence number for outgoing packets.
    seq_send: u32,
    /// Sequence number for incoming packets.
    seq_recv: u32,
    /// Cipher state for outgoing packets (None = plaintext).
    cipher_send: Option<SshCipher>,
    /// Cipher state for incoming packets (None = plaintext).
    cipher_recv: Option<SshCipher>,
    /// An incomplete wire packet survives a nonblocking relay read. Its first
    /// block is decrypted once, when the length becomes available.
    incoming: Vec<u8>,
    incoming_length: Option<usize>,
}

impl PacketIo {
    /// Create a new packet I/O handler for an SSH connection.
    pub fn new(fd: Fd) -> Self {
        Self {
            fd,
            rng: Csprng::new(),
            seq_send: 0,
            seq_recv: 0,
            cipher_send: None,
            cipher_recv: None,
            incoming: Vec::new(),
            incoming_length: None,
        }
    }

    /// Install encryption for outgoing packets.
    pub fn set_cipher_send(&mut self, cipher: SshCipher) {
        self.cipher_send = Some(cipher);
    }

    /// Install encryption for incoming packets.
    pub fn set_cipher_recv(&mut self, cipher: SshCipher) {
        self.cipher_recv = Some(cipher);
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
    fn write_all(&self, buf: &[u8]) -> Result<(), Error> {
        let mut offset = 0;
        while offset < buf.len() {
            let n = socket::send(self.fd, &buf[offset..])?;
            if n == 0 {
                return Err(Error::Os(Errno::EIO));
            }
            offset += n;
        }
        Ok(())
    }

    /// Send an SSH packet with the given payload.
    ///
    /// Constructs the binary packet with proper padding, optionally encrypts
    /// and computes MAC, then writes to the socket.
    pub fn send_packet(&mut self, payload: &[u8]) -> Result<(), Error> {
        let block_size = if self.cipher_send.is_some() { 16 } else { 8 };

        // Calculate padding: total of (4 + 1 + payload + padding) must be
        // a multiple of block_size, with padding >= MIN_PADDING.
        let gcm = self.cipher_send.as_ref().is_some_and(SshCipher::is_gcm);
        let unpadded = if gcm { 1 + payload.len() } else { 4 + 1 + payload.len() };
        let mut padding = block_size - (unpadded % block_size);
        if padding < MIN_PADDING {
            padding += block_size;
        }

        let packet_length = 1 + payload.len() + padding;

        // Build the unencrypted packet
        let mut packet = Vec::with_capacity(4 + packet_length);
        packet.extend_from_slice(&(packet_length as u32).to_be_bytes());
        packet.push(padding as u8);
        packet.extend_from_slice(payload);

        // Random padding
        let mut pad = vec![0u8; padding];
        self.rng.fill(&mut pad);
        packet.extend_from_slice(&pad);

        if let Some(ref mut cipher) = self.cipher_send {
            let mac = cipher.seal(self.seq_send, &mut packet)?;
            self.write_all(&packet)?;
            self.write_all(&mac)?;
        } else {
            // Plaintext
            self.write_all(&packet)?;
        }

        self.seq_send = self.seq_send.wrapping_add(1);
        Ok(())
    }

    /// Receive an SSH packet and return the payload.
    ///
    /// Reads the binary packet from the socket, optionally decrypts and
    /// verifies the MAC.
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

    /// Read only bytes already available, retaining a fragmented packet so
    /// the relay can keep draining process output and delivering process input.
    pub fn try_recv_packet(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let flags = crate::io::fcntl_getfl(self.fd)? as i32;
        crate::io::fcntl_setfl(self.fd, flags | crate::io::status_flags::O_NONBLOCK)?;
        let result = self.read_packet();
        crate::io::fcntl_setfl(self.fd, flags)?;
        result
    }

    fn read_packet(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let gcm = self.cipher_recv.as_ref().is_some_and(SshCipher::is_gcm);
        let block_size = if gcm { 4 } else if self.cipher_recv.is_some() { 16 } else { 8 };
        let mac_size = if gcm { 16 } else if self.cipher_recv.is_some() { 32 } else { 0 };
        loop {
            let needed = self.incoming_length.map(|n| 4 + n + mac_size).unwrap_or(block_size);
            while self.incoming.len() < needed {
                let filled = self.incoming.len();
                self.incoming.resize(needed, 0);
                let result = socket::recv(self.fd, &mut self.incoming[filled..]);
                let n = match result {
                    Ok(0) => return Err(Error::Os(Errno::ECONNRESET)),
                    Ok(n) => n,
                    Err(e) => {
                        self.incoming.truncate(filled);
                        match e {
                            Error::Os(Errno::EINTR) => continue,
                            Error::Os(Errno::EAGAIN) => return Ok(None),
                            _ => return Err(e),
                        }
                    }
                };
                self.incoming.truncate(filled + n);
            }
            if self.incoming_length.is_none() {
                if let Some(ref mut cipher) = self.cipher_recv {
                    cipher.decrypt(&mut self.incoming);
                }
                let length = u32::from_be_bytes(self.incoming[..4].try_into().unwrap()) as usize;
                let alignment = if self.cipher_recv.is_some() { 16 } else { 8 };
                if length > MAX_PACKET_SIZE || length < 1 + MIN_PADDING
                    || (length + if gcm { 0 } else { 4 }) % alignment != 0
                {
                    return Err(Error::Os(Errno::EMSGSIZE));
                }
                self.incoming_length = Some(length);
                continue;
            }
            let length = self.incoming_length.take().unwrap();
            let mut full = core::mem::take(&mut self.incoming);
            let (packet, mac) = full.split_at_mut(4 + length);
            if let Some(ref mut cipher) = self.cipher_recv {
                cipher.decrypt(&mut packet[block_size..]);
                cipher.open(self.seq_recv, packet, mac)?;
            }
            let padding = packet[4] as usize;
            if padding < MIN_PADDING || padding >= length {
                return Err(Error::Os(Errno::EBADMSG));
            }
            let payload = packet[5..4 + length - padding].to_vec();
            self.seq_recv = self.seq_recv.wrapping_add(1);
            return Ok(Some(payload));
        }
    }

    /// Read a line terminated by \r\n (for version string exchange).
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
        self.write_all(&buf)
    }

    /// Get the underlying file descriptor.
    pub fn fd(&self) -> Fd {
        self.fd
    }
}
