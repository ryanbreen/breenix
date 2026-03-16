//! SSH key exchange: curve25519-sha256 (RFC 8731)
//!
//! Implements the Elliptic Curve Diffie-Hellman key exchange using Curve25519,
//! with SHA-256 as the hash function. Derives session keys for AES-128-CTR
//! encryption and HMAC-SHA256 integrity.

use crate::crypto::mlkem;
use crate::crypto::rand::Csprng;
use crate::crypto::sha256::sha256;
use crate::crypto::x25519::{x25519, x25519_basepoint};

use super::cipher::SshCipher;
use super::keys::HostKey;
use super::packet::PacketIo;
use super::SshBuf;
use super::*;

/// Algorithms offered by BSSH.
// ext-info-s tells clients we'll send SSH_MSG_EXT_INFO after NEWKEYS (RFC 8308)
// TODO: mlkem768x25519-sha256 implementation complete but exchange hash computation
// doesn't match OpenSSH yet (incorrect signature). Disabled until fixed.
// Full ML-KEM 768 + Keccak/SHAKE primitives are in crypto/ and server_kex_hybrid()
// in this file is ready — just needs the hash encoding debugged.
pub const KEX_ALGORITHMS: &str = "curve25519-sha256,ext-info-s";
pub const HOST_KEY_ALGORITHMS: &str = "rsa-sha2-256,ssh-rsa";
pub const CIPHERS: &str = "aes128-ctr";
pub const MACS: &str = "hmac-sha2-256";
pub const COMPRESSION: &str = "none";

/// Key exchange state.
pub struct KexState {
    /// Our KEXINIT payload (for exchange hash computation).
    pub my_kexinit: Vec<u8>,
    /// Peer's KEXINIT payload.
    pub peer_kexinit: Vec<u8>,
    /// Exchange hash (H) — also the session_id for the first KEX.
    pub session_id: Option<Vec<u8>>,
}

impl KexState {
    pub fn new() -> Self {
        Self {
            my_kexinit: Vec::new(),
            peer_kexinit: Vec::new(),
            session_id: None,
        }
    }
}

/// Build a KEXINIT message payload.
pub fn build_kexinit(rng: &mut Csprng) -> Vec<u8> {
    let mut payload = Vec::with_capacity(256);

    // Message type
    payload.push(SSH_MSG_KEXINIT);

    // 16 bytes of random cookie
    let mut cookie = [0u8; 16];
    rng.fill(&mut cookie);
    payload.extend_from_slice(&cookie);

    // Algorithm name-lists (10 of them)
    SshBuf::put_string(&mut payload, KEX_ALGORITHMS.as_bytes());
    SshBuf::put_string(&mut payload, HOST_KEY_ALGORITHMS.as_bytes());
    SshBuf::put_string(&mut payload, CIPHERS.as_bytes()); // encryption C->S
    SshBuf::put_string(&mut payload, CIPHERS.as_bytes()); // encryption S->C
    SshBuf::put_string(&mut payload, MACS.as_bytes()); // MAC C->S
    SshBuf::put_string(&mut payload, MACS.as_bytes()); // MAC S->C
    SshBuf::put_string(&mut payload, COMPRESSION.as_bytes()); // compression C->S
    SshBuf::put_string(&mut payload, COMPRESSION.as_bytes()); // compression S->C
    SshBuf::put_string(&mut payload, b""); // languages C->S
    SshBuf::put_string(&mut payload, b""); // languages S->C

    // first_kex_packet_follows
    SshBuf::put_bool(&mut payload, false);

    // reserved (uint32)
    SshBuf::put_u32(&mut payload, 0);

    payload
}

