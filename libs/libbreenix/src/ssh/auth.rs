//! SSH authentication layer (RFC 4252)
//!
//! Implements public key and password authentication for the SSH user
//! authentication protocol. Public key auth is tried first; password
//! auth is the fallback.

use super::keys;
use super::packet::PacketIo;
use super::{SshBuf, SshError};
use super::{SSH_MSG_DEBUG, SSH_MSG_EXT_INFO, SSH_MSG_IGNORE};
use super::{SSH_MSG_SERVICE_ACCEPT, SSH_MSG_SERVICE_REQUEST};
use super::{
    SSH_MSG_USERAUTH_BANNER, SSH_MSG_USERAUTH_FAILURE, SSH_MSG_USERAUTH_REQUEST,
    SSH_MSG_USERAUTH_SUCCESS,
};

/// SSH_MSG_USERAUTH_PK_OK (RFC 4252 §7)
const SSH_MSG_USERAUTH_PK_OK: u8 = 60;
/// SSH_MSG_USERAUTH_INFO_REQUEST / INFO_RESPONSE (RFC 4256 §3)
const SSH_MSG_USERAUTH_INFO_REQUEST: u8 = 60;
const SSH_MSG_USERAUTH_INFO_RESPONSE: u8 = 61;

/// Handle the "ssh-userauth" service request (server side).
///
/// Skips any SSH_MSG_EXT_INFO (7), SSH_MSG_IGNORE (2), or SSH_MSG_DEBUG (4)
/// messages that the client may send after NEWKEYS before the SERVICE_REQUEST.
pub fn server_accept_service(io: &mut PacketIo) -> Result<(), SshError> {
    let msg = loop {
        let pkt = io.recv_packet().map_err(|_| SshError::Io)?;
        if pkt.is_empty() {
            return Err(SshError::Protocol(
                "empty packet waiting for SERVICE_REQUEST",
            ));
        }
        // Skip informational messages that arrive before SERVICE_REQUEST
        match pkt[0] {
            2 | 4 | 7 => continue, // IGNORE, DEBUG, EXT_INFO
            _ => break pkt,
        }
    };
    if msg[0] != SSH_MSG_SERVICE_REQUEST {
        return Err(SshError::Protocol("expected SERVICE_REQUEST"));
    }

    let mut pos = 1;
    let service =
        SshBuf::get_string(&msg, &mut pos).ok_or(SshError::Protocol("bad SERVICE_REQUEST"))?;

    if service != b"ssh-userauth" {
        return Err(SshError::Protocol("unknown service requested"));
    }

    let mut reply = Vec::with_capacity(20);
    reply.push(SSH_MSG_SERVICE_ACCEPT);
    SshBuf::put_string(&mut reply, b"ssh-userauth");
    io.send_packet(&reply).map_err(|_| SshError::Io)?;

    Ok(())
}

