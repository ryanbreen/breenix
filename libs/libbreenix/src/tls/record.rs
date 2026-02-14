//! TLS 1.2 Record Layer (RFC 5246 Section 6)
//!
//! Implements the TLS record protocol which provides:
//! - Fragmentation of data into manageable blocks
//! - Framing with content type and length
//! - Encryption and authentication via AES-128-GCM
//!
//! The record layer sits below the handshake, alert, and application data
//! protocols, providing a uniform framing and (after key exchange) confidentiality
//! and integrity.

use crate::crypto::gcm::AesGcm;

// ---------------------------------------------------------------------------
// Content types (RFC 5246 Section 6.2.1)
// ---------------------------------------------------------------------------

pub const CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;
pub const CONTENT_ALERT: u8 = 21;
pub const CONTENT_HANDSHAKE: u8 = 22;
pub const CONTENT_APPLICATION_DATA: u8 = 23;

// ---------------------------------------------------------------------------
// Protocol version (RFC 5246 Section 6.2.1)
// ---------------------------------------------------------------------------

/// TLS 1.2 protocol version {3, 3}.
pub const TLS_12: [u8; 2] = [0x03, 0x03];

/// TLS 1.0 protocol version {3, 1}. Used in some record headers for
/// compatibility with middleboxes that reject unfamiliar versions.
pub const TLS_10: [u8; 2] = [0x03, 0x01];

// ---------------------------------------------------------------------------
// Record limits (RFC 5246 Section 6.2.1)
// ---------------------------------------------------------------------------

/// Maximum plaintext fragment length: 2^14 bytes.
pub const MAX_RECORD_SIZE: usize = 16384;

/// Size of the 5-byte record header: content_type(1) + version(2) + length(2).
pub const RECORD_HEADER_SIZE: usize = 5;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Output buffer is too small to hold the result.
    BufferTooSmall,
    /// Payload exceeds the maximum record size.
    RecordTooLarge,
    /// The content type byte is not a recognized TLS content type.
    InvalidContentType,
    /// AEAD decryption or tag verification failed.
    DecryptionFailed,
    /// Not enough data to parse a complete record.
    IncompleteRecord,
    /// The record is structurally invalid.
    InvalidRecord,
}

// ---------------------------------------------------------------------------
// Plaintext record I/O
// ---------------------------------------------------------------------------

