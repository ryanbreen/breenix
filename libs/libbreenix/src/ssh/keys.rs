//! SSH keys: the server's Ed25519 host key, client identities, public-key
//! blobs and signature verification.
//!
//! Formats:
//! - Ed25519 public-key and signature blobs (RFC 8709 section 4 and 6):
//!   `string "ssh-ed25519" || string key(32)` and `string "ssh-ed25519" || string signature(64)`.
//! - RSA public-key blobs and rsa-sha2-256/512 signatures (RFC 4253 section 6.6, RFC 8332).
//! - Public-key lines, as in `.pub` and `authorized_keys` files: `type base64(blob) [comment]`.
//! - Private keys in the OpenSSH key format (`openssh-key-v1`, unencrypted), and
//!   unencrypted PKCS#1 PEM for RSA client identities.
//! - Fingerprints: `SHA256:` and the unpadded base64 of the blob's SHA-256.
//!
//! This module holds no compiled-in key material: host keys are generated per
//! machine (`HostKey::generate`) and identities are read from files.

use crate::crypto::bignum::BigNum;
use crate::crypto::ed25519::{self, SigningKey};
use crate::crypto::rsa::{
    rsa_sign_pkcs1_sha256, rsa_verify_pkcs1_sha256, rsa_verify_pkcs1_sha512, RsaPrivateKey,
    RsaPublicKey,
};
use crate::crypto::sha256::sha256;
use crate::crypto::sha512::sha512;

use super::SshBuf;

pub const ED25519: &str = "ssh-ed25519";
pub const RSA: &str = "ssh-rsa";

const OPENSSH_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const OPENSSH_END: &str = "-----END OPENSSH PRIVATE KEY-----";
const OPENSSH_MAGIC: &[u8] = b"openssh-key-v1\0";

// ==========================================================================
// Ed25519 keys
// ==========================================================================

/// Encode an Ed25519 public key as an SSH public-key blob.
pub fn ed25519_public_blob(public: &[u8; ed25519::PUBLIC_KEY_LEN]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(4 + ED25519.len() + 4 + public.len());
    SshBuf::put_string(&mut blob, ED25519.as_bytes());
    SshBuf::put_string(&mut blob, public);
    blob
}

/// An Ed25519 private key with SSH encodings: the server's host key, or a client identity.
pub struct Ed25519Key {
    key: SigningKey,
}

impl Ed25519Key {
    pub fn from_seed(seed: &[u8; ed25519::SEED_LEN]) -> Self {
        Self { key: SigningKey::from_seed(seed) }
    }