/// Handle authentication (server side).
///
/// Supports two methods in priority order:
/// 1. **publickey** — verifies the client's RSA signature against authorized keys
/// 2. **password** — accepts password "breenix" (development fallback)
///
/// The `session_id` is required for public key signature verification.
pub fn server_authenticate(io: &mut PacketIo, session_id: &[u8]) -> Result<String, SshError> {
    loop {
        let msg = io.recv_packet().map_err(|_| SshError::Io)?;
        if msg.is_empty() {
            return Err(SshError::Protocol("empty auth message"));
        }

        if msg[0] != SSH_MSG_USERAUTH_REQUEST {
            continue;
        }

        let mut pos = 1;
        let username =
            SshBuf::get_string(&msg, &mut pos).ok_or(SshError::Protocol("bad username"))?;
        let service =
            SshBuf::get_string(&msg, &mut pos).ok_or(SshError::Protocol("bad service"))?;
        let method = SshBuf::get_string(&msg, &mut pos).ok_or(SshError::Protocol("bad method"))?;

        if service != b"ssh-connection" {
            return Err(SshError::Protocol("unsupported service"));
        }

        let username_str = String::from_utf8_lossy(username).into_owned();

        if method == b"publickey" {
            let has_signature = SshBuf::get_bool(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad publickey has_signature"))?;
            let algo = SshBuf::get_string(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad publickey algorithm"))?;
            let key_blob = SshBuf::get_string(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad publickey blob"))?;

            println!(
                "bsshd: pubkey auth: algo={} blob_len={} has_sig={}",
                String::from_utf8_lossy(algo),
                key_blob.len(),
                has_signature
            );

            // Check if this key is in our authorized_keys
            if !keys::is_authorized_key(key_blob) {
                println!("bsshd: key NOT in authorized_keys");
                send_auth_failure(io, false)?;
                continue;
            }
            println!("bsshd: key MATCHES authorized_keys!");

            if !has_signature {
                // Query phase: client asks "would you accept this key?"
                // Respond with PK_OK
                let mut pk_ok = Vec::with_capacity(4 + algo.len() + 4 + key_blob.len() + 1);
                pk_ok.push(SSH_MSG_USERAUTH_PK_OK);
                SshBuf::put_string(&mut pk_ok, algo);
                SshBuf::put_string(&mut pk_ok, key_blob);
                io.send_packet(&pk_ok).map_err(|_| SshError::Io)?;
                continue;
            }

            // Signing phase: verify the client's signature
            let signature = SshBuf::get_string(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad publickey signature"))?;

            // Build the data that was signed (RFC 4252 §7):
            //   string    session_id
            //   byte      SSH_MSG_USERAUTH_REQUEST (50)
            //   string    user name
            //   string    "ssh-connection"
            //   string    "publickey"
            //   boolean   TRUE
            //   string    algorithm name
            //   string    public key blob
            let mut signed_data = Vec::with_capacity(256);
            SshBuf::put_string(&mut signed_data, session_id);
            signed_data.push(SSH_MSG_USERAUTH_REQUEST);
            SshBuf::put_string(&mut signed_data, username);
            SshBuf::put_string(&mut signed_data, b"ssh-connection");
            SshBuf::put_string(&mut signed_data, b"publickey");
            SshBuf::put_bool(&mut signed_data, true);
            SshBuf::put_string(&mut signed_data, algo);
            SshBuf::put_string(&mut signed_data, key_blob);

            if keys::verify_rsa_signature(key_blob, signature, &signed_data) {
                io.send_packet(&[SSH_MSG_USERAUTH_SUCCESS])
                    .map_err(|_| SshError::Io)?;
                return Ok(username_str);
            }

            // Signature verification failed
            send_auth_failure(io, false)?;
        } else if method == b"password" {
            let _change = SshBuf::get_bool(&msg, &mut pos);
            let password = SshBuf::get_string(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad password field"))?;

            if password == b"breenix" {
                io.send_packet(&[SSH_MSG_USERAUTH_SUCCESS])
                    .map_err(|_| SshError::Io)?;
                return Ok(username_str);
            }

            send_auth_failure(io, false)?;
        } else if method == b"none" {
            // "none" method is used to query supported methods
            send_auth_failure(io, false)?;
        } else {
            send_auth_failure(io, false)?;
        }
    }
}

/// Send USERAUTH_FAILURE with the list of supported methods.
fn send_auth_failure(io: &mut PacketIo, partial_success: bool) -> Result<(), SshError> {
    let mut failure = Vec::with_capacity(32);
    failure.push(SSH_MSG_USERAUTH_FAILURE);
    SshBuf::put_string(&mut failure, b"publickey,password");
    SshBuf::put_bool(&mut failure, partial_success);
    io.send_packet(&failure).map_err(|_| SshError::Io)?;
    Ok(())
}

// ==========================================================================
// Client-side authentication
// ==========================================================================

/// Request the ssh-userauth service (client side).
pub fn client_request_service(io: &mut PacketIo) -> Result<(), SshError> {
    let mut req = Vec::with_capacity(20);
    req.push(SSH_MSG_SERVICE_REQUEST);
    SshBuf::put_string(&mut req, b"ssh-userauth");
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_SERVICE_ACCEPT {
        return Err(SshError::Protocol("service request rejected"));
    }

    Ok(())
}

/// Authenticate with a password (client side).
pub fn client_auth_password(
    io: &mut PacketIo,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    println!("bssh: userauth request method=password user='{}'", username);

    let mut req = Vec::with_capacity(64);
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"password");
    SshBuf::put_bool(&mut req, false);
    SshBuf::put_string(&mut req, password.as_bytes());
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty auth response"));
    }

    match reply[0] {
        SSH_MSG_USERAUTH_SUCCESS => Ok(()),
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            if failure.method_allowed("keyboard-interactive") {
                println!("bssh: retrying auth with keyboard-interactive");
                client_auth_keyboard_interactive(io, username, password)
            } else {
                Err(SshError::Auth)
            }
        }
        _ => Err(SshError::Protocol("unexpected auth response")),
    }
}

/// Authenticate with keyboard-interactive using the supplied password for prompts.
pub fn client_auth_keyboard_interactive(
    io: &mut PacketIo,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    println!(
        "bssh: userauth request method=keyboard-interactive user='{}'",
        username
    );

    let mut req = Vec::with_capacity(96);
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"keyboard-interactive");
    SshBuf::put_string(&mut req, b"");
    SshBuf::put_string(&mut req, b"");
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    for _ in 0..8 {
        let reply = recv_client_reply(io)?;
        if reply.is_empty() {
            return Err(SshError::Protocol("empty keyboard-interactive response"));
        }

        match reply[0] {
            SSH_MSG_USERAUTH_SUCCESS => return Ok(()),
            SSH_MSG_USERAUTH_FAILURE => {
                let failure = parse_auth_failure(&reply)?;
                println!(
                    "bssh: userauth failure methods='{}' partial={}",
                    failure.methods, failure.partial_success
                );
                return Err(SshError::Auth);
            }
            SSH_MSG_USERAUTH_INFO_REQUEST => {
                let prompt_count = parse_keyboard_interactive_request(&reply)?;
                println!(
                    "bssh: keyboard-interactive info-request prompts={}",
                    prompt_count
                );

                let mut response = Vec::with_capacity(16 + password.len() * prompt_count);
                response.push(SSH_MSG_USERAUTH_INFO_RESPONSE);
                SshBuf::put_u32(&mut response, prompt_count as u32);
                for _ in 0..prompt_count {
                    SshBuf::put_string(&mut response, password.as_bytes());
                }
                io.send_packet(&response).map_err(|_| SshError::Io)?;
            }
            _ => {
                return Err(SshError::Protocol(
                    "unexpected keyboard-interactive response",
                ))
            }
        }
    }

    Err(SshError::Protocol("too many keyboard-interactive rounds"))
}

/// Authenticate with the SSH publickey method using the embedded bssh key.
pub fn client_auth_publickey(
    io: &mut PacketIo,
    username: &str,
    session_id: &[u8],
    wrong_key: bool,
) -> Result<(), SshError> {
    let mut key_blob = keys::embedded_client_public_key_blob();
    if wrong_key {
        if let Some(last) = key_blob.last_mut() {
            *last ^= 0x01;
        }
    }
    client_auth_publickey_with_signer(io, username, session_id, key_blob, |data| {
        keys::sign_with_embedded_client_key(data)
    })
}

/// Authenticate with the SSH publickey method using a caller-supplied identity.
pub fn client_auth_publickey_identity(
    io: &mut PacketIo,
    username: &str,
    session_id: &[u8],
    identity: &keys::RsaIdentity,
) -> Result<(), SshError> {
    let key_blob = identity.public_key_blob();
    client_auth_publickey_with_signer(io, username, session_id, key_blob, |data| {
        identity.sign(data)
    })
}

fn client_auth_publickey_with_signer<F>(
    io: &mut PacketIo,
    username: &str,
    session_id: &[u8],
    key_blob: Vec<u8>,
    sign: F,
) -> Result<(), SshError>
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    let algo = b"rsa-sha2-256";
    println!(
        "bssh: userauth request method=publickey user='{}' algo={} has_signature=false key_blob_len={}",
        username,
        String::from_utf8_lossy(algo),
        key_blob.len()
    );

    let mut query = Vec::with_capacity(128 + key_blob.len());
    query.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut query, username.as_bytes());
    SshBuf::put_string(&mut query, b"ssh-connection");
    SshBuf::put_string(&mut query, b"publickey");
    SshBuf::put_bool(&mut query, false);
    SshBuf::put_string(&mut query, algo);
    SshBuf::put_string(&mut query, &key_blob);
    io.send_packet(&query).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty publickey query response"));
    }
    match reply[0] {
        SSH_MSG_USERAUTH_PK_OK => {}
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            return Err(SshError::Auth);
        }
        _ => return Err(SshError::Protocol("unexpected publickey query response")),
    }

    let mut pos = 1;
    let accepted_algo = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad publickey pk_ok algorithm"))?;
    let accepted_key = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad publickey pk_ok key"))?;
    if accepted_algo != algo || accepted_key != key_blob.as_slice() {
        return Err(SshError::Protocol("publickey pk_ok mismatch"));
    }

    let mut signed_data = Vec::with_capacity(128 + session_id.len() + key_blob.len());
    SshBuf::put_string(&mut signed_data, session_id);
    signed_data.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut signed_data, username.as_bytes());
    SshBuf::put_string(&mut signed_data, b"ssh-connection");
    SshBuf::put_string(&mut signed_data, b"publickey");
    SshBuf::put_bool(&mut signed_data, true);
    SshBuf::put_string(&mut signed_data, algo);
    SshBuf::put_string(&mut signed_data, &key_blob);
    let signature = sign(&signed_data);

    println!(
        "bssh: userauth request method=publickey user='{}' algo={} has_signature=true signature_len={}",
        username,
        String::from_utf8_lossy(algo),
        signature.len()
    );

    let mut req = Vec::with_capacity(128 + key_blob.len() + signature.len());
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"publickey");
    SshBuf::put_bool(&mut req, true);
    SshBuf::put_string(&mut req, algo);
    SshBuf::put_string(&mut req, &key_blob);
    SshBuf::put_string(&mut req, &signature);
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty publickey auth response"));
    }

    match reply[0] {
        SSH_MSG_USERAUTH_SUCCESS => Ok(()),
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            Err(SshError::Auth)
        }
        _ => Err(SshError::Protocol("unexpected publickey auth response")),
    }
}

