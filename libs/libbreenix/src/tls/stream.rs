//! TLS stream: encrypted wrapper around a TCP connection
//!
//! Provides [`TlsStream`], which performs the TLS 1.2 handshake over an existing
//! TCP socket and then transparently encrypts/decrypts application data.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use super::handshake::{perform_handshake, HandshakeError, HandshakeResult};
use super::record::{
    RecordDecryptor, RecordEncryptor, RecordError, CONTENT_ALERT, CONTENT_APPLICATION_DATA,
    MAX_RECORD_SIZE, RECORD_HEADER_SIZE,
};
use crate::socket::{recv, send};
use crate::types::Fd;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during TLS stream operations.
#[derive(Debug)]
pub enum TlsError {
    /// The TLS handshake failed.
    HandshakeError(HandshakeError),
    /// A TLS record-layer error occurred.
    RecordError(RecordError),
    /// An underlying I/O (socket) error occurred.
    IoError,
    /// The peer closed the connection (or we did).
    ConnectionClosed,
    /// A TLS alert was received: `(level, description)`.
    AlertReceived(u8, u8),
}

impl From<HandshakeError> for TlsError {
    fn from(e: HandshakeError) -> Self {
        TlsError::HandshakeError(e)
    }
}

impl From<RecordError> for TlsError {
    fn from(e: RecordError) -> Self {
        TlsError::RecordError(e)
    }
}

// ---------------------------------------------------------------------------
// Socket I/O helpers
// ---------------------------------------------------------------------------

/// Read exactly `len` bytes from the socket into `buf[..len]`.
///
/// Loops over partial `recv` calls until the full amount has been read.
/// Returns `TlsError::ConnectionClosed` if the peer closes before we get
/// all requested bytes, and `TlsError::IoError` on socket errors.
fn read_exact(fd: Fd, buf: &mut [u8], len: usize) -> Result<(), TlsError> {
    let mut offset = 0;
    while offset < len {
        match recv(fd, &mut buf[offset..len]) {
            Ok(0) => return Err(TlsError::ConnectionClosed),
            Ok(n) => offset += n,
            Err(_) => return Err(TlsError::IoError),
        }
    }
    Ok(())
}