/// Perform server-side key exchange.
///
/// Expects the client's KEX_ECDH_INIT message and responds with
/// KEX_ECDH_REPLY containing the host key, server's DH public key,
/// and signature over the exchange hash.
///
/// Returns the exchange hash H and the shared secret K.
pub fn server_kex_ecdh(
    io: &mut PacketIo,
    host_key: &HostKey,
    kex: &mut KexState,
    client_version: &str,
    server_version: &str,
    client_ecdh_init: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), SshError> {
    // Parse client's ephemeral public key from KEX_ECDH_INIT
    // Format: byte SSH_MSG_KEX_ECDH_INIT, string Q_C
    let mut pos = 1; // skip message type byte
    let q_c = SshBuf::get_string(client_ecdh_init, &mut pos)
        .ok_or(SshError::Protocol("bad KEX_ECDH_INIT"))?;

    if q_c.len() != 32 {
        return Err(SshError::Protocol("invalid client DH key length"));
    }

    // Generate server's ephemeral key pair
    let mut rng = Csprng::new();
    let mut server_secret = [0u8; 32];
    rng.fill(&mut server_secret);
    let server_public = x25519_basepoint(&server_secret);

    // Compute shared secret
    let mut q_c_arr = [0u8; 32];
    q_c_arr.copy_from_slice(q_c);
    let shared_secret = x25519(&server_secret, &q_c_arr);

    // Check for all-zero shared secret (invalid point)
    if shared_secret.iter().all(|&b| b == 0) {
        return Err(SshError::KeyExchange);
    }

    // Get host key blob
    let host_key_blob = host_key.public_key_blob();

    // Compute exchange hash H = SHA-256(V_C || V_S || I_C || I_S || K_S || Q_C || Q_S || K)
    let h = compute_exchange_hash(
        client_version,
        server_version,
        &kex.peer_kexinit,
        &kex.my_kexinit,
        &host_key_blob,
        q_c,
        &server_public,
        &shared_secret,
    );

    // If this is the first KEX, the exchange hash becomes the session_id
    if kex.session_id.is_none() {
        kex.session_id = Some(h.clone());
    }

    // Sign the exchange hash
    let signature = host_key.sign(&h);

    // Build KEX_ECDH_REPLY: string K_S, string Q_S, string signature
    let mut reply = Vec::with_capacity(512);
    reply.push(SSH_MSG_KEX_ECDH_REPLY);
    SshBuf::put_string(&mut reply, &host_key_blob);
    SshBuf::put_string(&mut reply, &server_public);
    SshBuf::put_string(&mut reply, &signature);

    io.send_packet(&reply).map_err(|_| SshError::Io)?;

    // Send NEWKEYS
    io.send_packet(&[SSH_MSG_NEWKEYS])
        .map_err(|_| SshError::Io)?;

    Ok((h, shared_secret.to_vec()))
}

/// Server-side hybrid post-quantum key exchange (mlkem768x25519-sha256).
pub fn server_kex_hybrid(
    io: &mut PacketIo,
    host_key: &HostKey,
    kex: &mut KexState,
    client_version: &str,
    server_version: &str,
    client_init: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), SshError> {
    let mut pos = 1;
    let c_init = SshBuf::get_string(client_init, &mut pos)
        .ok_or(SshError::Protocol("bad KEX_HYBRID_INIT"))?;
    if c_init.len() != 1216 {
        return Err(SshError::Protocol("invalid hybrid C_INIT length"));
    }

    let mut rng = Csprng::new();

    // X25519
    let mut x25519_secret = [0u8; 32];
    rng.fill(&mut x25519_secret);
    let x25519_public = x25519_basepoint(&x25519_secret);
    let mut client_x25519 = [0u8; 32];
    client_x25519.copy_from_slice(&c_init[1184..]);
    let k_cl = x25519(&x25519_secret, &client_x25519);
    if k_cl.iter().all(|&b| b == 0) { return Err(SshError::KeyExchange); }

    // ML-KEM 768 encapsulation
    let mut pk_bytes = [0u8; 1184];
    pk_bytes.copy_from_slice(&c_init[..1184]);
    let mlkem_pk = mlkem::MlKemPublicKey { bytes: pk_bytes };
    let mut encap_rand = [0u8; 32];
    rng.fill(&mut encap_rand);
    let (ct, k_pq) = mlkem::encapsulate(&mlkem_pk, &encap_rand);

    // K = SHA-256(K_PQ || K_CL)
    let mut k_input = [0u8; 64];
    k_input[..32].copy_from_slice(&k_pq);
    k_input[32..].copy_from_slice(&k_cl);
    let shared_secret = sha256(&k_input);

    // S_REPLY = ML-KEM ct (1088) || X25519 pk (32)
    let mut s_reply = Vec::with_capacity(1120);
    s_reply.extend_from_slice(&ct.bytes);
    s_reply.extend_from_slice(&x25519_public);

    let host_key_blob = host_key.public_key_blob();
    let h = compute_exchange_hash_hybrid(
        client_version, server_version,
        &kex.peer_kexinit, &kex.my_kexinit,
        &host_key_blob, c_init, &s_reply, &shared_secret,
    );
    if kex.session_id.is_none() { kex.session_id = Some(h.clone()); }

    let signature = host_key.sign(&h);
    let mut reply = Vec::with_capacity(2048);
    reply.push(SSH_MSG_KEX_ECDH_REPLY);
    SshBuf::put_string(&mut reply, &host_key_blob);
    SshBuf::put_string(&mut reply, &s_reply);
    SshBuf::put_string(&mut reply, &signature);
    io.send_packet(&reply).map_err(|_| SshError::Io)?;
    io.send_packet(&[SSH_MSG_NEWKEYS]).map_err(|_| SshError::Io)?;

    Ok((h, shared_secret.to_vec()))
}

