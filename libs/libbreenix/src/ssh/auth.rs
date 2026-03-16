//! SSH authentication layer (RFC 4252)
//!
//! Implements public key and password authentication for the SSH user
//! authentication protocol. Public key auth is tried first; password
//! auth is the fallback.

use super::keys;
use super::packet::PacketIo;
use super::{SshBuf, SshError};
use super::{SSH_MSG_SERVICE_ACCEPT, SSH_MSG_SERVICE_REQUEST};
use super::{SSH_MSG_USERAUTH_FAILURE, SSH_MSG_USERAUTH_REQUEST, SSH_MSG_USERAUTH_SUCCESS};

/// SSH_MSG_USERAUTH_PK_OK (RFC 4252 §7)
const SSH_MSG_USERAUTH_PK_OK: u8 = 60;

/// Handle the "ssh-userauth" service request (server side).
pub fn server_accept_service(io: &mut PacketIo) -> Result<(), SshError> {
    let msg = io.recv_packet().map_err(|_| SshError::Io)?;
    if msg.is_empty() || msg[0] != SSH_MSG_SERVICE_REQUEST {
        return Err(SshError::Protocol("expected SERVICE_REQUEST"));
    }

    let mut pos = 1;
    let service = SshBuf::get_string(&msg, &mut pos)
        .ok_or(SshError::Protocol("bad SERVICE_REQUEST"))?;

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
pub fn server_authenticate(
    io: &mut PacketIo,
    session_id: &[u8],
) -> Result<String, SshError> {
    loop {
        let msg = io.recv_packet().map_err(|_| SshError::Io)?;
        if msg.is_empty() {
            return Err(SshError::Protocol("empty auth message"));
        }

        if msg[0] != SSH_MSG_USERAUTH_REQUEST {
            continue;
        }

        let mut pos = 1;
        let username = SshBuf::get_string(&msg, &mut pos)
            .ok_or(SshError::Protocol("bad username"))?;
        let service = SshBuf::get_string(&msg, &mut pos)
            .ok_or(SshError::Protocol("bad service"))?;
        let method = SshBuf::get_string(&msg, &mut pos)
            .ok_or(SshError::Protocol("bad method"))?;

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

            println!("bsshd: pubkey auth: algo={} blob_len={} has_sig={}",
                String::from_utf8_lossy(algo), key_blob.len(), has_signature);

            // Check if this key is in our authorized_keys
            if !keys::is_authorized_key(key_blob) {
                println!("bsshd: key NOT in authorized_keys (expected {} bytes)",
                    keys::authorized_key_blob_len());
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

    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
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
    let mut req = Vec::with_capacity(64);
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"password");
    SshBuf::put_bool(&mut req, false);
    SshBuf::put_string(&mut req, password.as_bytes());
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty auth response"));
    }

    match reply[0] {
        SSH_MSG_USERAUTH_SUCCESS => Ok(()),
        SSH_MSG_USERAUTH_FAILURE => Err(SshError::Auth),
        _ => Err(SshError::Protocol("unexpected auth response")),
    }
}