struct AuthFailure {
    methods: String,
    partial_success: bool,
}

impl AuthFailure {
    fn method_allowed(&self, method: &str) -> bool {
        self.methods.split(',').any(|candidate| candidate == method)
    }
}

fn parse_auth_failure(reply: &[u8]) -> Result<AuthFailure, SshError> {
    let mut pos = 1;
    let methods = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad userauth failure methods"))?;
    let partial_success = SshBuf::get_bool(reply, &mut pos)
        .ok_or(SshError::Protocol("bad userauth failure partial flag"))?;
    Ok(AuthFailure {
        methods: String::from_utf8_lossy(methods).into_owned(),
        partial_success,
    })
}

fn parse_keyboard_interactive_request(reply: &[u8]) -> Result<usize, SshError> {
    let mut pos = 1;
    let _name = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive name"))?;
    let _instruction = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive instruction"))?;
    let _language = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive language"))?;
    let prompt_count = SshBuf::get_u32(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive prompt count"))?
        as usize;

    for _ in 0..prompt_count {
        let _prompt = SshBuf::get_string(reply, &mut pos)
            .ok_or(SshError::Protocol("bad keyboard-interactive prompt"))?;
        let _echo = SshBuf::get_bool(reply, &mut pos)
            .ok_or(SshError::Protocol("bad keyboard-interactive echo flag"))?;
    }

    Ok(prompt_count)
}

fn recv_client_reply(io: &mut PacketIo) -> Result<Vec<u8>, SshError> {
    loop {
        let reply = io.recv_packet().map_err(|_| SshError::Io)?;
        if reply.is_empty() {
            return Ok(reply);
        }
        match reply[0] {
            SSH_MSG_IGNORE | SSH_MSG_DEBUG | SSH_MSG_EXT_INFO | SSH_MSG_USERAUTH_BANNER => continue,
            _ => return Ok(reply),
        }
    }
}