/// Write a TLS record header and payload into `out`.
///
/// The record format is:
/// ```text
/// content_type(1) | TLS_12(2) | length(2, big-endian) | payload(length)
/// ```
///
/// Returns the total number of bytes written (header + payload).
pub fn write_record(
    content_type: u8,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, RecordError> {
    if payload.len() > MAX_RECORD_SIZE {
        return Err(RecordError::RecordTooLarge);
    }

    let total = RECORD_HEADER_SIZE + payload.len();
    if out.len() < total {
        return Err(RecordError::BufferTooSmall);
    }

    validate_content_type(content_type)?;

    // Content type
    out[0] = content_type;

    // Protocol version
    out[1] = TLS_12[0];
    out[2] = TLS_12[1];

    // Payload length (big-endian u16)
    let len_bytes = (payload.len() as u16).to_be_bytes();
    out[3] = len_bytes[0];
    out[4] = len_bytes[1];

    // Payload
    out[RECORD_HEADER_SIZE..total].copy_from_slice(payload);

    Ok(total)
}

/// Parse a TLS record from the beginning of `data`.
///
/// Returns `(content_type, payload_slice, total_bytes_consumed)` where
/// `total_bytes_consumed` includes the 5-byte header.
pub fn read_record(data: &[u8]) -> Result<(u8, &[u8], usize), RecordError> {
    if data.len() < RECORD_HEADER_SIZE {
        return Err(RecordError::IncompleteRecord);
    }

    let content_type = data[0];
    validate_content_type(content_type)?;

    // Bytes [1..3] are the protocol version -- we accept any version here
    // since the record layer should be version-tolerant for receiving.

    let payload_len = u16::from_be_bytes([data[3], data[4]]) as usize;

    if payload_len > MAX_RECORD_SIZE {
        return Err(RecordError::RecordTooLarge);
    }

    let total = RECORD_HEADER_SIZE + payload_len;
    if data.len() < total {
        return Err(RecordError::IncompleteRecord);
    }

    let payload = &data[RECORD_HEADER_SIZE..total];
    Ok((content_type, payload, total))
}

/// Validate that a byte is a recognised TLS content type.
fn validate_content_type(ct: u8) -> Result<(), RecordError> {
    match ct {
        CONTENT_CHANGE_CIPHER_SPEC
        | CONTENT_ALERT
        | CONTENT_HANDSHAKE
        | CONTENT_APPLICATION_DATA => Ok(()),
        _ => Err(RecordError::InvalidContentType),
    }
}

// ---------------------------------------------------------------------------
// AES-128-GCM additional authenticated data
// ---------------------------------------------------------------------------

/// Size of the AAD for TLS 1.2 AES-GCM:
/// seq_num(8) + content_type(1) + version(2) + plaintext_length(2) = 13 bytes.
const AAD_SIZE: usize = 13;

/// Construct the 13-byte additional authenticated data block used by TLS 1.2
/// AES-GCM cipher suites.
///
/// Layout: `seq_num(8, big-endian) || content_type(1) || version(2) || length(2, big-endian)`
fn build_aad(seq_num: u64, content_type: u8, plaintext_len: u16) -> [u8; AAD_SIZE] {
    let mut aad = [0u8; AAD_SIZE];
    aad[0..8].copy_from_slice(&seq_num.to_be_bytes());
    aad[8] = content_type;
    aad[9] = TLS_12[0];
    aad[10] = TLS_12[1];
    aad[11..13].copy_from_slice(&plaintext_len.to_be_bytes());
    aad
}

// ---------------------------------------------------------------------------
// Nonce construction
// ---------------------------------------------------------------------------

/// Size of the explicit nonce sent on the wire (the sequence number).
const EXPLICIT_NONCE_SIZE: usize = 8;

/// Size of the AES-GCM authentication tag.
const TAG_SIZE: usize = 16;

/// Construct the 12-byte AES-GCM nonce from the fixed IV and explicit nonce.
///
/// `nonce = fixed_iv(4) || explicit_nonce(8)`
fn build_nonce(fixed_iv: &[u8; 4], explicit_nonce: &[u8; 8]) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(fixed_iv);
    nonce[4..12].copy_from_slice(explicit_nonce);
    nonce
}

// ---------------------------------------------------------------------------
// Record encryptor
// ---------------------------------------------------------------------------

/// Encrypts outgoing TLS records using AES-128-GCM.
///
/// Maintains the write sequence number which is used both as the explicit
/// nonce (sent on the wire) and as part of the additional authenticated data.
pub struct RecordEncryptor {
    cipher: AesGcm,
    /// Fixed 4-byte implicit IV derived from the key material.
    fixed_iv: [u8; 4],
    /// Monotonically increasing sequence number (also serves as the explicit nonce).
    seq_num: u64,
}

impl RecordEncryptor {
    /// Create a new record encryptor from the AES-128 key and the 4-byte
    /// fixed (implicit) IV portion derived during key expansion.
    pub fn new(key: &[u8; 16], fixed_iv: &[u8; 4]) -> Self {
        Self {
            cipher: AesGcm::new(key),
            fixed_iv: *fixed_iv,
            seq_num: 0,
        }
    }