    /// A new key from `random`, which must fill the buffer from a credited source.
    pub fn generate(random: impl FnOnce(&mut [u8]) -> Result<(), &'static str>) -> Result<Self, &'static str> {
        let mut seed = [0u8; ed25519::SEED_LEN];
        random(&mut seed)?;
        let key = Self::from_seed(&seed);
        for byte in seed.iter_mut() {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        Ok(key)
    }

    pub fn public_key_blob(&self) -> Vec<u8> {
        ed25519_public_blob(&self.key.public_key())
    }

    /// Sign `data`, returning an SSH signature blob.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        let signature = self.key.sign(data);
        let mut blob = Vec::with_capacity(4 + ED25519.len() + 4 + signature.len());
        SshBuf::put_string(&mut blob, ED25519.as_bytes());
        SshBuf::put_string(&mut blob, &signature);
        blob
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        sha256(&self.public_key_blob())
    }

    /// The `ssh-ed25519 <base64> <comment>` line of a `.pub` file.
    pub fn public_line(&self, comment: &str) -> String {
        public_line(&self.public_key_blob(), comment)
    }

    /// The key in the OpenSSH private-key format, unencrypted. `check` is the
    /// format's random check value.
    pub fn to_openssh(&self, comment: &str, check: u32) -> String {
        let public = self.key.public_key();
        let mut private = Vec::with_capacity(160 + comment.len());
        SshBuf::put_u32(&mut private, check);
        SshBuf::put_u32(&mut private, check);
        SshBuf::put_string(&mut private, ED25519.as_bytes());
        SshBuf::put_string(&mut private, &public);
        let mut secret = [0u8; 64];
        secret[..32].copy_from_slice(self.key.seed());
        secret[32..].copy_from_slice(&public);
        SshBuf::put_string(&mut private, &secret);
        for byte in secret.iter_mut() {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        SshBuf::put_string(&mut private, comment.as_bytes());
        let mut pad = 1u8;
        while private.len() % 8 != 0 {
            private.push(pad);
            pad += 1;
        }

        let mut body = Vec::with_capacity(64 + private.len());
        body.extend_from_slice(OPENSSH_MAGIC);
        SshBuf::put_string(&mut body, b"none");
        SshBuf::put_string(&mut body, b"none");
        SshBuf::put_string(&mut body, b"");
        SshBuf::put_u32(&mut body, 1);
        SshBuf::put_string(&mut body, &self.public_key_blob());
        SshBuf::put_string(&mut body, &private);
        for byte in private.iter_mut() {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }

        let encoded = base64_encode(&body, true);
        let mut text = String::with_capacity(encoded.len() + 80);
        text.push_str(OPENSSH_BEGIN);
        text.push('\n');
        for line in encoded.as_bytes().chunks(70) {
            text.push_str(core::str::from_utf8(line).unwrap_or(""));
            text.push('\n');
        }
        text.push_str(OPENSSH_END);
        text.push('\n');
        text
    }

    /// Read an unencrypted OpenSSH-format Ed25519 private key.
    pub fn from_openssh(text: &str) -> Result<Self, &'static str> {
        let body = decode_pem_block(text, OPENSSH_BEGIN, OPENSSH_END)?;
        if !body.starts_with(OPENSSH_MAGIC) {
            return Err("not an openssh-key-v1 private key");
        }
        let mut pos = OPENSSH_MAGIC.len();
        let field = |pos: &mut usize| SshBuf::get_string(&body, pos).ok_or("truncated private key");
        if field(&mut pos)? != b"none" || field(&mut pos)? != b"none" || !field(&mut pos)?.is_empty() {
            return Err("encrypted private keys are not supported");
        }
        if SshBuf::get_u32(&body, &mut pos) != Some(1) {
            return Err("a private key file must hold one key");
        }
        let public_blob = field(&mut pos)?;
        let private = field(&mut pos)?;
        if pos != body.len() {
            return Err("trailing data after the private key");
        }

        let mut at = 0;
        let check1 = SshBuf::get_u32(private, &mut at).ok_or("truncated private section")?;
        let check2 = SshBuf::get_u32(private, &mut at).ok_or("truncated private section")?;
        if check1 != check2 {
            return Err("private key check values differ");
        }
        let get = |at: &mut usize| SshBuf::get_string(private, at).ok_or("truncated private section");
        if get(&mut at)? != ED25519.as_bytes() {
            return Err("only ssh-ed25519 OpenSSH private keys are supported");
        }
        let public = get(&mut at)?;
        let secret = get(&mut at)?;
        let _comment = get(&mut at)?;
        let padding = &private[at..];
        if padding.len() >= 8 || padding.iter().enumerate().any(|(i, byte)| *byte as usize != i + 1) {
            return Err("bad private key padding");
        }
        if public.len() != 32 || secret.len() != 64 || &secret[32..] != public {
            return Err("bad ssh-ed25519 private key");
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&secret[..32]);
        let key = Self::from_seed(&seed);
        for byte in seed.iter_mut() {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        if key.key.public_key() != public || key.public_key_blob() != public_blob {
            return Err("the private key does not match its public key");
        }
        Ok(key)
    }
}

/// The server's host key.
pub type HostKey = Ed25519Key;

// ==========================================================================
// Client identities
// ==========================================================================

/// Caller-supplied RSA identity for SSH publickey client authentication.
pub struct RsaIdentity {
    key: RsaPrivateKey,
}

impl RsaIdentity {
    /// Load an unencrypted PKCS#1 PEM RSA private key.
    pub fn from_pkcs1_pem(pem: &str) -> Result<Self, &'static str> {
        let der = decode_pem_block(
            pem,
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----END RSA PRIVATE KEY-----",
        )?;
        let (n, e, d) = parse_pkcs1_rsa_private_key(&der)?;
        Ok(Self {
            key: RsaPrivateKey {
                n: BigNum::from_be_bytes(n),
                e: BigNum::from_be_bytes(e),
                d: BigNum::from_be_bytes(d),
            },
        })
    }

    /// Encode this identity's public key in SSH wire format.
    pub fn public_key_blob(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(4 + 7 + 4 + 4 + 4 + 384);
        SshBuf::put_string(&mut blob, RSA.as_bytes());
        SshBuf::put_mpint(&mut blob, &self.key.e.to_be_bytes());
        SshBuf::put_mpint(&mut blob, &self.key.n.to_be_bytes());
        blob
    }

    /// Sign data with rsa-sha2-256, returning an SSH signature blob.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        let hash = sha256(data);
        let sig_bytes = rsa_sign_pkcs1_sha256(&self.key, &hash);

        let mut sig = Vec::with_capacity(4 + 12 + 4 + sig_bytes.len());
        SshBuf::put_string(&mut sig, b"rsa-sha2-256");
        SshBuf::put_string(&mut sig, &sig_bytes);
        sig
    }
}

