//! SSH authentication layer (RFC 4252)
//!
//! Implements password authentication for the SSH user authentication protocol.

use super::packet::PacketIo;
use super::{SshBuf, SshError};
use super::{SSH_MSG_SERVICE_ACCEPT, SSH_MSG_SERVICE_REQUEST};
use super::{SSH_MSG_USERAUTH_FAILURE, SSH_MSG_USERAUTH_REQUEST, SSH_MSG_USERAUTH_SUCCESS};

/// Handle the "ssh-userauth" service request (server side).
///
/// Waits for the client to request the ssh-userauth service, then accepts it.
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

    // Send SERVICE_ACCEPT
    let mut reply = Vec::with_capacity(20);
    reply.push(SSH_MSG_SERVICE_ACCEPT);
    SshBuf::put_string(&mut reply, b"ssh-userauth");
    io.send_packet(&reply).map_err(|_| SshError::Io)?;

    Ok(())
}

/// Handle password authentication (server side).
///
/// Reads the client's USERAUTH_REQUEST and validates the password.
/// Returns the username on success.
///
/// # Authentication Policy
/// Accepts any username with password "breenix" (development default).
pub fn server_authenticate(io: &mut PacketIo) -> Result<String, SshError> {
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

        if method == b"password" {
            // Read password: boolean change_password, string password
            let _change = SshBuf::get_bool(&msg, &mut pos);
            let password = SshBuf::get_string(&msg, &mut pos)
                .ok_or(SshError::Protocol("bad password field"))?;

            if password == b"breenix" {
                // Send USERAUTH_SUCCESS
                io.send_packet(&[SSH_MSG_USERAUTH_SUCCESS])
                    .map_err(|_| SshError::Io)?;
                return Ok(username_str);
            }
        } else if method == b"none" {
            // "none" method is used to query supported methods
        }

        // Send USERAUTH_FAILURE with supported methods
        let mut failure = Vec::with_capacity(32);
        failure.push(SSH_MSG_USERAUTH_FAILURE);
        SshBuf::put_string(&mut failure, b"password");
        SshBuf::put_bool(&mut failure, false); // partial success
        io.send_packet(&failure).map_err(|_| SshError::Io)?;
    }
}

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
    SshBuf::put_bool(&mut req, false); // not changing password
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