/// Perform client-side key exchange.
///
/// Sends KEX_ECDH_INIT with the client's ephemeral public key and
/// processes the server's KEX_ECDH_REPLY.
///
/// Returns the exchange hash H, shared secret K, and server's host key blob.
pub fn client_kex_ecdh(
    io: &mut PacketIo,
    kex: &mut KexState,
    client_version: &str,
    server_version: &str,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), SshError> {
    // Generate client's ephemeral key pair
    let mut rng = Csprng::new();
    let mut client_secret = [0u8; 32];
    rng.fill(&mut client_secret);
    let client_public = x25519_basepoint(&client_secret);

    // Send KEX_ECDH_INIT: string Q_C
    let mut init = Vec::with_capacity(37);
    init.push(SSH_MSG_KEX_ECDH_INIT);
    SshBuf::put_string(&mut init, &client_public);
    io.send_packet(&init).map_err(|_| SshError::Io)?;

    // Receive KEX_ECDH_REPLY
    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_KEX_ECDH_REPLY {
        return Err(SshError::Protocol("expected KEX_ECDH_REPLY"));
    }

    // Parse: string K_S, string Q_S, string signature
    let mut pos = 1;
    let host_key_blob = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad host key in KEX_ECDH_REPLY"))?
        .to_vec();
    let q_s = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad server DH key in KEX_ECDH_REPLY"))?;
    let signature = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad signature in KEX_ECDH_REPLY"))?;

    if q_s.len() != 32 {
        return Err(SshError::Protocol("invalid server DH key length"));
    }

    // Compute shared secret
    let mut q_s_arr = [0u8; 32];
    q_s_arr.copy_from_slice(q_s);
    let shared_secret = x25519(&client_secret, &q_s_arr);

    if shared_secret.iter().all(|&b| b == 0) {
        return Err(SshError::KeyExchange);
    }

    // Compute exchange hash
    let h = compute_exchange_hash(
        client_version,
        server_version,
        &kex.my_kexinit,
        &kex.peer_kexinit,
        &host_key_blob,
        &client_public,
        q_s,
        &shared_secret,
    );

    if kex.session_id.is_none() {
        kex.session_id = Some(h.clone());
    }

    // Verify the server's signature over the exchange hash
    if !super::keys::verify_rsa_signature(&host_key_blob, signature, &h) {
        return Err(SshError::Protocol("host key signature verification failed"));
    }

    Ok((h, shared_secret.to_vec(), host_key_blob))
}

/// Compute the exchange hash H per RFC 8731.
///
/// H = SHA-256(V_C || V_S || I_C || I_S || K_S || Q_C || Q_S || K)
///
/// All values are encoded as SSH strings (uint32 length prefix) except K
/// which is encoded as an SSH mpint for classical KEX.
fn compute_exchange_hash(
    v_c: &str,
    v_s: &str,
    i_c: &[u8],
    i_s: &[u8],
    k_s: &[u8],
    q_c: &[u8],
    q_s: &[u8],
    k: &[u8],
) -> Vec<u8> {
    compute_exchange_hash_inner(v_c, v_s, i_c, i_s, k_s, q_c, q_s, k, false)
}

/// Exchange hash for hybrid KEX — K is encoded as string, not mpint.
fn compute_exchange_hash_hybrid(
    v_c: &str,
    v_s: &str,
    i_c: &[u8],
    i_s: &[u8],
    k_s: &[u8],
    c_init: &[u8],
    s_reply: &[u8],
    k: &[u8],
) -> Vec<u8> {
    compute_exchange_hash_inner(v_c, v_s, i_c, i_s, k_s, c_init, s_reply, k, true)
}