/// A client's private key for publickey authentication.
pub enum Identity {
    Ed25519(Ed25519Key),
    Rsa(RsaIdentity),
}

impl Identity {
    /// Read an unencrypted private key file: OpenSSH format (Ed25519) or PKCS#1 PEM (RSA).
    pub fn from_file_text(text: &str) -> Result<Self, &'static str> {
        if text.contains(OPENSSH_BEGIN) {
            Ed25519Key::from_openssh(text).map(Identity::Ed25519)
        } else {
            RsaIdentity::from_pkcs1_pem(text).map(Identity::Rsa)
        }
    }

    /// The publickey method's algorithm name for this identity.
    pub fn algorithm(&self) -> &'static str {
        match self {
            Identity::Ed25519(_) => ED25519,
            Identity::Rsa(_) => "rsa-sha2-256",
        }
    }

    pub fn public_key_blob(&self) -> Vec<u8> {
        match self {
            Identity::Ed25519(key) => key.public_key_blob(),
            Identity::Rsa(key) => key.public_key_blob(),
        }
    }

    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        match self {
            Identity::Ed25519(key) => key.sign(data),
            Identity::Rsa(key) => key.sign(data),
        }
    }
}

/// Server host-key identity observed during client key exchange.
#[derive(Debug, Clone)]
pub struct ServerHostKeyInfo {
    /// Raw SSH public-key blob from KEX_ECDH_REPLY.
    pub key_blob: Vec<u8>,
    /// Key type stored inside the key blob, e.g. "ssh-ed25519".
    pub key_type: String,
    /// Signature/host-key algorithm used in KEX, e.g. "ssh-ed25519".
    pub algorithm: String,
    /// SHA-256 fingerprint of `key_blob`.
    pub fingerprint: [u8; 32],
}

impl ServerHostKeyInfo {
    pub fn from_parts(key_blob: Vec<u8>, signature_blob: &[u8]) -> Option<Self> {
        let key_type = key_type_from_blob(&key_blob)?;
        let algorithm = signature_algorithm(signature_blob)?;
        let fingerprint = sha256(&key_blob);

        Some(Self {
            key_blob,
            key_type,
            algorithm,
            fingerprint,
        })
    }
}

/// Parse the key type string from an SSH public-key blob.
pub fn key_type_from_blob(blob: &[u8]) -> Option<String> {
    let mut pos = 0;
    let key_type = SshBuf::get_string(blob, &mut pos)?;
    Some(String::from_utf8_lossy(key_type).into_owned())
}

/// Parse the algorithm string from an SSH signature blob.
pub fn signature_algorithm(signature_blob: &[u8]) -> Option<String> {
    let mut pos = 0;
    let algorithm = SshBuf::get_string(signature_blob, &mut pos)?;
    Some(String::from_utf8_lossy(algorithm).into_owned())
}

// ==========================================================================
// Public-key lines and fingerprints
// ==========================================================================

/// `type base64(blob) comment`, the line `.pub` and `authorized_keys` files hold.
pub fn public_line(blob: &[u8], comment: &str) -> String {
    let key_type = key_type_from_blob(blob).unwrap_or_default();
    let mut line = format!("{} {}", key_type, base64_encode(blob, true));
    if !comment.is_empty() {
        line.push(' ');
        line.push_str(comment);
    }
    line
}

/// Parse `type base64 [comment]`: the blob, when its embedded type is `type`.
/// A line starting with anything but a key type (such as options) is refused.
pub fn parse_public_line(line: &str) -> Option<Vec<u8>> {
    let mut words = line.split_ascii_whitespace();
    let key_type = words.next()?;
    if key_type != ED25519 && key_type != RSA {
        return None;
    }
    let blob = base64_decode(words.next()?.as_bytes()).ok()?;
    if key_type_from_blob(&blob)? != key_type {
        return None;
    }
    Some(blob)
}