    /// Encrypt a TLS record.
    ///
    /// The output format is:
    /// ```text
    /// record_header(5) | explicit_nonce(8) | ciphertext(plaintext.len()) | tag(16)
    /// ```
    ///
    /// The record header's length field covers `explicit_nonce + ciphertext + tag`.
    ///
    /// Returns the total number of bytes written to `out`.
    pub fn encrypt_record(
        &mut self,
        content_type: u8,
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, RecordError> {
        if plaintext.len() > MAX_RECORD_SIZE {
            return Err(RecordError::RecordTooLarge);
        }

        validate_content_type(content_type)?;

        // The on-wire payload after the record header:
        //   explicit_nonce(8) + ciphertext(same length as plaintext) + tag(16)
        let record_payload_len = EXPLICIT_NONCE_SIZE + plaintext.len() + TAG_SIZE;
        let total_len = RECORD_HEADER_SIZE + record_payload_len;

        if out.len() < total_len {
            return Err(RecordError::BufferTooSmall);
        }

        // -- Record header --
        out[0] = content_type;
        out[1] = TLS_12[0];
        out[2] = TLS_12[1];
        let payload_len_bytes = (record_payload_len as u16).to_be_bytes();
        out[3] = payload_len_bytes[0];
        out[4] = payload_len_bytes[1];

        // -- Explicit nonce (sequence number, big-endian) --
        let explicit_nonce = self.seq_num.to_be_bytes();
        out[RECORD_HEADER_SIZE..RECORD_HEADER_SIZE + EXPLICIT_NONCE_SIZE]
            .copy_from_slice(&explicit_nonce);

        // -- Construct 12-byte nonce --
        let nonce = build_nonce(&self.fixed_iv, &explicit_nonce);

        // -- Construct AAD --
        let aad = build_aad(self.seq_num, content_type, plaintext.len() as u16);

        // -- Encrypt --
        let ct_start = RECORD_HEADER_SIZE + EXPLICIT_NONCE_SIZE;
        let ct_end = ct_start + plaintext.len();
        let tag_start = ct_end;
        let tag_end = tag_start + TAG_SIZE;

        let mut tag = [0u8; TAG_SIZE];
        self.cipher.encrypt(
            &nonce,
            &aad,
            plaintext,
            &mut out[ct_start..ct_end],
            &mut tag,
        );
        out[tag_start..tag_end].copy_from_slice(&tag);

        // Advance sequence number
        self.seq_num += 1;

        Ok(total_len)
    }
}

// ---------------------------------------------------------------------------
// Record decryptor
// ---------------------------------------------------------------------------

/// Decrypts incoming TLS records using AES-128-GCM.
///
/// Maintains the read sequence number which must stay in sync with the
/// sender's write sequence number.
pub struct RecordDecryptor {
    cipher: AesGcm,
    /// Fixed 4-byte implicit IV derived from the key material.
    fixed_iv: [u8; 4],
    /// Monotonically increasing sequence number.
    seq_num: u64,
}

impl RecordDecryptor {
    /// Create a new record decryptor from the AES-128 key and the 4-byte
    /// fixed (implicit) IV portion derived during key expansion.
    pub fn new(key: &[u8; 16], fixed_iv: &[u8; 4]) -> Self {
        Self {
            cipher: AesGcm::new(key),
            fixed_iv: *fixed_iv,
            seq_num: 0,
        }
    }