/// Send all bytes in `data` to the socket.
///
/// Loops over partial `send` calls until every byte has been written.
fn send_all(fd: Fd, data: &[u8]) -> Result<(), TlsError> {
    let mut offset = 0;
    while offset < data.len() {
        match send(fd, &data[offset..]) {
            Ok(0) => return Err(TlsError::ConnectionClosed),
            Ok(n) => offset += n,
            Err(_) => return Err(TlsError::IoError),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TlsStream
// ---------------------------------------------------------------------------

/// An encrypted TLS 1.2 stream over a TCP socket.
///
/// Created via [`TlsStream::connect`], which performs the full handshake and
/// returns a stream ready for encrypted reads and writes.
pub struct TlsStream {
    fd: Fd,
    encryptor: RecordEncryptor,
    decryptor: RecordDecryptor,
    /// Buffered decrypted data from partial record reads.
    read_buf: Vec<u8>,
    /// Current read position inside `read_buf`.
    read_pos: usize,
    /// Whether the connection has been closed.
    closed: bool,
}

impl TlsStream {
    /// Establish a TLS connection over an existing TCP socket.
    ///
    /// Performs the full TLS 1.2 handshake with the given `hostname`.
    /// If `insecure` is `true`, certificate verification is skipped (useful
    /// for development/testing only).
    ///
    /// On success, returns a [`TlsStream`] ready for encrypted I/O.
    pub fn connect(fd: Fd, hostname: &str, insecure: bool) -> Result<TlsStream, TlsError> {
        let result: HandshakeResult = perform_handshake(fd, hostname, insecure)?;
        Ok(TlsStream {
            fd,
            encryptor: result.encryptor,
            decryptor: result.decryptor,
            read_buf: Vec::new(),
            read_pos: 0,
            closed: false,
        })
    }

    /// Read decrypted data from the TLS connection.
    ///
    /// Copies up to `buf.len()` bytes of plaintext into `buf` and returns the
    /// number of bytes actually read.  Returns `Ok(0)` when the connection
    /// has been cleanly closed.
    ///
    /// Internally this reads full TLS records and buffers any excess data so
    /// that subsequent calls can return it without another socket read.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        if self.closed {
            return Ok(0);
        }

        // 1. Return buffered data if available.
        if self.read_pos < self.read_buf.len() {
            let available = self.read_buf.len() - self.read_pos;
            let to_copy = available.min(buf.len());
            buf[..to_copy].copy_from_slice(&self.read_buf[self.read_pos..self.read_pos + to_copy]);
            self.read_pos += to_copy;

            // If we consumed the whole buffer, reset it.
            if self.read_pos == self.read_buf.len() {
                self.read_buf.clear();
                self.read_pos = 0;
            }
            return Ok(to_copy);
        }

        // 2. Read a full TLS record from the socket.
        loop {
            // 2a. Read the 5-byte record header.
            let mut header = [0u8; RECORD_HEADER_SIZE];
            read_exact(self.fd, &mut header, RECORD_HEADER_SIZE)?;

            let content_type = header[0];
            let payload_len = u16::from_be_bytes([header[3], header[4]]) as usize;

            // Sanity-check the payload length.
            if payload_len > MAX_RECORD_SIZE + 256 {
                // 256 bytes of overhead should be more than enough for any
                // TLS 1.2 cipher suite (GCM adds 8-byte nonce + 16-byte tag).
                return Err(TlsError::RecordError(RecordError::InvalidRecord));
            }

            // 2b. Read the payload.
            let mut payload = vec![0u8; payload_len];
            read_exact(self.fd, &mut payload, payload_len)?;

            // 2c. Dispatch on content type.
            match content_type {
                CONTENT_APPLICATION_DATA => {
                    // Decrypt the record.
                    let mut pt_buf = vec![0u8; payload_len];
                    let pt_len = self
                        .decryptor
                        .decrypt_record(CONTENT_APPLICATION_DATA, &payload, &mut pt_buf)?;

                    if pt_len == 0 {
                        // Empty application data -- keep reading.
                        continue;
                    }

                    // Copy as much as fits into the caller's buffer.
                    let to_copy = pt_len.min(buf.len());
                    buf[..to_copy].copy_from_slice(&pt_buf[..to_copy]);

                    // Buffer the rest for future reads.
                    if to_copy < pt_len {
                        self.read_buf = pt_buf[to_copy..pt_len].to_vec();
                        self.read_pos = 0;
                    }

                    return Ok(to_copy);
                }
                CONTENT_ALERT => {
                    // Alerts are 2 bytes: level, description.
                    // They may or may not be encrypted depending on the state.
                    let (alert_level, alert_desc) = if payload_len == 2 {
                        // Unencrypted alert (before ChangeCipherSpec, or in
                        // some edge cases).
                        (payload[0], payload[1])
                    } else {
                        // Encrypted alert -- decrypt it.
                        let mut alert_buf = [0u8; 256];
                        let alert_len = self
                            .decryptor
                            .decrypt_record(CONTENT_ALERT, &payload, &mut alert_buf)?;
                        if alert_len < 2 {
                            return Err(TlsError::RecordError(RecordError::InvalidRecord));
                        }
                        (alert_buf[0], alert_buf[1])
                    };

                    // close_notify: level=1 (warning), description=0
                    if alert_level == 1 && alert_desc == 0 {
                        self.closed = true;
                        return Ok(0);
                    }

                    return Err(TlsError::AlertReceived(alert_level, alert_desc));
                }
                _ => {
                    // Unknown / unexpected content type -- skip and try again.
                    // In a stricter implementation we would error out, but for
                    // robustness we silently ignore unexpected record types
                    // (e.g. heartbeat, CCS after handshake, etc.).
                    continue;
                }
            }
        }
    }

    /// Write data to the TLS connection (encrypts and sends).
    ///
    /// The data is split into TLS records of at most [`MAX_RECORD_SIZE`]
    /// (16 384) bytes each.  Returns the total number of plaintext bytes
    /// written (always `data.len()` on success).
    pub fn write(&mut self, data: &[u8]) -> Result<usize, TlsError> {
        if self.closed {
            return Err(TlsError::ConnectionClosed);
        }

        // Buffer large enough for one encrypted record:
        // header(5) + explicit_nonce(8) + max_plaintext(16384) + tag(16)
        let mut record_buf = vec![0u8; RECORD_HEADER_SIZE + 8 + MAX_RECORD_SIZE + 16];

        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + MAX_RECORD_SIZE).min(data.len());
            let chunk = &data[offset..end];

            // Encrypt the chunk as an APPLICATION_DATA record.
            let enc_len = self
                .encryptor
                .encrypt_record(CONTENT_APPLICATION_DATA, chunk, &mut record_buf)?;

            // Send the complete record to the socket.
            send_all(self.fd, &record_buf[..enc_len])?;

            offset = end;
        }

        Ok(data.len())
    }

    /// Send a `close_notify` alert and mark the connection as closed.
    ///
    /// After calling this method, further reads will return `Ok(0)` and
    /// writes will return `Err(TlsError::ConnectionClosed)`.
    pub fn close(&mut self) -> Result<(), TlsError> {
        if self.closed {
            return Ok(());
        }

        // close_notify alert: level = 1 (warning), description = 0
        let alert_payload = [1u8, 0u8];

        // Encrypt as CONTENT_ALERT record.
        // Buffer: header(5) + explicit_nonce(8) + alert(2) + tag(16) = 31 bytes
        let mut record_buf = [0u8; 64];
        let enc_len = self
            .encryptor
            .encrypt_record(CONTENT_ALERT, &alert_payload, &mut record_buf)?;
        send_all(self.fd, &record_buf[..enc_len])?;

        self.closed = true;
        Ok(())
    }

    /// Get the underlying file descriptor.
    pub fn fd(&self) -> Fd {
        self.fd
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the TlsError variants can be constructed.
    #[test]
    fn test_tls_error_variants() {
        let _io = TlsError::IoError;
        let _closed = TlsError::ConnectionClosed;
        let _alert = TlsError::AlertReceived(2, 40); // fatal, handshake_failure
        // HandshakeError and RecordError variants are tested in their own modules.
    }

    /// Verify that TlsStream fields have the expected layout.
    ///
    /// We cannot fully construct a TlsStream without a real server, but we
    /// can verify that the struct's public API surface compiles correctly.
    #[test]
    fn test_tls_stream_struct_size() {
        // TlsStream should be a non-zero-sized type.
        assert!(core::mem::size_of::<TlsStream>() > 0);
    }
}