/// `SHA256:<unpadded base64>`, as OpenSSH prints fingerprints.
pub fn fingerprint_text(blob: &[u8]) -> String {
    format!("SHA256:{}", base64_encode(&sha256(blob), false))
}

/// Compare two byte strings without stopping at the first difference.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ==========================================================================
// Signature verification
// ==========================================================================

/// Verify `signature_blob` over `data` by the public key `key_blob`, for the
/// signature algorithm `algorithm`: ssh-ed25519 with an Ed25519 key, or
/// rsa-sha2-256/512 with an RSA key. The blob's own algorithm must match.
pub fn verify_signature(algorithm: &[u8], key_blob: &[u8], signature_blob: &[u8], data: &[u8]) -> bool {
    let mut pos = 0;
    if SshBuf::get_string(signature_blob, &mut pos) != Some(algorithm) {
        return false;
    }
    match algorithm {
        b"ssh-ed25519" => verify_ed25519_signature(key_blob, signature_blob, data),
        b"rsa-sha2-256" | b"rsa-sha2-512" => verify_rsa_signature(key_blob, signature_blob, data),
        _ => false,
    }
}

/// Verify an ssh-ed25519 signature blob with an ssh-ed25519 public-key blob.
pub fn verify_ed25519_signature(key_blob: &[u8], signature_blob: &[u8], data: &[u8]) -> bool {
    let mut pos = 0;
    if SshBuf::get_string(key_blob, &mut pos) != Some(ED25519.as_bytes()) {
        return false;
    }
    let Some(public) = SshBuf::get_string(key_blob, &mut pos) else { return false };
    if pos != key_blob.len() {
        return false;
    }
    let mut pos = 0;
    if SshBuf::get_string(signature_blob, &mut pos) != Some(ED25519.as_bytes()) {
        return false;
    }
    let Some(signature) = SshBuf::get_string(signature_blob, &mut pos) else { return false };
    if pos != signature_blob.len() {
        return false;
    }
    let (Ok(public), Ok(signature)) = (<[u8; 32]>::try_from(public), <[u8; 64]>::try_from(signature)) else {
        return false;
    };
    ed25519::verify(&public, data, &signature)
}

/// Parse an SSH RSA public key blob and extract the public key components.
///
/// Parses: string "ssh-rsa" || mpint e || mpint n
pub fn parse_rsa_pubkey_blob(blob: &[u8]) -> Option<RsaPublicKey> {
    let mut pos = 0;
    let key_type = SshBuf::get_string(blob, &mut pos)?;

    if key_type != RSA.as_bytes() {
        return None;
    }

    let e_bytes = SshBuf::get_string(blob, &mut pos)?;
    let n_bytes = SshBuf::get_string(blob, &mut pos)?;

    Some(RsaPublicKey {
        e: BigNum::from_be_bytes(e_bytes),
        n: BigNum::from_be_bytes(n_bytes),
    })
}

/// Verify an RSA signature over data using a parsed SSH public key blob.
///
/// The signature is in SSH wire format: string algorithm || string sig_blob,
/// with algorithm rsa-sha2-256 or rsa-sha2-512.
pub fn verify_rsa_signature(pubkey_blob: &[u8], signature_blob: &[u8], data: &[u8]) -> bool {
    let pubkey = match parse_rsa_pubkey_blob(pubkey_blob) {
        Some(k) => k,
        None => return false,
    };

    let mut pos = 0;
    let algo = match SshBuf::get_string(signature_blob, &mut pos) {
        Some(a) => a,
        None => return false,
    };
    let raw_sig = match SshBuf::get_string(signature_blob, &mut pos) {
        Some(s) => s,
        None => return false,
    };

    if pos != signature_blob.len() {
        return false;
    }

    if algo == b"rsa-sha2-256" {
        let hash = sha256(data);
        rsa_verify_pkcs1_sha256(&pubkey, raw_sig, &hash)
    } else if algo == b"rsa-sha2-512" {
        let hash = sha512(data);
        rsa_verify_pkcs1_sha512(&pubkey, raw_sig, &hash)
    } else {
        false
    }
}

// ==========================================================================
// PEM, base64 and DER
// ==========================================================================

