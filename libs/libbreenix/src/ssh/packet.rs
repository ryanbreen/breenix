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
                return Err(Error::Os(Errno::EIO));
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
        let unpadded = 4 + 1 + payload.len();
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
            // Compute MAC over: sequence_number || unencrypted_packet
            let mac = cipher.compute_mac(self.seq_send, &packet);

            // Encrypt the packet (in place)
            cipher.encrypt(&mut packet);

            // Send encrypted packet + MAC
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
        let block_size = if self.cipher_recv.is_some() { 16 } else { 8 };

        // Read the first block to get packet_length
        let mut first_block = vec![0u8; block_size];
        self.read_exact(&mut first_block)?;

        if let Some(ref mut cipher) = self.cipher_recv {
            cipher.decrypt(&mut first_block);
        }

        let packet_length = u32::from_be_bytes([
            first_block[0],
            first_block[1],
            first_block[2],
            first_block[3],
        ]) as usize;

        if packet_length > MAX_PACKET_SIZE {
            return Err(Error::Os(Errno::EMSGSIZE));
        }

        // Total bytes after the length field
        let remaining = packet_length + 4 - block_size;

        // Read the rest of the packet
        let mut rest = vec![0u8; remaining];
        self.read_exact(&mut rest)?;

        if let Some(ref mut cipher) = self.cipher_recv {
            cipher.decrypt(&mut rest);
        }

        // Reassemble the full packet for MAC verification
        let mut full_packet = Vec::with_capacity(4 + packet_length);
        full_packet.extend_from_slice(&first_block);
        full_packet.extend_from_slice(&rest);

        if self.cipher_recv.is_some() {
            // Read MAC first (before borrowing cipher mutably)
            let mut mac_received = vec![0u8; 32]; // HMAC-SHA256 = 32 bytes
            self.read_exact(&mut mac_received)?;

            // Now verify MAC
            let cipher = self.cipher_recv.as_mut().unwrap();
            let mac_computed = cipher.verify_mac(self.seq_recv, &full_packet);
            if mac_received != mac_computed {
                return Err(Error::Os(Errno::EBADMSG));
            }
        }

        // Extract payload
        let padding_length = full_packet[4] as usize;
        let payload_length = packet_length - 1 - padding_length;
        let payload = full_packet[5..5 + payload_length].to_vec();

        self.seq_recv = self.seq_recv.wrapping_add(1);
        Ok(payload)
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

