//! TLS 1.2 handshake state machine for cipher suite
//! TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 (0xC02F).
//!
//! Implements the full client-side handshake: ClientHello through server
//! Finished, producing symmetric encryption keys for the application data
//! phase.

extern crate alloc;

use alloc::vec::Vec;

use crate::crypto::bignum::BigNum;
use crate::crypto::rand::Csprng;
use crate::crypto::rsa::{rsa_verify_pkcs1_sha256, RsaPublicKey};
use crate::crypto::sha256::Sha256;
use crate::crypto::x25519::{x25519, x25519_basepoint};
use crate::socket::{recv, send};
use crate::types::Fd;
use crate::x509::ca_bundle::RootStore;
use crate::x509::cert::{parse_certificate, Certificate};
use crate::x509::chain::{validate_chain, validate_chain_insecure, ChainError};

use super::prf::{compute_verify_data, derive_key_material, derive_master_secret};
use super::record::{
    self, RecordDecryptor, RecordEncryptor, CONTENT_CHANGE_CIPHER_SPEC, CONTENT_HANDSHAKE,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Handshake message types (RFC 5246 Section 7.4)
const HS_CLIENT_HELLO: u8 = 1;
const HS_SERVER_HELLO: u8 = 2;
const HS_CERTIFICATE: u8 = 11;
const HS_SERVER_KEY_EXCHANGE: u8 = 12;
const HS_SERVER_HELLO_DONE: u8 = 14;
const HS_CLIENT_KEY_EXCHANGE: u8 = 16;
const HS_FINISHED: u8 = 20;

/// Cipher suite identifier
const TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: u16 = 0xC02F;

/// Named curve identifier for X25519 (RFC 8422)
const NAMED_CURVE_X25519: u16 = 0x001D;

/// Signature algorithm: RSA PKCS1 with SHA-256 (RFC 5246 Section 7.4.1.4.1)
const SIG_RSA_PKCS1_SHA256: u16 = 0x0401;

/// Extension types
const EXT_SERVER_NAME: u16 = 0x0000;
const EXT_EC_POINT_FORMATS: u16 = 0x000B;
const EXT_SUPPORTED_GROUPS: u16 = 0x000A;
const EXT_SIGNATURE_ALGORITHMS: u16 = 0x000D;

/// Buffer size for handshake message assembly and reception.
/// 16 KB payload + TLS record header + some headroom.
const HANDSHAKE_BUF_SIZE: usize = 18000;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during the TLS handshake.
#[derive(Debug)]
pub enum HandshakeError {
    /// Socket I/O error (send or recv failed), with errno and context
    IoError,
    /// Socket I/O error with details: (errno, context string)
    IoErrorDetail(i32, &'static str),
    /// Received a message type we did not expect at this stage
    UnexpectedMessage,
    /// Server selected a cipher suite we do not support
    UnsupportedCipherSuite,
    /// Server selected a named curve we do not support
    UnsupportedCurve,
    /// Certificate chain validation failed
    CertificateError(ChainError),
    /// RSA signature on ServerKeyExchange did not verify
    SignatureVerificationFailed,
    /// Server Finished verify_data did not match
    FinishedVerifyFailed,
    /// A message exceeded our fixed-size buffer
    BufferTooSmall,
    /// ServerKeyExchange contained an invalid public key
    InvalidServerKey,
    /// Error from the record layer
    RecordError(record::RecordError),
}

impl From<record::RecordError> for HandshakeError {
    fn from(e: record::RecordError) -> Self {
        HandshakeError::RecordError(e)
    }
}

// ---------------------------------------------------------------------------
// Handshake result
// ---------------------------------------------------------------------------

/// Successful handshake result -- contains keys needed for encrypted
/// communication in both directions.
pub struct HandshakeResult {
    /// Encrypts client-to-server application data records.
    pub encryptor: RecordEncryptor,
    /// Decrypts server-to-client application data records.
    pub decryptor: RecordDecryptor,
}

// ---------------------------------------------------------------------------
// Byte encoding helpers
// ---------------------------------------------------------------------------

/// Write a big-endian u16 into `buf` at `pos`, returning the next write
/// position.
#[inline]
fn put_u16(buf: &mut [u8], pos: usize, val: u16) -> usize {
    buf[pos] = (val >> 8) as u8;
    buf[pos + 1] = val as u8;
    pos + 2
}

/// Write a big-endian 24-bit integer into `buf` at `pos`, returning the next
/// write position.
#[inline]
fn put_u24(buf: &mut [u8], pos: usize, val: u32) -> usize {
    buf[pos] = (val >> 16) as u8;
    buf[pos + 1] = (val >> 8) as u8;
    buf[pos + 2] = val as u8;
    pos + 3
}

/// Read a big-endian u16 from `buf` at `pos`.
#[inline]
fn get_u16(buf: &[u8], pos: usize) -> u16 {
    (buf[pos] as u16) << 8 | buf[pos + 1] as u16
}

/// Read a big-endian 24-bit integer from `buf` at `pos`.
#[inline]
fn get_u24(buf: &[u8], pos: usize) -> u32 {
    (buf[pos] as u32) << 16 | (buf[pos + 1] as u32) << 8 | buf[pos + 2] as u32
}

// ---------------------------------------------------------------------------
// I/O helpers
// ---------------------------------------------------------------------------

/// Send all bytes in `data` to the socket, looping on partial writes.
fn send_all(fd: Fd, data: &[u8], verbose: bool) -> Result<(), HandshakeError> {
    let mut offset = 0;
    while offset < data.len() {
        match send(fd, &data[offset..]) {
            Ok(n) => {
                if n == 0 {
                    if verbose { eprint!("* TLS send: got 0 bytes at offset {}/{}\n", offset, data.len()); }
                    return Err(HandshakeError::IoErrorDetail(0, "send returned 0"));
                }
                offset += n;
            }
            Err(e) => {
                let errno = match e {
                    crate::error::Error::Os(en) => en as i32,
                };
                if verbose { eprint!("* TLS send: error errno={} at offset {}/{}\n", errno, offset, data.len()); }
                return Err(HandshakeError::IoErrorDetail(errno, "send failed"));
            }
        }
    }
    Ok(())
}

/// Read a complete TLS record from the socket.
///
/// First reads the 5-byte record header (content_type + version + length),
/// then reads exactly `length` bytes of payload.
///
/// Returns `(content_type, header_end, payload_end)` where the payload
/// occupies `buf[5..payload_end]` and the full record is `buf[0..payload_end]`.
fn read_full_record(fd: Fd, buf: &mut [u8], verbose: bool) -> Result<(u8, usize, usize), HandshakeError> {
    // Read the 5-byte TLS record header
    let mut hdr_read = 0;
    while hdr_read < 5 {
        match recv(fd, &mut buf[hdr_read..5]) {
            Ok(n) => {
                if n == 0 {
                    if verbose { eprint!("* TLS recv: got EOF reading header at {}/5\n", hdr_read); }
                    return Err(HandshakeError::IoErrorDetail(0, "recv header EOF"));
                }
                hdr_read += n;
            }
            Err(e) => {
                let errno = match e {
                    crate::error::Error::Os(en) => en as i32,
                };
                if verbose { eprint!("* TLS recv: error errno={} reading header at {}/5\n", errno, hdr_read); }
                return Err(HandshakeError::IoErrorDetail(errno, "recv header failed"));
            }
        }
    }

    let content_type = buf[0];
    // buf[1..3] is the protocol version (ignored during read)
    let payload_len = get_u16(buf, 3) as usize;

    if verbose { eprint!("* TLS recv: record type={} payload_len={}\n", content_type, payload_len); }

    if 5 + payload_len > buf.len() {
        return Err(HandshakeError::BufferTooSmall);
    }

    // Read the full payload
    let mut payload_read = 0;
    while payload_read < payload_len {
        match recv(fd, &mut buf[5 + payload_read..5 + payload_len]) {
            Ok(n) => {
                if n == 0 {
                    if verbose { eprint!("* TLS recv: got EOF reading payload at {}/{}\n", payload_read, payload_len); }
                    return Err(HandshakeError::IoErrorDetail(0, "recv payload EOF"));
                }
                payload_read += n;
            }
            Err(e) => {
                let errno = match e {
                    crate::error::Error::Os(en) => en as i32,
                };
                if verbose { eprint!("* TLS recv: error errno={} reading payload at {}/{}\n", errno, payload_read, payload_len); }
                return Err(HandshakeError::IoErrorDetail(errno, "recv payload failed"));
            }
        }
    }

    Ok((content_type, 5, 5 + payload_len))
}

// ---------------------------------------------------------------------------
// Extension builders
// ---------------------------------------------------------------------------

/// Build the Server Name Indication (SNI) extension into `buf` starting at
/// the beginning.  Returns the number of bytes written.
///
/// Extension layout:
///   extension_type(2) = 0x0000
///   extension_data_length(2)
///     server_name_list_length(2)
///       name_type(1) = 0x00 (host_name)
///       host_name_length(2)
///       host_name(variable)
fn build_sni_extension(hostname: &str, buf: &mut [u8]) -> usize {
    let name_bytes = hostname.as_bytes();
    let host_name_length = name_bytes.len() as u16;
    // name_type(1) + host_name_length(2) + hostname
    let server_name_list_length = 1 + 2 + host_name_length;
    // server_name_list_length(2) + the list
    let extension_data_length = 2 + server_name_list_length;

    let mut pos = 0;
    pos = put_u16(buf, pos, EXT_SERVER_NAME);
    pos = put_u16(buf, pos, extension_data_length);
    pos = put_u16(buf, pos, server_name_list_length);
    buf[pos] = 0x00; // name_type = host_name
    pos += 1;
    pos = put_u16(buf, pos, host_name_length);
    buf[pos..pos + name_bytes.len()].copy_from_slice(name_bytes);
    pos += name_bytes.len();
    pos
}

/// Build the supported_groups extension (contains only x25519).
fn build_supported_groups_extension(buf: &mut [u8], pos: usize) -> usize {
    let mut p = pos;
    p = put_u16(buf, p, EXT_SUPPORTED_GROUPS);
    // extension_data_length: named_curve_list_length(2) + 1 curve(2) = 4
    p = put_u16(buf, p, 4);
    // named_curve_list_length: 1 curve * 2 bytes = 2
    p = put_u16(buf, p, 2);
    p = put_u16(buf, p, NAMED_CURVE_X25519);
    p
}

/// Build the signature_algorithms extension (contains only RSA PKCS1 SHA-256).
fn build_signature_algorithms_extension(buf: &mut [u8], pos: usize) -> usize {
    let mut p = pos;
    p = put_u16(buf, p, EXT_SIGNATURE_ALGORITHMS);
    // extension_data_length: list_length(2) + 1 algorithm(2) = 4
    p = put_u16(buf, p, 4);
    // signature_hash_algorithms_length: 1 algorithm * 2 bytes = 2
    p = put_u16(buf, p, 2);
    p = put_u16(buf, p, SIG_RSA_PKCS1_SHA256);
    p
}

/// Build the ec_point_formats extension (contains only uncompressed).
fn build_ec_point_formats_extension(buf: &mut [u8], pos: usize) -> usize {
    let mut p = pos;
    p = put_u16(buf, p, EXT_EC_POINT_FORMATS);
    // extension_data_length: formats_length(1) + 1 format(1) = 2
    p = put_u16(buf, p, 2);
    buf[p] = 1; // ec_point_format_list length
    p += 1;
    buf[p] = 0x00; // uncompressed
    p += 1;
    p
}

// ---------------------------------------------------------------------------
// Main handshake
// ---------------------------------------------------------------------------

/// Perform the full TLS 1.2 handshake over a connected TCP socket.
///
/// Negotiates cipher suite TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 and returns
/// a [`HandshakeResult`] containing the record-layer encryptor and decryptor
/// for the remainder of the connection.
///
/// If `insecure` is `true`, certificate chain validation is skipped (useful
/// for testing against self-signed servers).
pub fn perform_handshake(
    fd: Fd,
    hostname: &str,
    insecure: bool,
    verbose: bool,
) -> Result<HandshakeResult, HandshakeError> {
    if verbose { eprint!("* TLS: initializing CSPRNG...\n"); }
    let mut rng = Csprng::new();
    if verbose { eprint!("* TLS: CSPRNG ready\n"); }
    let mut transcript = Sha256::new();

    // Scratch buffers -- large enough for any single TLS record.
    // Heap-allocated to avoid stack overflow (3 × 18 KB would exceed the 64 KB
    // user stack).
    let mut send_buf = vec![0u8; HANDSHAKE_BUF_SIZE];
    let mut recv_buf = vec![0u8; HANDSHAKE_BUF_SIZE];
    if verbose { eprint!("* TLS: buffers allocated ({} bytes each)\n", HANDSHAKE_BUF_SIZE); }

    // -----------------------------------------------------------------------
    // 1. Send ClientHello
    // -----------------------------------------------------------------------

    let mut client_random = [0u8; 32];
    rng.fill(&mut client_random);

    let hs_msg_len = build_client_hello(&mut send_buf, &client_random, hostname);
    if verbose { eprint!("* TLS: sending ClientHello ({} bytes)...\n", hs_msg_len); }

    // The handshake message (type + length + body) starts at send_buf[0].
    // Hash the handshake message (NOT the record header) into the transcript.
    transcript.update(&send_buf[..hs_msg_len]);

    // Wrap in a TLS record and send.
    let record_len = wrap_handshake_record(&mut send_buf, hs_msg_len);
    send_all(fd, &send_buf[..record_len], verbose)?;
    if verbose { eprint!("* TLS: ClientHello sent\n"); }

    // -----------------------------------------------------------------------
    // 2. Receive ServerHello
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for ServerHello...\n"); }
    let (ct, _hdr_end, rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if verbose { eprint!("* TLS: received record type={} len={}\n", ct, rec_end - 5); }
    if ct != CONTENT_HANDSHAKE {
        if verbose { eprint!("* TLS: expected handshake (22), got {}\n", ct); }
        return Err(HandshakeError::UnexpectedMessage);
    }

    let payload = &recv_buf[5..rec_end];
    if payload.is_empty() || payload[0] != HS_SERVER_HELLO {
        if verbose { eprint!("* TLS: expected ServerHello (2), got msg type {}\n", if payload.is_empty() { 255 } else { payload[0] }); }
        return Err(HandshakeError::UnexpectedMessage);
    }

    // Hash the handshake message (the payload inside the record).
    transcript.update(payload);

    let server_random = parse_server_hello(payload)?;
    if verbose { eprint!("* TLS: ServerHello parsed OK\n"); }

    // -----------------------------------------------------------------------
    // 3. Receive Certificate
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for Certificate...\n"); }
    let (ct, _hdr_end, rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if verbose { eprint!("* TLS: received record type={} len={}\n", ct, rec_end - 5); }
    if ct != CONTENT_HANDSHAKE {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let payload = &recv_buf[5..rec_end];
    if payload.is_empty() || payload[0] != HS_CERTIFICATE {
        if verbose { eprint!("* TLS: expected Certificate (11), got msg type {}\n", if payload.is_empty() { 255 } else { payload[0] }); }
        return Err(HandshakeError::UnexpectedMessage);
    }

    transcript.update(payload);

    let certs = parse_certificate_message(payload)?;
    if verbose { eprint!("* TLS: parsed {} certificate(s)\n", certs.len()); }
    if verbose && !certs.is_empty() {
        let leaf = &certs[0];
        eprint!("* TLS: leaf cert SAN DNS names ({}):\n", leaf.san_dns_names.len());
        for san in &leaf.san_dns_names {
            if let Ok(s) = core::str::from_utf8(san) {
                eprint!("*   {}\n", s);
            } else {
                eprint!("*   <non-utf8: {} bytes>\n", san.len());
            }
        }
    }

    // Validate the certificate chain.
    if !insecure {
        if verbose { eprint!("* TLS: validating certificate chain...\n"); }
        let root_store = RootStore::mozilla();
        if verbose {
            eprint!("* TLS: root store has {} CA certs\n", root_store.len());
            // Count how many root certs actually parse
            let mut parse_ok = 0usize;
            let mut parse_fail = 0usize;
            for i in 0..root_store.len() {
                if root_store.get(i).is_some() {
                    parse_ok += 1;
                } else {
                    parse_fail += 1;
                }
            }
            eprint!("* TLS: root certs parseable: {} ok, {} failed\n", parse_ok, parse_fail);

            // Show issuer chain
            for (i, c) in certs.iter().enumerate() {
                let subj = crate::x509::cert::get_common_name(&c.subject)
                    .and_then(|b| core::str::from_utf8(b).ok())
                    .unwrap_or("<no CN>");
                let iss = crate::x509::cert::get_common_name(&c.issuer)
                    .and_then(|b| core::str::from_utf8(b).ok())
                    .unwrap_or("<no CN>");
                eprint!("*   cert[{}]: subj='{}' issuer='{}' is_ca={}\n", i, subj, iss, c.is_ca);
            }

            // Try to find the top cert's issuer in root store
            let top_issuer = &certs[certs.len() - 1].issuer;
            let top_iss_cn = crate::x509::cert::get_common_name(top_issuer)
                .and_then(|b| core::str::from_utf8(b).ok())
                .unwrap_or("<no CN>");
            eprint!("* TLS: looking for root with subject='{}'\n", top_iss_cn);
        }
        let now = current_unix_time();
        if verbose {
            eprint!("* TLS: current_unix_time = {}\n", now);
            if now < 1_577_836_800 {
                eprint!("* TLS: WARNING: system clock not set, skipping time validation\n");
            }
        }
        match validate_chain(&certs, hostname, root_store, now) {
            Ok(()) => {
                if verbose { eprint!("* TLS: certificate chain valid\n"); }
            }
            Err(e) => {
                if verbose { eprint!("* TLS: certificate chain INVALID: {:?}\n", e); }
                return Err(HandshakeError::CertificateError(e));
            }
        }
    } else {
        if verbose { eprint!("* TLS: skipping cert validation (insecure)\n"); }
        validate_chain_insecure(&certs, hostname).map_err(HandshakeError::CertificateError)?;
    }

    // -----------------------------------------------------------------------
    // 4. Receive ServerKeyExchange
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for ServerKeyExchange...\n"); }
    let (ct, _hdr_end, rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if verbose { eprint!("* TLS: received record type={} len={}\n", ct, rec_end - 5); }
    if ct != CONTENT_HANDSHAKE {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let payload = &recv_buf[5..rec_end];
    if payload.is_empty() || payload[0] != HS_SERVER_KEY_EXCHANGE {
        if verbose { eprint!("* TLS: expected ServerKeyExchange (12), got msg type {}\n", if payload.is_empty() { 255 } else { payload[0] }); }
        return Err(HandshakeError::UnexpectedMessage);
    }

    transcript.update(payload);

    if verbose { eprint!("* TLS: verifying server key exchange signature...\n"); }
    let server_pubkey =
        parse_and_verify_server_key_exchange(payload, &client_random, &server_random, &certs[0])?;
    if verbose { eprint!("* TLS: ServerKeyExchange verified\n"); }

    // -----------------------------------------------------------------------
    // 5. Receive ServerHelloDone
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for ServerHelloDone...\n"); }
    let (ct, _hdr_end, rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if ct != CONTENT_HANDSHAKE {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let payload = &recv_buf[5..rec_end];
    if payload.is_empty() || payload[0] != HS_SERVER_HELLO_DONE {
        return Err(HandshakeError::UnexpectedMessage);
    }

    // ServerHelloDone is just the 4-byte handshake header with length 0.
    transcript.update(payload);
    if verbose { eprint!("* TLS: ServerHelloDone received\n"); }

    // -----------------------------------------------------------------------
    // 6. Send ClientKeyExchange
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: computing X25519 key exchange...\n"); }
    let mut client_privkey = [0u8; 32];
    rng.fill(&mut client_privkey);
    if verbose { eprint!("* TLS: X25519 basepoint...\n"); }

    let client_pubkey = x25519_basepoint(&client_privkey);
    if verbose { eprint!("* TLS: X25519 shared secret...\n"); }
    let pre_master_secret = x25519(&client_privkey, &server_pubkey);
    if verbose { eprint!("* TLS: X25519 done\n"); }

    // Build ClientKeyExchange handshake message.
    // Body: pubkey_length(1) + client_pubkey(32)
    let body_len: u32 = 1 + 32;
    let mut pos = 0;
    send_buf[pos] = HS_CLIENT_KEY_EXCHANGE;
    pos += 1;
    pos = put_u24(&mut send_buf, pos, body_len);
    send_buf[pos] = 32; // public key length
    pos += 1;
    send_buf[pos..pos + 32].copy_from_slice(&client_pubkey);
    pos += 32;

    let hs_msg_len = pos;
    transcript.update(&send_buf[..hs_msg_len]);

    let record_len = wrap_handshake_record(&mut send_buf, hs_msg_len);
    if verbose {
        // Poll the socket to check connection state before sending
        let mut poll_fds = [crate::io::PollFd { fd: fd.raw() as i32, events: 0x0004 | 0x0001, revents: 0 }]; // POLLOUT|POLLIN
        match crate::io::poll(&mut poll_fds, 0) {
            Ok(n) => eprint!("* TLS: pre-send poll: n={} revents=0x{:04x} (fd={})\n", n, poll_fds[0].revents, fd.raw()),
            Err(e) => eprint!("* TLS: pre-send poll: error {:?} (fd={})\n", e, fd.raw()),
        }
        eprint!("* TLS: sending ClientKeyExchange ({} bytes)...\n", record_len);
    }
    send_all(fd, &send_buf[..record_len], verbose)?;
    if verbose { eprint!("* TLS: ClientKeyExchange sent\n"); }

    // -----------------------------------------------------------------------
    // 7. Send ChangeCipherSpec
    // -----------------------------------------------------------------------

    // ChangeCipherSpec is a single byte 0x01 in its own record type.
    // It is NOT hashed into the transcript.
    let mut ccs_buf = [0u8; 6];
    ccs_buf[0] = CONTENT_CHANGE_CIPHER_SPEC;
    ccs_buf[1] = 0x03;
    ccs_buf[2] = 0x03; // TLS 1.2
    ccs_buf[3] = 0x00;
    ccs_buf[4] = 0x01; // length = 1
    ccs_buf[5] = 0x01; // payload
    if verbose { eprint!("* TLS: sending ChangeCipherSpec...\n"); }
    send_all(fd, &ccs_buf, verbose)?;
    if verbose { eprint!("* TLS: ChangeCipherSpec sent\n"); }

    // -----------------------------------------------------------------------
    // 8. Derive keys
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: deriving session keys...\n"); }
    let master_secret =
        derive_master_secret(&pre_master_secret, &client_random, &server_random);

    let key_block = derive_key_material(&master_secret, &server_random, &client_random);

    let mut encryptor =
        RecordEncryptor::new(&key_block.client_write_key, &key_block.client_write_iv);
    let mut decryptor =
        RecordDecryptor::new(&key_block.server_write_key, &key_block.server_write_iv);
    if verbose { eprint!("* TLS: session keys derived\n"); }

    // -----------------------------------------------------------------------
    // 9. Send client Finished
    // -----------------------------------------------------------------------

    // Compute the transcript hash up to (but not including) this Finished.
    // We need to clone the hasher state since we will continue hashing after.
    let handshake_hash = clone_sha256_state(&transcript);

    let verify_data = compute_verify_data(&master_secret, b"client finished", &handshake_hash);

    // Build the Finished handshake message: type(1) + length(3) + verify_data(12)
    let mut pos = 0;
    send_buf[pos] = HS_FINISHED;
    pos += 1;
    pos = put_u24(&mut send_buf, pos, verify_data.len() as u32);
    send_buf[pos..pos + verify_data.len()].copy_from_slice(&verify_data);
    pos += verify_data.len();

    let finished_msg_len = pos;

    // Hash the plaintext Finished into the transcript BEFORE encrypting.
    transcript.update(&send_buf[..finished_msg_len]);

    // Encrypt and send as a handshake record.
    // encrypt_record writes header(5) + explicit_nonce(8) + ciphertext(16) + tag(16) = 45 bytes
    let mut encrypted = [0u8; 64];
    let enc_len = encryptor.encrypt_record(CONTENT_HANDSHAKE, &send_buf[..finished_msg_len], &mut encrypted)?;
    if verbose { eprint!("* TLS: sending encrypted Finished ({} bytes)...\n", enc_len); }
    send_all(fd, &encrypted[..enc_len], verbose)?;
    if verbose { eprint!("* TLS: client Finished sent\n"); }

    // -----------------------------------------------------------------------
    // 10. Receive ChangeCipherSpec from server
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for server ChangeCipherSpec...\n"); }
    let (ct, _hdr_end, _rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if ct != CONTENT_CHANGE_CIPHER_SPEC {
        return Err(HandshakeError::UnexpectedMessage);
    }
    if verbose { eprint!("* TLS: server ChangeCipherSpec received\n"); }
    // Do NOT hash ChangeCipherSpec.

    // -----------------------------------------------------------------------
    // 11. Receive server Finished
    // -----------------------------------------------------------------------

    if verbose { eprint!("* TLS: waiting for server Finished...\n"); }
    let (ct, _hdr_end, rec_end) = read_full_record(fd, &mut recv_buf, verbose)?;
    if ct != CONTENT_HANDSHAKE {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let encrypted_payload = &recv_buf[5..rec_end];
    // Finished message is 4-byte header + 12-byte verify_data = 16 bytes plaintext.
    let mut plaintext_buf = [0u8; 64];
    let pt_len = decryptor.decrypt_record(CONTENT_HANDSHAKE, encrypted_payload, &mut plaintext_buf)?;
    let plaintext = &plaintext_buf[..pt_len];

    // Parse the decrypted Finished message.
    if plaintext.is_empty() || plaintext[0] != HS_FINISHED {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let finished_len = get_u24(plaintext, 1) as usize;
    if finished_len != 12 || plaintext.len() < 4 + 12 {
        return Err(HandshakeError::FinishedVerifyFailed);
    }

    let server_verify_data = &plaintext[4..4 + 12];

    // Compute expected verify_data using the transcript that includes the
    // client Finished message.
    let handshake_hash = clone_sha256_state(&transcript);
    let expected_verify_data =
        compute_verify_data(&master_secret, b"server finished", &handshake_hash);

    if !constant_time_eq(server_verify_data, &expected_verify_data) {
        if verbose { eprint!("* TLS: server Finished verify FAILED\n"); }
        return Err(HandshakeError::FinishedVerifyFailed);
    }

    // Hash the server Finished into the transcript (for session resumption, if
    // we ever support it).
    transcript.update(plaintext);
    if verbose { eprint!("* TLS: handshake complete, connection encrypted\n"); }

    Ok(HandshakeResult {
        encryptor,
        decryptor,
    })
}

// ---------------------------------------------------------------------------
// Message builders
// ---------------------------------------------------------------------------

/// Build a ClientHello handshake message (without the record header) into
/// `buf`.  Returns the total length of the handshake message.
fn build_client_hello(buf: &mut [u8], client_random: &[u8; 32], hostname: &str) -> usize {
    // We first build the ClientHello body, then prepend the handshake header.
    // To avoid double-copying we leave 4 bytes at the front for the header.

    let mut pos = 4; // reserve space for handshake header

    // client_version: TLS 1.2
    pos = put_u16(buf, pos, 0x0303);

    // random: 32 bytes
    buf[pos..pos + 32].copy_from_slice(client_random);
    pos += 32;

    // session_id: empty
    buf[pos] = 0; // length = 0
    pos += 1;

    // cipher_suites: length(2) + 1 suite(2) = 4 bytes total
    pos = put_u16(buf, pos, 2); // 2 bytes of cipher suite data
    pos = put_u16(buf, pos, TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256);

    // compression_methods: length(1) + null(1) = 2 bytes
    buf[pos] = 1; // 1 compression method
    pos += 1;
    buf[pos] = 0x00; // null compression
    pos += 1;

    // Extensions -- build into a temporary area, then copy with length prefix.
    let ext_start = pos + 2; // leave 2 bytes for total extensions length
    let mut ext_pos = ext_start;

    // SNI extension
    let sni_len = build_sni_extension(hostname, &mut buf[ext_pos..]);
    ext_pos += sni_len;

    // supported_groups extension
    ext_pos = build_supported_groups_extension(buf, ext_pos);

    // signature_algorithms extension
    ext_pos = build_signature_algorithms_extension(buf, ext_pos);

    // ec_point_formats extension
    ext_pos = build_ec_point_formats_extension(buf, ext_pos);

    let extensions_length = ext_pos - ext_start;
    put_u16(buf, pos, extensions_length as u16);
    pos = ext_pos;

    // Now fill in the handshake header at the front.
    let body_length = pos - 4;
    buf[0] = HS_CLIENT_HELLO;
    put_u24(buf, 1, body_length as u32);

    pos
}

/// Wrap a handshake message that occupies `buf[0..msg_len]` inside a TLS
/// record header.
///
/// Shifts the message 5 bytes to the right and prepends the record header.
/// Returns the total record length (5 + msg_len).
fn wrap_handshake_record(buf: &mut [u8], msg_len: usize) -> usize {
    // Shift the message right by 5 bytes to make room for the record header.
    // We work from the end to avoid clobbering.
    buf.copy_within(0..msg_len, 5);

    buf[0] = CONTENT_HANDSHAKE;
    buf[1] = 0x03;
    buf[2] = 0x03; // TLS 1.2
    put_u16(buf, 3, msg_len as u16);

    5 + msg_len
}

// ---------------------------------------------------------------------------
// Message parsers
// ---------------------------------------------------------------------------

/// Parse a ServerHello handshake message payload (including the handshake
/// header).  Returns the 32-byte `server_random`.
fn parse_server_hello(payload: &[u8]) -> Result<[u8; 32], HandshakeError> {
    // Handshake header: type(1) + length(3)
    if payload.len() < 4 {
        return Err(HandshakeError::UnexpectedMessage);
    }
    let _hs_type = payload[0]; // already verified as HS_SERVER_HELLO
    let hs_len = get_u24(payload, 1) as usize;

    let body = &payload[4..];
    if body.len() < hs_len || hs_len < 2 + 32 + 1 {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let mut pos = 0;

    // server_version: must be 0x0303 (TLS 1.2)
    let version = get_u16(body, pos);
    pos += 2;
    if version != 0x0303 {
        return Err(HandshakeError::UnexpectedMessage);
    }

    // server_random: 32 bytes
    let mut server_random = [0u8; 32];
    server_random.copy_from_slice(&body[pos..pos + 32]);
    pos += 32;

    // session_id: length(1) + data
    let session_id_len = body[pos] as usize;
    pos += 1 + session_id_len;

    if pos + 3 > body.len() {
        return Err(HandshakeError::UnexpectedMessage);
    }

    // cipher_suite: must be our suite
    let suite = get_u16(body, pos);
    pos += 2;
    if suite != TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 {
        return Err(HandshakeError::UnsupportedCipherSuite);
    }

    // compression_method: must be 0x00
    let compression = body[pos];
    if compression != 0x00 {
        return Err(HandshakeError::UnexpectedMessage);
    }
    // Remaining bytes are extensions, which we skip.

    Ok(server_random)
}

/// Parse a Certificate handshake message payload.
/// Returns a `Vec` of parsed certificates.
fn parse_certificate_message(
    payload: &[u8],
) -> Result<Vec<Certificate>, HandshakeError> {
    // Handshake header: type(1) + length(3)
    if payload.len() < 4 {
        return Err(HandshakeError::UnexpectedMessage);
    }
    let body = &payload[4..];

    if body.len() < 3 {
        return Err(HandshakeError::UnexpectedMessage);
    }

    // certificates_length(3)
    let total_len = get_u24(body, 0) as usize;
    let mut pos = 3;

    if pos + total_len > body.len() {
        return Err(HandshakeError::UnexpectedMessage);
    }

    let end = pos + total_len;
    let mut certs = Vec::new();

    while pos < end {
        if pos + 3 > end {
            return Err(HandshakeError::UnexpectedMessage);
        }
        let cert_len = get_u24(body, pos) as usize;
        pos += 3;
        if pos + cert_len > end {
            return Err(HandshakeError::UnexpectedMessage);
        }
        let cert_data = &body[pos..pos + cert_len];
        let cert = parse_certificate(cert_data)
            .map_err(|_| HandshakeError::CertificateError(ChainError::ParseError))?;
        certs.push(cert);
        pos += cert_len;
    }

    if certs.is_empty() {
        return Err(HandshakeError::CertificateError(ChainError::EmptyChain));
    }

    Ok(certs)
}

/// Parse and verify a ServerKeyExchange handshake message.
///
/// Returns the server's 32-byte X25519 public key on success.
fn parse_and_verify_server_key_exchange(
    payload: &[u8],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
    leaf_cert: &Certificate,
) -> Result<[u8; 32], HandshakeError> {
    // Handshake header: type(1) + length(3)
    if payload.len() < 4 {
        return Err(HandshakeError::UnexpectedMessage);
    }
    let body = &payload[4..];

    let mut pos = 0;

    // --- ECDHE parameters ---

    // curve_type: must be 0x03 (named_curve)
    if pos >= body.len() {
        return Err(HandshakeError::InvalidServerKey);
    }
    let curve_type = body[pos];
    pos += 1;
    if curve_type != 0x03 {
        return Err(HandshakeError::UnsupportedCurve);
    }

    // named_curve: must be x25519
    if pos + 2 > body.len() {
        return Err(HandshakeError::InvalidServerKey);
    }
    let named_curve = get_u16(body, pos);
    pos += 2;
    if named_curve != NAMED_CURVE_X25519 {
        return Err(HandshakeError::UnsupportedCurve);
    }

    // pubkey_length: must be 32
    if pos >= body.len() {
        return Err(HandshakeError::InvalidServerKey);
    }
    let pubkey_len = body[pos] as usize;
    pos += 1;
    if pubkey_len != 32 {
        return Err(HandshakeError::InvalidServerKey);
    }

    // server public key (32 bytes)
    if pos + 32 > body.len() {
        return Err(HandshakeError::InvalidServerKey);
    }
    let mut server_pubkey = [0u8; 32];
    server_pubkey.copy_from_slice(&body[pos..pos + 32]);
    pos += 32;

    // The ECDHE params that are signed: curve_type(1) + named_curve(2) +
    // pubkey_length(1) + server_pubkey(32) = 36 bytes.
    let ecdhe_params = &body[..pos]; // first `pos` bytes of body

    // --- Signature ---

    if pos + 4 > body.len() {
        return Err(HandshakeError::SignatureVerificationFailed);
    }

    let sig_algorithm = get_u16(body, pos);
    pos += 2;
    // We only support RSA PKCS1 SHA-256
    if sig_algorithm != SIG_RSA_PKCS1_SHA256 {
        return Err(HandshakeError::SignatureVerificationFailed);
    }

    let sig_len = get_u16(body, pos) as usize;
    pos += 2;

    if pos + sig_len > body.len() {
        return Err(HandshakeError::SignatureVerificationFailed);
    }

    let signature = &body[pos..pos + sig_len];

    // Build the message that was signed:
    // client_random(32) + server_random(32) + ecdhe_params
    let mut signed_msg = Vec::with_capacity(32 + 32 + ecdhe_params.len());
    signed_msg.extend_from_slice(client_random);
    signed_msg.extend_from_slice(server_random);
    signed_msg.extend_from_slice(ecdhe_params);

    // Hash the signed message with SHA-256
    let message_hash = crate::crypto::sha256::sha256(&signed_msg);

    // Verify the signature using the leaf certificate's RSA public key
    let pk_info = leaf_cert.public_key.as_ref().ok_or(HandshakeError::InvalidServerKey)?;
    let rsa_key = RsaPublicKey {
        n: BigNum::from_be_bytes(&pk_info.modulus),
        e: BigNum::from_be_bytes(&pk_info.exponent),
    };

    if !rsa_verify_pkcs1_sha256(&rsa_key, signature, &message_hash) {
        return Err(HandshakeError::SignatureVerificationFailed);
    }

    Ok(server_pubkey)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Produce the SHA-256 digest of the current transcript state without
/// consuming the hasher.
///
/// The `Sha256` type is consumed by `finalize()`, so we clone the internal
/// state (which is just stack data) and finalize the clone.
fn clone_sha256_state(hasher: &Sha256) -> [u8; 32] {
    // Safety: Sha256 is a plain struct with no heap allocations and no
    // interior mutability.  A byte-for-byte copy is correct.  This is a
    // workaround for Sha256 not implementing Clone.
    let clone: Sha256 = unsafe { core::ptr::read(hasher) };
    clone.finalize()
}

/// Get the current Unix timestamp (seconds since epoch).
///
/// Uses the `clock_gettime(CLOCK_REALTIME)` syscall wrapper.
/// Returns 0 if the syscall fails (should not happen in normal operation).
fn current_unix_time() -> u64 {
    match crate::time::now_realtime() {
        Ok(ts) => ts.tv_sec as u64,
        Err(_) => 0,
    }
}

/// Constant-time comparison of two byte slices.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Byte encoding helpers --

    #[test]
    fn test_put_u16_get_u16() {
        let mut buf = [0u8; 4];
        let next = put_u16(&mut buf, 0, 0x0303);
        assert_eq!(next, 2);
        assert_eq!(buf[0], 0x03);
        assert_eq!(buf[1], 0x03);
        assert_eq!(get_u16(&buf, 0), 0x0303);
    }

    #[test]
    fn test_put_u16_nonzero_offset() {
        let mut buf = [0u8; 8];
        let next = put_u16(&mut buf, 3, 0xC02F);
        assert_eq!(next, 5);
        assert_eq!(buf[3], 0xC0);
        assert_eq!(buf[4], 0x2F);
        assert_eq!(get_u16(&buf, 3), 0xC02F);
    }

    #[test]
    fn test_put_u24_get_u24() {
        let mut buf = [0u8; 4];
        let next = put_u24(&mut buf, 0, 0x00ABCDEF);
        assert_eq!(next, 3);
        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 0xCD);
        assert_eq!(buf[2], 0xEF);
        assert_eq!(get_u24(&buf, 0), 0x00ABCDEF);
    }

    #[test]
    fn test_put_u24_zero() {
        let mut buf = [0u8; 4];
        put_u24(&mut buf, 0, 0);
        assert_eq!(buf[0], 0);
        assert_eq!(buf[1], 0);
        assert_eq!(buf[2], 0);
        assert_eq!(get_u24(&buf, 0), 0);
    }

    #[test]
    fn test_put_u16_boundary_values() {
        let mut buf = [0u8; 2];

        put_u16(&mut buf, 0, 0x0000);
        assert_eq!(get_u16(&buf, 0), 0x0000);

        put_u16(&mut buf, 0, 0xFFFF);
        assert_eq!(get_u16(&buf, 0), 0xFFFF);

        put_u16(&mut buf, 0, 0x0001);
        assert_eq!(get_u16(&buf, 0), 0x0001);

        put_u16(&mut buf, 0, 0x8000);
        assert_eq!(get_u16(&buf, 0), 0x8000);
    }

    #[test]
    fn test_put_u24_max() {
        let mut buf = [0u8; 3];
        put_u24(&mut buf, 0, 0x00FFFFFF);
        assert_eq!(get_u24(&buf, 0), 0x00FFFFFF);
    }

    // -- Constant-time comparison --

    #[test]
    fn test_constant_time_eq_equal() {
        let a = [1u8, 2, 3, 4, 5];
        let b = [1u8, 2, 3, 4, 5];
        assert!(constant_time_eq(&a, &b));
    }

    #[test]
    fn test_constant_time_eq_different() {
        let a = [1u8, 2, 3, 4, 5];
        let b = [1u8, 2, 3, 4, 6];
        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        let a = [1u8, 2, 3];
        let b = [1u8, 2, 3, 4];
        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        let a: [u8; 0] = [];
        let b: [u8; 0] = [];
        assert!(constant_time_eq(&a, &b));
    }

    // -- ClientHello construction --

    #[test]
    fn test_client_hello_structure() {
        let mut buf = [0u8; HANDSHAKE_BUF_SIZE];
        let client_random = [0x42u8; 32];
        let len = build_client_hello(&mut buf, &client_random, "example.com");

        // Handshake header
        assert_eq!(buf[0], HS_CLIENT_HELLO);
        let body_len = get_u24(&buf, 1) as usize;
        assert_eq!(len, 4 + body_len);

        // ClientHello body starts at offset 4
        let body = &buf[4..len];

        // Version: TLS 1.2
        assert_eq!(body[0], 0x03);
        assert_eq!(body[1], 0x03);

        // Random: our 32 bytes
        assert_eq!(&body[2..34], &[0x42u8; 32]);

        // Session ID length: 0
        assert_eq!(body[34], 0);

        // Cipher suites length: 2
        assert_eq!(get_u16(body, 35), 2);

        // Cipher suite: 0xC02F
        assert_eq!(get_u16(body, 37), 0xC02F);

        // Compression methods length: 1, method: null
        assert_eq!(body[39], 1);
        assert_eq!(body[40], 0x00);

        // Extensions should follow at offset 41
        let ext_total_len = get_u16(body, 41) as usize;
        assert!(ext_total_len > 0);
        assert_eq!(len, 4 + 43 + ext_total_len);
    }

    #[test]
    fn test_client_hello_sni_present() {
        let mut buf = [0u8; HANDSHAKE_BUF_SIZE];
        let client_random = [0x00u8; 32];
        let len = build_client_hello(&mut buf, &client_random, "test.example.com");

        // Search for the SNI extension type (0x0000) in the extensions area.
        // Extensions start at body offset 43 (after the 4-byte HS header = buf offset 47).
        let body = &buf[4..len];
        let ext_start = 43;
        let ext_total_len = get_u16(body, 41) as usize;
        let ext_end = ext_start + ext_total_len;

        // The first extension should be SNI
        assert_eq!(get_u16(body, ext_start), EXT_SERVER_NAME);

        // Verify the hostname appears in the extension data
        let hostname = b"test.example.com";
        let mut found = false;
        for window in body[ext_start..ext_end].windows(hostname.len()) {
            if window == hostname {
                found = true;
                break;
            }
        }
        assert!(found, "hostname not found in SNI extension");
    }

    #[test]
    fn test_client_hello_extensions_contain_all_types() {
        let mut buf = [0u8; HANDSHAKE_BUF_SIZE];
        let client_random = [0x00u8; 32];
        let len = build_client_hello(&mut buf, &client_random, "example.com");

        let body = &buf[4..len];
        let ext_start = 43;
        let ext_total_len = get_u16(body, 41) as usize;
        let ext_end = ext_start + ext_total_len;

        // Walk extensions and collect their types
        let mut ext_types = Vec::new();
        let mut pos = ext_start;
        while pos + 4 <= ext_end {
            let ext_type = get_u16(body, pos);
            let ext_len = get_u16(body, pos + 2) as usize;
            ext_types.push(ext_type);
            pos += 4 + ext_len;
        }

        assert!(ext_types.contains(&EXT_SERVER_NAME), "missing SNI extension");
        assert!(
            ext_types.contains(&EXT_SUPPORTED_GROUPS),
            "missing supported_groups extension"
        );
        assert!(
            ext_types.contains(&EXT_SIGNATURE_ALGORITHMS),
            "missing signature_algorithms extension"
        );
        assert!(
            ext_types.contains(&EXT_EC_POINT_FORMATS),
            "missing ec_point_formats extension"
        );
    }

    // -- SNI extension builder --

    #[test]
    fn test_build_sni_extension() {
        let mut buf = [0u8; 256];
        let len = build_sni_extension("example.com", &mut buf);

        // Extension type
        assert_eq!(get_u16(&buf, 0), EXT_SERVER_NAME);

        // The hostname "example.com" is 11 bytes
        // extension_data_length = 2 + 1 + 2 + 11 = 16
        assert_eq!(get_u16(&buf, 2), 16);

        // server_name_list_length = 1 + 2 + 11 = 14
        assert_eq!(get_u16(&buf, 4), 14);

        // name_type = host_name (0)
        assert_eq!(buf[6], 0x00);

        // host_name_length = 11
        assert_eq!(get_u16(&buf, 7), 11);

        // hostname bytes
        assert_eq!(&buf[9..9 + 11], b"example.com");

        // Total length: 2 + 2 + 2 + 1 + 2 + 11 = 20
        assert_eq!(len, 20);
    }

    // -- Record wrapping --

    #[test]
    fn test_wrap_handshake_record() {
        let mut buf = [0u8; 256];
        // Simulate a 10-byte handshake message at buf[0..10]
        for i in 0..10 {
            buf[i] = (i + 1) as u8;
        }

        let total = wrap_handshake_record(&mut buf, 10);
        assert_eq!(total, 15);

        // Record header
        assert_eq!(buf[0], CONTENT_HANDSHAKE);
        assert_eq!(buf[1], 0x03);
        assert_eq!(buf[2], 0x03);
        assert_eq!(get_u16(&buf, 3), 10);

        // Original message shifted by 5
        for i in 0..10 {
            assert_eq!(buf[5 + i], (i + 1) as u8);
        }
    }
}