fn decode_pem_block(
    pem: &str,
    begin_marker: &str,
    end_marker: &str,
) -> Result<Vec<u8>, &'static str> {
    let begin = pem.find(begin_marker).ok_or("missing PEM begin marker")?;
    let body_start = begin + begin_marker.len();
    let end = pem[body_start..]
        .find(end_marker)
        .ok_or("missing PEM end marker")?
        + body_start;

    let mut b64 = Vec::new();
    for byte in pem[body_start..end].bytes() {
        if byte == b'\r' || byte == b'\n' || byte == b'\t' || byte == b' ' {
            continue;
        }
        b64.push(byte);
    }

    base64_decode(&b64)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 (RFC 4648 section 4), padded or not.
pub fn base64_encode(input: &[u8], pad: bool) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(value >> (18 - 6 * i)) as usize & 63] as char);
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// Decode padded base64 (RFC 4648 section 4).
pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut chunk_len = 0usize;
    let mut ended = false;

    for &byte in input {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err("invalid base64 character"),
        };
        if ended {
            return Err("data after base64 padding");
        }

        chunk[chunk_len] = value;
        chunk_len += 1;
        if chunk_len != 4 {
            continue;
        }

        if chunk[0] == 64 || chunk[1] == 64 || (chunk[2] == 64 && chunk[3] != 64) {
            return Err("invalid base64 padding");
        }

        out.push((chunk[0] << 2) | (chunk[1] >> 4));
        if chunk[2] != 64 {
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        if chunk[3] != 64 {
            out.push((chunk[2] << 6) | chunk[3]);
        }
        ended = chunk[3] == 64;

        chunk_len = 0;
    }

    if chunk_len != 0 {
        return Err("truncated base64 data");
    }

    Ok(out)
}

fn parse_pkcs1_rsa_private_key(der: &[u8]) -> Result<(&[u8], &[u8], &[u8]), &'static str> {
    let mut reader = DerReader::new(der);
    let seq = reader.read_constructed(0x30)?;
    if !reader.is_empty() {
        return Err("trailing data after private key");
    }

    let mut seq_reader = DerReader::new(seq);
    let _version = seq_reader.read_integer()?;
    let n = seq_reader.read_integer()?;
    let e = seq_reader.read_integer()?;
    let d = seq_reader.read_integer()?;
    Ok((n, e, d))
}

struct DerReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.data.len()
    }

    fn read_constructed(&mut self, tag: u8) -> Result<&'a [u8], &'static str> {
        self.read_tlv(tag)
    }

    fn read_integer(&mut self) -> Result<&'a [u8], &'static str> {
        let value = self.read_tlv(0x02)?;
        Ok(strip_der_integer_prefix(value))
    }

    fn read_tlv(&mut self, expected_tag: u8) -> Result<&'a [u8], &'static str> {
        if self.pos >= self.data.len() {
            return Err("unexpected end of DER");
        }
        let tag = self.data[self.pos];
        self.pos += 1;
        if tag != expected_tag {
            return Err("unexpected DER tag");
        }

        let len = self.read_len()?;
        if self.pos + len > self.data.len() {
            return Err("DER length exceeds input");
        }
        let start = self.pos;
        self.pos += len;
        Ok(&self.data[start..start + len])
    }

    fn read_len(&mut self) -> Result<usize, &'static str> {
        if self.pos >= self.data.len() {
            return Err("missing DER length");
        }
        let first = self.data[self.pos];
        self.pos += 1;

        if first & 0x80 == 0 {
            return Ok(first as usize);
        }

        let count = (first & 0x7f) as usize;
        if count == 0 || count > core::mem::size_of::<usize>() || self.pos + count > self.data.len()
        {
            return Err("invalid DER length");
        }

        let mut len = 0usize;
        for _ in 0..count {
            len = (len << 8) | self.data[self.pos] as usize;
            self.pos += 1;
        }
        Ok(len)
    }
}