    /// Decrypt a TLS record payload.
    ///
    /// `record_payload` is the data after the 5-byte record header, with the
    /// layout:
    /// ```text
    /// explicit_nonce(8) | ciphertext(N) | tag(16)
    /// ```
    ///
    /// The decrypted plaintext is written to `out`, and the number of plaintext
    /// bytes is returned.
    pub fn decrypt_record(
        &mut self,
        content_type: u8,
        record_payload: &[u8],
        out: &mut [u8],
    ) -> Result<usize, RecordError> {
        // Minimum payload: explicit_nonce(8) + tag(16) = 24 bytes (zero-length plaintext)
        let min_payload = EXPLICIT_NONCE_SIZE + TAG_SIZE;
        if record_payload.len() < min_payload {
            return Err(RecordError::IncompleteRecord);
        }

        validate_content_type(content_type)?;

        // Extract explicit nonce
        let mut explicit_nonce = [0u8; EXPLICIT_NONCE_SIZE];
        explicit_nonce.copy_from_slice(&record_payload[..EXPLICIT_NONCE_SIZE]);

        // Ciphertext sits between the explicit nonce and the trailing tag
        let ciphertext_len = record_payload.len() - EXPLICIT_NONCE_SIZE - TAG_SIZE;
        let ct_start = EXPLICIT_NONCE_SIZE;
        let ct_end = ct_start + ciphertext_len;
        let ciphertext = &record_payload[ct_start..ct_end];

        // Extract tag
        let mut tag = [0u8; TAG_SIZE];
        tag.copy_from_slice(&record_payload[ct_end..ct_end + TAG_SIZE]);

        if out.len() < ciphertext_len {
            return Err(RecordError::BufferTooSmall);
        }

        // Construct 12-byte nonce
        let nonce = build_nonce(&self.fixed_iv, &explicit_nonce);

        // Construct AAD (plaintext_len == ciphertext_len for AES-GCM in CTR mode)
        let aad = build_aad(self.seq_num, content_type, ciphertext_len as u16);

        // Decrypt and verify
        let valid = self.cipher.decrypt(
            &nonce,
            &aad,
            ciphertext,
            &tag,
            &mut out[..ciphertext_len],
        );

        if !valid {
            // Zero out any partially written plaintext
            for byte in out[..ciphertext_len].iter_mut() {
                *byte = 0;
            }
            return Err(RecordError::DecryptionFailed);
        }

        // Advance sequence number
        self.seq_num += 1;

        Ok(ciphertext_len)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Plaintext record tests --

    #[test]
    fn test_write_read_roundtrip() {
        let payload = b"Hello, TLS!";
        let mut buf = [0u8; 256];

        let written = write_record(CONTENT_APPLICATION_DATA, payload, &mut buf).unwrap();
        assert_eq!(written, RECORD_HEADER_SIZE + payload.len());

        let (ct, data, consumed) = read_record(&buf[..written]).unwrap();
        assert_eq!(ct, CONTENT_APPLICATION_DATA);
        assert_eq!(data, payload);
        assert_eq!(consumed, written);
    }

    #[test]
    fn test_write_read_roundtrip_empty_payload() {
        let payload: &[u8] = &[];
        let mut buf = [0u8; 64];

        let written = write_record(CONTENT_HANDSHAKE, payload, &mut buf).unwrap();
        assert_eq!(written, RECORD_HEADER_SIZE);

        let (ct, data, consumed) = read_record(&buf[..written]).unwrap();
        assert_eq!(ct, CONTENT_HANDSHAKE);
        assert_eq!(data.len(), 0);
        assert_eq!(consumed, RECORD_HEADER_SIZE);
    }

    #[test]
    fn test_record_header_encoding() {
        let payload = &[0xAA, 0xBB, 0xCC];
        let mut buf = [0u8; 64];

        let written = write_record(CONTENT_ALERT, payload, &mut buf).unwrap();
        assert_eq!(written, 8);

        // Content type
        assert_eq!(buf[0], CONTENT_ALERT);

        // Version: TLS 1.2
        assert_eq!(buf[1], 0x03);
        assert_eq!(buf[2], 0x03);

        // Length: 3 in big-endian
        assert_eq!(buf[3], 0x00);
        assert_eq!(buf[4], 0x03);

        // Payload
        assert_eq!(&buf[5..8], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_write_record_buffer_too_small() {
        let payload = &[0u8; 10];
        let mut buf = [0u8; 10]; // too small for header + payload

        let result = write_record(CONTENT_APPLICATION_DATA, payload, &mut buf);
        assert_eq!(result, Err(RecordError::BufferTooSmall));
    }

    #[test]
    fn test_write_record_too_large() {
        let payload = &[0u8; MAX_RECORD_SIZE + 1];
        let mut buf = [0u8; MAX_RECORD_SIZE + RECORD_HEADER_SIZE + 2];

        let result = write_record(CONTENT_APPLICATION_DATA, payload, &mut buf);
        assert_eq!(result, Err(RecordError::RecordTooLarge));
    }

    #[test]
    fn test_write_record_invalid_content_type() {
        let payload = &[0u8; 4];
        let mut buf = [0u8; 64];

        let result = write_record(0xFF, payload, &mut buf);
        assert_eq!(result, Err(RecordError::InvalidContentType));
    }

    #[test]
    fn test_read_record_incomplete_header() {
        let data = &[0x17, 0x03, 0x03]; // only 3 bytes, need 5
        let result = read_record(data);
        assert_eq!(result, Err(RecordError::IncompleteRecord));
    }

    #[test]
    fn test_read_record_incomplete_payload() {
        // Header says 100 bytes of payload, but we only provide the header
        let data = &[0x17, 0x03, 0x03, 0x00, 0x64];
        let result = read_record(data);
        assert_eq!(result, Err(RecordError::IncompleteRecord));
    }

    #[test]
    fn test_read_record_invalid_content_type() {
        let data = &[0x00, 0x03, 0x03, 0x00, 0x00];
        let result = read_record(data);
        assert_eq!(result, Err(RecordError::InvalidContentType));
    }

    #[test]
    fn test_all_content_types() {
        let mut buf = [0u8; 64];
        let payload = &[0x42];

        for &ct in &[
            CONTENT_CHANGE_CIPHER_SPEC,
            CONTENT_ALERT,
            CONTENT_HANDSHAKE,
            CONTENT_APPLICATION_DATA,
        ] {
            let written = write_record(ct, payload, &mut buf).unwrap();
            let (parsed_ct, data, consumed) = read_record(&buf[..written]).unwrap();
            assert_eq!(parsed_ct, ct);
            assert_eq!(data, payload);
            assert_eq!(consumed, written);
        }
    }

    // -- AAD construction tests --

    #[test]
    fn test_aad_construction() {
        let aad = build_aad(0x0000_0000_0000_0001, CONTENT_APPLICATION_DATA, 0x0100);

        // seq_num: 8 bytes big-endian
        assert_eq!(&aad[0..8], &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        // content_type
        assert_eq!(aad[8], CONTENT_APPLICATION_DATA);
        // version: TLS 1.2
        assert_eq!(aad[9], 0x03);
        assert_eq!(aad[10], 0x03);
        // plaintext length: 256 = 0x0100
        assert_eq!(&aad[11..13], &[0x01, 0x00]);
    }

    #[test]
    fn test_aad_size() {
        let aad = build_aad(0, CONTENT_HANDSHAKE, 0);
        assert_eq!(aad.len(), 13);
    }

    // -- Encrypted record tests --

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0x01u8; 16];
        let iv = [0xABu8; 4];
        let plaintext = b"The quick brown fox jumps over the lazy dog";

        let mut enc = RecordEncryptor::new(&key, &iv);
        let mut dec = RecordDecryptor::new(&key, &iv);

        let mut encrypted = [0u8; 1024];
        let enc_len = enc
            .encrypt_record(CONTENT_APPLICATION_DATA, plaintext, &mut encrypted)
            .unwrap();

        // Parse the record header to get the payload
        let (ct, payload, consumed) = read_record(&encrypted[..enc_len]).unwrap();
        assert_eq!(ct, CONTENT_APPLICATION_DATA);
        assert_eq!(consumed, enc_len);

        // Decrypt the payload
        let mut decrypted = [0u8; 1024];
        let dec_len = dec
            .decrypt_record(CONTENT_APPLICATION_DATA, payload, &mut decrypted)
            .unwrap();

        assert_eq!(dec_len, plaintext.len());
        assert_eq!(&decrypted[..dec_len], &plaintext[..]);
    }

    #[test]
    fn test_encrypt_decrypt_empty_plaintext() {
        let key = [0x42u8; 16];
        let iv = [0x00u8; 4];

        let mut enc = RecordEncryptor::new(&key, &iv);
        let mut dec = RecordDecryptor::new(&key, &iv);

        let plaintext: &[u8] = &[];
        let mut encrypted = [0u8; 256];
        let enc_len = enc
            .encrypt_record(CONTENT_HANDSHAKE, plaintext, &mut encrypted)
            .unwrap();

        // Header(5) + explicit_nonce(8) + ciphertext(0) + tag(16) = 29
        assert_eq!(enc_len, RECORD_HEADER_SIZE + EXPLICIT_NONCE_SIZE + TAG_SIZE);

        let (ct, payload, _) = read_record(&encrypted[..enc_len]).unwrap();
        assert_eq!(ct, CONTENT_HANDSHAKE);

        let mut decrypted = [0u8; 256];
        let dec_len = dec
            .decrypt_record(CONTENT_HANDSHAKE, payload, &mut decrypted)
            .unwrap();
        assert_eq!(dec_len, 0);
    }

    #[test]
    fn test_encrypt_decrypt_multiple_records() {
        let key = [0x77u8; 16];
        let iv = [0x11u8; 4];

        let mut enc = RecordEncryptor::new(&key, &iv);
        let mut dec = RecordDecryptor::new(&key, &iv);

        // Encrypt and decrypt three records in sequence, verifying that
        // the sequence numbers stay in sync.
        let messages: &[&[u8]] = &[b"first", b"second", b"third"];

        for &msg in messages {
            let mut encrypted = [0u8; 256];
            let enc_len = enc
                .encrypt_record(CONTENT_APPLICATION_DATA, msg, &mut encrypted)
                .unwrap();

            let (_, payload, _) = read_record(&encrypted[..enc_len]).unwrap();

            let mut decrypted = [0u8; 256];
            let dec_len = dec
                .decrypt_record(CONTENT_APPLICATION_DATA, payload, &mut decrypted)
                .unwrap();

            assert_eq!(&decrypted[..dec_len], msg);
        }
    }

    #[test]
    fn test_decrypt_rejects_tampered_ciphertext() {
        let key = [0x55u8; 16];
        let iv = [0x22u8; 4];

        let mut enc = RecordEncryptor::new(&key, &iv);
        let mut dec = RecordDecryptor::new(&key, &iv);

        let plaintext = b"sensitive data";
        let mut encrypted = [0u8; 256];
        let enc_len = enc
            .encrypt_record(CONTENT_APPLICATION_DATA, plaintext, &mut encrypted)
            .unwrap();

        // Tamper with a ciphertext byte (after header + explicit nonce)
        let tamper_offset = RECORD_HEADER_SIZE + EXPLICIT_NONCE_SIZE + 2;
        encrypted[tamper_offset] ^= 0xFF;

        let (_, payload, _) = read_record(&encrypted[..enc_len]).unwrap();

        let mut decrypted = [0u8; 256];
        let result = dec.decrypt_record(CONTENT_APPLICATION_DATA, payload, &mut decrypted);
        assert_eq!(result, Err(RecordError::DecryptionFailed));
    }

    #[test]
    fn test_decrypt_rejects_wrong_sequence_number() {
        let key = [0x99u8; 16];
        let iv = [0x33u8; 4];

        let mut enc = RecordEncryptor::new(&key, &iv);

        // Encrypt two records
        let mut encrypted1 = [0u8; 256];
        let enc_len1 = enc
            .encrypt_record(CONTENT_APPLICATION_DATA, b"first", &mut encrypted1)
            .unwrap();

        let mut encrypted2 = [0u8; 256];
        let _enc_len2 = enc
            .encrypt_record(CONTENT_APPLICATION_DATA, b"second", &mut encrypted2)
            .unwrap();

        // Create a decryptor and try to decrypt the second record first
        // (seq_num mismatch: decryptor expects 0, but record was encrypted with 1)
        let mut dec = RecordDecryptor::new(&key, &iv);

        // First, consume record 1 with the decryptor so seq advances to 1
        let (_, payload1, _) = read_record(&encrypted1[..enc_len1]).unwrap();
        let mut decrypted = [0u8; 256];
        dec.decrypt_record(CONTENT_APPLICATION_DATA, payload1, &mut decrypted)
            .unwrap();

        // Now try to replay record 1 again (decryptor is at seq 1, but record
        // was encrypted with seq 0) -- AAD mismatch should cause auth failure.
        let (_, payload1_again, _) = read_record(&encrypted1[..enc_len1]).unwrap();
        let result =
            dec.decrypt_record(CONTENT_APPLICATION_DATA, payload1_again, &mut decrypted);
        assert_eq!(result, Err(RecordError::DecryptionFailed));
    }

    #[test]
    fn test_encrypt_record_output_layout() {
        let key = [0xAAu8; 16];
        let iv = [0xBBu8; 4];

        let mut enc = RecordEncryptor::new(&key, &iv);
        let plaintext = &[0x01, 0x02, 0x03, 0x04, 0x05];

        let mut out = [0u8; 256];
        let total = enc
            .encrypt_record(CONTENT_APPLICATION_DATA, plaintext, &mut out)
            .unwrap();

        // Total = header(5) + explicit_nonce(8) + ciphertext(5) + tag(16) = 34
        assert_eq!(total, 5 + 8 + 5 + 16);

        // Header checks
        assert_eq!(out[0], CONTENT_APPLICATION_DATA);
        assert_eq!(out[1], 0x03); // TLS 1.2 major
        assert_eq!(out[2], 0x03); // TLS 1.2 minor

        // Record payload length = 8 + 5 + 16 = 29 = 0x001D
        assert_eq!(out[3], 0x00);
        assert_eq!(out[4], 0x1D);

        // Explicit nonce should be seq_num 0 in big-endian
        assert_eq!(&out[5..13], &[0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_nonce_construction() {
        let fixed = [0x01, 0x02, 0x03, 0x04];
        let explicit = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05];
        let nonce = build_nonce(&fixed, &explicit);

        assert_eq!(nonce.len(), 12);
        assert_eq!(&nonce[0..4], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&nonce[4..12], &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05]);
    }
}