fn compute_exchange_hash_inner(
    v_c: &str,
    v_s: &str,
    i_c: &[u8],
    i_s: &[u8],
    k_s: &[u8],
    q_c: &[u8],
    q_s: &[u8],
    k: &[u8],
    k_as_string: bool,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(2048);

    SshBuf::put_string(&mut data, v_c.as_bytes());
    SshBuf::put_string(&mut data, v_s.as_bytes());
    SshBuf::put_string(&mut data, i_c);
    SshBuf::put_string(&mut data, i_s);
    SshBuf::put_string(&mut data, k_s);
    SshBuf::put_string(&mut data, q_c);
    SshBuf::put_string(&mut data, q_s);
    if k_as_string {
        // Hybrid KEX: K is the 32-byte SHA-256 output, encoded as SSH string
        SshBuf::put_string(&mut data, k);
    } else {
        // Classical KEX: K is the raw DH output, encoded as SSH mpint
        SshBuf::put_mpint(&mut data, k);
    }

    sha256(&data).to_vec()
}

/// Derive session keys from the shared secret and exchange hash (RFC 4253 §7.2).
///
/// Derives:
/// - IV for client→server (16 bytes, letter 'A')
/// - IV for server→client (16 bytes, letter 'B')
/// - Encryption key client→server (16 bytes, letter 'C')
/// - Encryption key server→client (16 bytes, letter 'D')
/// - Integrity key client→server (32 bytes, letter 'E')
/// - Integrity key server→client (32 bytes, letter 'F')
pub fn derive_keys(
    shared_secret: &[u8],
    exchange_hash: &[u8],
    session_id: &[u8],
) -> (SshCipher, SshCipher) {
    derive_keys_inner(shared_secret, exchange_hash, session_id, false)
}

/// Derive session keys for hybrid KEX (K encoded as string, not mpint).
pub fn derive_keys_hybrid(
    shared_secret: &[u8],
    exchange_hash: &[u8],
    session_id: &[u8],
) -> (SshCipher, SshCipher) {
    derive_keys_inner(shared_secret, exchange_hash, session_id, true)
}

fn derive_keys_inner(
    shared_secret: &[u8],
    exchange_hash: &[u8],
    session_id: &[u8],
    k_as_string: bool,
) -> (SshCipher, SshCipher) {
    let derive = |letter: u8, len: usize| -> Vec<u8> {
        let mut data = Vec::with_capacity(256);
        if k_as_string {
            SshBuf::put_string(&mut data, shared_secret);
        } else {
            SshBuf::put_mpint(&mut data, shared_secret);
        }
        data.extend_from_slice(exchange_hash);
        data.push(letter);
        data.extend_from_slice(session_id);

        let k1 = sha256(&data);
        if len <= 32 {
            k1[..len].to_vec()
        } else {
            // Extend key material: K2 = HASH(K || H || K1), etc.
            let mut result = k1.to_vec();
            while result.len() < len {
                let mut ext_data = Vec::new();
                if k_as_string {
                    SshBuf::put_string(&mut ext_data, shared_secret);
                } else {
                    SshBuf::put_mpint(&mut ext_data, shared_secret);
                }
                ext_data.extend_from_slice(exchange_hash);
                ext_data.extend_from_slice(&result);
                let kn = sha256(&ext_data);
                result.extend_from_slice(&kn);
            }
            result.truncate(len);
            result
        }
    };

    // Derive all keys
    let iv_c2s = derive(b'A', 16);
    let iv_s2c = derive(b'B', 16);
    let enc_c2s = derive(b'C', 16);
    let enc_s2c = derive(b'D', 16);
    let mac_c2s = derive(b'E', 32);
    let mac_s2c = derive(b'F', 32);

    let mut iv_c2s_arr = [0u8; 16];
    let mut iv_s2c_arr = [0u8; 16];
    let mut enc_c2s_arr = [0u8; 16];
    let mut enc_s2c_arr = [0u8; 16];
    let mut mac_c2s_arr = [0u8; 32];
    let mut mac_s2c_arr = [0u8; 32];

    iv_c2s_arr.copy_from_slice(&iv_c2s);
    iv_s2c_arr.copy_from_slice(&iv_s2c);
    enc_c2s_arr.copy_from_slice(&enc_c2s);
    enc_s2c_arr.copy_from_slice(&enc_s2c);
    mac_c2s_arr.copy_from_slice(&mac_c2s);
    mac_s2c_arr.copy_from_slice(&mac_s2c);

    let cipher_c2s = SshCipher::new(&enc_c2s_arr, &iv_c2s_arr, &mac_c2s_arr);
    let cipher_s2c = SshCipher::new(&enc_s2c_arr, &iv_s2c_arr, &mac_s2c_arr);

    (cipher_c2s, cipher_s2c)
}