fn strip_der_integer_prefix(value: &[u8]) -> &[u8] {
    if value.len() > 1 && value[0] == 0 {
        &value[1..]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bssh-keys-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ssh_keygen(args: &[&str]) -> std::process::Output {
        let output = Command::new("ssh-keygen").args(args).output()
            .expect("ssh-keygen (OpenSSH) is a host-test prerequisite");
        assert!(output.status.success(), "ssh-keygen {:?}: {}", args, String::from_utf8_lossy(&output.stderr));
        output
    }

    #[test]
    fn written_host_key_is_read_back_by_openssh_and_by_us() {
        let dir = scratch("write");
        let key = Ed25519Key::from_seed(&[7u8; 32]);
        let path = dir.join("ssh_host_ed25519_key");
        std::fs::write(&path, key.to_openssh("breenix host key", 0x1234_5678)).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let derived = ssh_keygen(&["-y", "-f", path.to_str().unwrap()]);
        let line = String::from_utf8(derived.stdout).unwrap();
        assert_eq!(parse_public_line(line.trim()).unwrap(), key.public_key_blob());
        let reread = Ed25519Key::from_openssh(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reread.public_key_blob(), key.public_key_blob());
        let fingerprint = ssh_keygen(&["-l", "-E", "sha256", "-f", path.to_str().unwrap()]);
        let fingerprint = String::from_utf8(fingerprint.stdout).unwrap();
        assert!(fingerprint.contains(&fingerprint_text(&key.public_key_blob())), "{fingerprint}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn openssh_generated_identity_loads_and_signs_verifiably() {
        let dir = scratch("load");
        let path = dir.join("id_ed25519");
        ssh_keygen(&["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f", path.to_str().unwrap()]);
        let identity = Identity::from_file_text(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let public = std::fs::read_to_string(dir.join("id_ed25519.pub")).unwrap();
        assert_eq!(parse_public_line(public.trim()).unwrap(), identity.public_key_blob());
        let signature = identity.sign(b"session data");
        assert!(verify_signature(b"ssh-ed25519", &identity.public_key_blob(), &signature, b"session data"));
        assert!(!verify_signature(b"ssh-ed25519", &identity.public_key_blob(), &signature, b"other data"));
        assert!(!verify_signature(b"rsa-sha2-256", &identity.public_key_blob(), &signature, b"session data"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn openssl_signed_rsa_4096_user_key_verifies_with_sha2_512_and_256() {
        let dir = scratch("rsa");
        let path = dir.join("id_rsa");
        ssh_keygen(&["-q", "-t", "rsa", "-b", "4096", "-m", "PEM", "-N", "", "-f", path.to_str().unwrap()]);
        let blob = parse_public_line(std::fs::read_to_string(dir.join("id_rsa.pub")).unwrap().trim()).unwrap();
        let data = dir.join("data");
        std::fs::write(&data, b"publickey request data").unwrap();
        for (digest, algorithm) in [("-sha512", "rsa-sha2-512"), ("-sha256", "rsa-sha2-256")] {
            let output = Command::new("openssl").args(["dgst", digest, "-sign", path.to_str().unwrap(), data.to_str().unwrap()])
                .output().expect("openssl is a host-test prerequisite");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let mut signature = Vec::new();
            SshBuf::put_string(&mut signature, algorithm.as_bytes());
            SshBuf::put_string(&mut signature, &output.stdout);
            assert!(verify_signature(algorithm.as_bytes(), &blob, &signature, b"publickey request data"), "{algorithm}");
            assert!(!verify_signature(algorithm.as_bytes(), &blob, &signature, b"other data"), "{algorithm}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn encrypted_or_altered_private_keys_are_refused() {
        let dir = scratch("refuse");
        let path = dir.join("id_ed25519");
        ssh_keygen(&["-q", "-t", "ed25519", "-N", "a passphrase", "-f", path.to_str().unwrap()]);
        assert!(Identity::from_file_text(&std::fs::read_to_string(&path).unwrap()).is_err());
        let text = Ed25519Key::from_seed(&[9u8; 32]).to_openssh("", 1);
        let mut body = decode_pem_block(&text, OPENSSH_BEGIN, OPENSSH_END).unwrap();
        let last = body.len() - 20;
        body[last] ^= 1;
        let altered = format!("{}\n{}\n{}\n", OPENSSH_BEGIN, base64_encode(&body, true), OPENSSH_END);
        assert!(Ed25519Key::from_openssh(&altered).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn public_lines_need_a_known_type_that_matches_the_blob() {
        let blob = Ed25519Key::from_seed(&[1u8; 32]).public_key_blob();
        let line = public_line(&blob, "operator@host");
        assert_eq!(parse_public_line(&line), Some(blob.clone()));
        assert_eq!(parse_public_line(&line.replacen("ssh-ed25519", "ssh-rsa", 1)), None);
        assert_eq!(parse_public_line(&format!("command=\"true\" {}", line)), None);
        assert_eq!(parse_public_line("ssh-ed25519 !!!!"), None);
        for size in 0..6 {
            let data: Vec<u8> = (0..size as u8).collect();
            assert_eq!(base64_decode(base64_encode(&data, true).as_bytes()).unwrap(), data);
        }
    }
}
