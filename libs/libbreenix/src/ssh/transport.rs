//! SSH transport layer — high-level session management
//!
//! Ties together the packet, key exchange, cipher, authentication, and channel
//! layers into a complete SSH server or client session.

use crate::crypto::rand::Csprng;
use crate::errno::Errno;
use crate::error::Error;
use crate::types::Fd;

use super::auth;
use super::channel::{self, Channel};
use super::kex::{self, KexState};
use super::keys::{HostKey, Identity, ServerHostKeyInfo};
use super::packet::PacketIo;
use super::{SshBuf, SshError, BSSH_VERSION};
use super::{
    SSH_MSG_CHANNEL_CLOSE, SSH_MSG_CHANNEL_DATA, SSH_MSG_CHANNEL_EXTENDED_DATA, SSH_MSG_CHANNEL_EOF, SSH_MSG_CHANNEL_OPEN,
    SSH_MSG_CHANNEL_REQUEST, SSH_MSG_CHANNEL_SUCCESS, SSH_MSG_CHANNEL_WINDOW_ADJUST,
    SSH_MSG_DISCONNECT, SSH_MSG_GLOBAL_REQUEST, SSH_MSG_IGNORE, SSH_MSG_KEXINIT, SSH_MSG_NEWKEYS,
    SSH_MSG_UNIMPLEMENTED,
};

/// An SSH server session on an accepted TCP connection.
pub struct ServerSession<'a> {
    io: PacketIo,
    host_key: &'a HostKey,
    authorizer: &'a mut dyn auth::Authorizer,
    kex: KexState,
    client_version: String,
    channel: Option<Channel>,
    username: String,
}

impl<'a> ServerSession<'a> {
    /// A server session on an accepted TCP connection, signing with
    /// `host_key` and deciding publickey logins with `authorizer`.
    pub fn new(fd: Fd, host_key: &'a HostKey, authorizer: &'a mut dyn auth::Authorizer) -> Self {
        Self {
            io: PacketIo::new(fd),
            host_key,
            authorizer,
            kex: KexState::new(),
            client_version: String::new(),
            channel: None,
            username: String::new(),
        }
    }

    /// Perform the full SSH handshake: version exchange, key exchange,
    /// authentication, and channel setup.
    ///
    /// Returns the authenticated username on success.
    pub fn handshake(&mut self) -> Result<String, SshError> {
        // 1. Version exchange
        // RFC 4253 section 4.2: send identification once TCP is established.
        // sendmsg waits for the accepted connection to leave SynReceived.
        self.io.write_line(BSSH_VERSION).map_err(|_| SshError::Io)?;
        self.client_version = match self.io.read_line() {
            Ok(v) => v,
            Err(e) => {
                let err = ssh_io_error(e);
                println!("bsshd: read_line failed: {:?}", err);
                return Err(err);
            }
        };
        println!("bsshd: client version: '{}'", self.client_version.escape_debug());

        if !self.client_version.starts_with("SSH-2.0-") {
            return Err(SshError::Protocol("unsupported SSH version"));
        }

        self.handshake_after_version()
    }

    /// Continue the SSH handshake after the version exchange is complete.
    ///
    /// Expects `self.client_version` to be set already.
    pub fn handshake_after_version(&mut self) -> Result<String, SshError> {
        if !self.client_version.starts_with("SSH-2.0-") {
            return Err(SshError::Protocol("unsupported SSH version"));
        }

        // 2. Key exchange
        let mut rng = Csprng::new();
        let my_kexinit = kex::build_kexinit(&mut rng);
        self.kex.my_kexinit = my_kexinit.clone();
        self.io.send_packet(&my_kexinit).map_err(|_| SshError::Io)?;

        // Receive client's KEXINIT
        let peer_kexinit = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if peer_kexinit.is_empty() || peer_kexinit[0] != SSH_MSG_KEXINIT {
            return Err(SshError::Protocol("expected KEXINIT"));
        }
        self.kex.peer_kexinit = peer_kexinit;

        // Receive KEX init (type 30)
        let kex_init = self.io.recv_packet().map_err(|_| SshError::Io)?;

        // Dispatch: hybrid C_INIT is 1216 bytes, X25519 Q_C is 32 bytes
        let c_init_len = if kex_init.len() > 5 {
            u32::from_be_bytes([kex_init[1], kex_init[2], kex_init[3], kex_init[4]]) as usize
        } else {
            0
        };

        let (exchange_hash, shared_secret) = if c_init_len == 1216 {
            println!("bsshd: KEX mlkem768x25519-sha256 (post-quantum)");
            kex::server_kex_hybrid(
                &mut self.io,
                self.host_key,
                &mut self.kex,
                &self.client_version,
                BSSH_VERSION,
                &kex_init,
            )?
        } else {
            println!("bsshd: KEX curve25519-sha256");
            kex::server_kex_ecdh(
                &mut self.io,
                self.host_key,
                &mut self.kex,
                &self.client_version,
                BSSH_VERSION,
                &kex_init,
            )?
        };

        // Receive client's NEWKEYS
        let newkeys = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if newkeys.is_empty() || newkeys[0] != SSH_MSG_NEWKEYS {
            return Err(SshError::Protocol("expected NEWKEYS"));
        }

        // Derive and install session keys
        let session_id = self.kex.session_id.as_ref().unwrap();
        let (c2s, s2c) = kex::negotiate_ciphers(&self.kex.peer_kexinit, &self.kex.my_kexinit)?;
        let (cipher_c2s, cipher_s2c) = kex::derive_keys(
            &shared_secret, &exchange_hash, session_id, c_init_len == 1216, c2s, s2c,
        );

        self.io.set_cipher_recv(cipher_c2s); // client→server = our recv
        self.io.set_cipher_send(cipher_s2c); // server→client = our send

        // Send EXT_INFO (RFC 8308) to advertise supported pubkey algorithms.
        // Without this, modern OpenSSH (which disables ssh-rsa SHA-1 by default)
        // won't offer RSA keys for publickey auth — it needs to know we support
        // rsa-sha2-256 for signature verification. Ed25519 keys need no SHA-1.
        {
            let mut ext_info = Vec::with_capacity(64);
            ext_info.push(7); // SSH_MSG_EXT_INFO
            SshBuf::put_u32(&mut ext_info, 1); // nr-extensions = 1
            SshBuf::put_string(&mut ext_info, b"server-sig-algs");
            SshBuf::put_string(&mut ext_info, b"ssh-ed25519,rsa-sha2-256,rsa-sha2-512");
            self.io.send_packet(&ext_info).map_err(|_| SshError::Io)?;
        }

        // 3. Authentication
        auth::server_accept_service(&mut self.io)?;
        self.username = auth::server_authenticate(&mut self.io, session_id, self.authorizer)?;

        Ok(self.username.clone())
    }

    /// Wait for the client to open a channel and request a shell.
    ///
    /// Handles CHANNEL_OPEN and CHANNEL_REQUEST messages until a shell
    /// or exec request is received. Returns true if a PTY was requested.
    pub fn wait_for_channel(&mut self) -> Result<bool, SshError> {
        let mut pty_requested = false;
        let mut shell_requested = false;

        while !shell_requested {
            let msg = self.io.recv_packet().map_err(|_| SshError::Io)?;
            if msg.is_empty() {
                return Err(SshError::Disconnected);
            }

            match msg[0] {
                SSH_MSG_CHANNEL_OPEN => {
                    let ch = channel::server_handle_channel_open(&mut self.io, &msg, 0)?;
                    self.channel = Some(ch);
                }
                SSH_MSG_CHANNEL_REQUEST => {
                    if let Some(ref mut ch) = self.channel {
                        let req_type =
                            channel::server_handle_channel_request(&mut self.io, &msg, ch)?;
                        match req_type.as_str() {
                            "pty-req" => pty_requested = true,
                            "shell" | "exec" => shell_requested = true,
                            _ => {}
                        }
                    }
                }
                SSH_MSG_GLOBAL_REQUEST => {
                    // Respond with REQUEST_FAILURE for unrecognized global requests
                    let mut pos = 1;
                    let _req_name = SshBuf::get_string(&msg, &mut pos);
                    let want_reply = SshBuf::get_bool(&msg, &mut pos).unwrap_or(false);
                    if want_reply {
                        self.io
                            .send_packet(&[super::SSH_MSG_REQUEST_FAILURE])
                            .map_err(|_| SshError::Io)?;
                    }
                }
                SSH_MSG_IGNORE | SSH_MSG_UNIMPLEMENTED => {}
                SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                    if let Some(ref mut ch) = self.channel {
                        let mut pos = 1;
                        let _id = SshBuf::get_u32(&msg, &mut pos);
                        let bytes = SshBuf::get_u32(&msg, &mut pos).unwrap_or(0);
                        ch.send_window = ch.send_window.saturating_add(bytes);
                    }
                }
                SSH_MSG_DISCONNECT => return Err(SshError::Disconnected),
                _ => {}
            }
        }

        Ok(pty_requested)
    }

    /// The terminal type the client's pty-req named, if any.
    pub fn pty_term(&self) -> Option<String> {
        self.channel.as_ref().and_then(|ch| ch.pty_term.clone())
    }

    /// Get the channel's PTY dimensions.
    pub fn pty_size(&self) -> (u32, u32) {
        self.channel
            .as_ref()
            .map(|ch| ch.pty_size)
            .unwrap_or((80, 24))
    }

    /// Send data on the channel to the client.
    pub fn send_data(&mut self, data: &[u8]) -> Result<(), SshError> {
        if let Some(ref mut ch) = self.channel {
            channel::send_channel_data(&mut self.io, ch, data)
        } else {
            Err(SshError::ChannelNotFound)
        }
    }

    /// Send the command's stderr as RFC 4254 extended data type 1.
    pub fn send_stderr(&mut self, data: &[u8]) -> Result<(), SshError> {
        if let Some(ref mut ch) = self.channel {
            channel::send_channel_stderr(&mut self.io, ch, data)
        } else {
            Err(SshError::ChannelNotFound)
        }
    }

    /// Bytes that can be sent without waiting for peer window credit.
    pub fn send_capacity(&self) -> usize {
        self.channel.as_ref().map(|ch| ch.send_window.min(ch.max_packet) as usize).unwrap_or(0)
    }

    /// Replenish input credit only after the consumer has delivered or discarded it.
    pub fn consume_input(&mut self, bytes: usize) -> Result<(), SshError> {
        let ch = self.channel.as_mut().ok_or(SshError::ChannelNotFound)?;
        channel::send_window_adjust(&mut self.io, ch, bytes as u32)
    }

    /// Take the command supplied by an SSH "exec" request, if any.
    pub fn take_exec_command(&mut self) -> Option<String> {
        self.channel
            .as_mut()
            .and_then(|channel| channel.exec_command.take())
    }

    /// Report an exec command's exit status to the SSH client.
    pub fn send_exit_status(&mut self, status: i32) -> Result<(), SshError> {
        if let Some(ref ch) = self.channel {
            let mut msg = Vec::with_capacity(24);
            msg.push(SSH_MSG_CHANNEL_REQUEST);
            SshBuf::put_u32(&mut msg, ch.remote_id);
            SshBuf::put_string(&mut msg, b"exit-status");
            SshBuf::put_bool(&mut msg, false);
            SshBuf::put_u32(&mut msg, status.max(0) as u32);
            self.io.send_packet(&msg).map_err(|_| SshError::Io)
        } else {
            Err(SshError::ChannelNotFound)
        }
    }

    /// Receive a message from the client.
    ///
    /// Returns Some(data) for channel data, None for non-data messages
    /// (window adjust, etc.), or Err for disconnect/errors.
    pub fn recv_data(&mut self) -> Result<Option<Vec<u8>>, SshError> {
        let msg = self.io.recv_packet().map_err(|_| SshError::Io)?;
        self.handle_channel_packet(&msg, true)
    }

    /// Nonblocking receive for a process relay. A fragmented packet stays in
    /// PacketIo; an incomplete packet or control message yields no channel data. The caller
    /// returns input credit with consume_input after delivering the data.
    pub fn try_recv_data_uncredited(&mut self) -> Result<Option<Vec<u8>>, SshError> {
        let Some(msg) = self.io.try_recv_packet().map_err(|_| SshError::Io)? else { return Ok(None) };
        self.handle_channel_packet(&msg, false)
    }

    fn handle_channel_packet(&mut self, msg: &[u8], replenish: bool) -> Result<Option<Vec<u8>>, SshError> {
        if msg.is_empty() {
            return Err(SshError::Disconnected);
        }

        match msg[0] {
            SSH_MSG_CHANNEL_DATA => {
                let mut pos = 1;
                let _channel_id = SshBuf::get_u32(msg, &mut pos);
                let data = SshBuf::get_string(msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel data"))?;

                // Adjust receive window
                if let Some(ref mut ch) = self.channel {
                    ch.recv_window = ch.recv_window.checked_sub(data.len() as u32)
                        .ok_or(SshError::Protocol("channel receive window exceeded"))?;
                    if replenish && ch.recv_window < 512 * 1024 {
                        channel::send_window_adjust(&mut self.io, ch, 1024 * 1024)?;
                    }
                }

                Ok(Some(data.to_vec()))
            }
            SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                if let Some(ref mut ch) = self.channel {
                    let mut pos = 1;
                    let _id = SshBuf::get_u32(msg, &mut pos);
                    let bytes = SshBuf::get_u32(msg, &mut pos).unwrap_or(0);
                    ch.send_window = ch.send_window.saturating_add(bytes);
                }
                Ok(None)
            }
            SSH_MSG_CHANNEL_EOF => {
                if let Some(ref mut ch) = self.channel {
                    ch.eof_received = true;
                }
                // EOF closes only the peer's sending direction.
                Ok(None)
            }
            SSH_MSG_CHANNEL_CLOSE => {
                let mut pos = 1;
                let recipient = SshBuf::get_u32(msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel close"))?;
                if pos != msg.len() {
                    return Err(SshError::Protocol("bad channel close length"));
                }
                let ch = self.channel.as_mut().ok_or(SshError::ChannelNotFound)?;
                if recipient != ch.local_id {
                    return Err(SshError::ChannelNotFound);
                }
                ch.closed = true;
                channel::send_channel_close(&mut self.io, ch)?;
                Err(SshError::Disconnected)
            }
            SSH_MSG_DISCONNECT => Err(SshError::Disconnected),
            SSH_MSG_IGNORE | SSH_MSG_UNIMPLEMENTED => Ok(None),
            SSH_MSG_CHANNEL_REQUEST => {
                // Handle window-change during session
                if let Some(ref mut ch) = self.channel {
                    let _ = channel::server_handle_channel_request(&mut self.io, &msg, ch);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Whether the peer has finished sending channel input.
    pub fn input_eof(&self) -> bool {
        self.channel.as_ref().is_some_and(|ch| ch.eof_received)
    }

    /// Finish a process channel in RFC 4254 sections 6.10/5.3 order.
    /// A peer-initiated CLOSE is acknowledged once; our CLOSE awaits theirs.
    pub fn finish(&mut self, status: i32) -> Result<(), SshError> {
        if self
            .channel
            .as_ref()
            .ok_or(SshError::ChannelNotFound)?
            .closed
        {
            return Ok(());
        }
        self.send_exit_status(status)?;
        let ch = self.channel.as_mut().ok_or(SshError::ChannelNotFound)?;
        channel::send_channel_eof(&mut self.io, ch)?;
        channel::send_channel_close(&mut self.io, ch)?;
        let deadline = crate::time::now_monotonic()
            .map_err(|_| SshError::Io)?
            .tv_sec
            + 5;
        while !self.channel.as_ref().unwrap().closed {
            if crate::time::now_monotonic()
                .map_err(|_| SshError::Io)?
                .tv_sec
                >= deadline
            {
                return Err(SshError::Protocol("channel close timeout"));
            }
            let mut fds = [crate::io::PollFd::new(
                self.io.fd(),
                crate::io::poll_events::POLLIN,
            )];
            if crate::io::poll(&mut fds, 100).map_err(|_| SshError::Io)? == 0 {
                continue;
            }
            match self.recv_data() {
                Err(SshError::Disconnected) if self.channel.as_ref().unwrap().closed => break,
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        Ok(())
    }

    /// Send channel EOF and close.
    pub fn close(&mut self) {
        if let Some(ref mut ch) = self.channel {
            let _ = channel::send_channel_eof(&mut self.io, ch);
            let _ = channel::send_channel_close(&mut self.io, ch);
        }
    }

    /// Get the underlying packet I/O (for advanced use).
    pub fn io(&mut self) -> &mut PacketIo {
        &mut self.io
    }
}

fn ssh_io_error(err: Error) -> SshError {
    match err {
        Error::Os(Errno::ECONNRESET | Errno::EPIPE) => SshError::Disconnected,
        _ => SshError::Io,
    }
}

/// An SSH client session.
pub struct ClientSession {
    io: PacketIo,
    kex: KexState,
    server_version: String,
    channel: Option<Channel>,
    exit_status: Option<i32>,
    data_is_stderr: bool,
}

/// Authentication method for SSH client handshakes.
pub enum ClientAuthMethod<'a> {
    Password(&'a str),
    /// Publickey with `identity`; `wrong_signer` signs with a fresh random key instead.
    PublicKey { identity: &'a Identity, wrong_signer: bool },
}

impl ClientSession {
    /// Create a new client session from a connected TCP socket fd.
    pub fn new(fd: Fd) -> Self {
        Self {
            io: PacketIo::new(fd),
            kex: KexState::new(),
            server_version: String::new(),
            channel: None,
            exit_status: None,
            data_is_stderr: false,
        }
    }

    /// Perform the full SSH handshake and authentication.
    pub fn handshake(&mut self, username: &str, password: &str) -> Result<(), SshError> {
        self.handshake_with_auth(username, ClientAuthMethod::Password(password))
    }

    /// Perform the full SSH handshake with a selected authentication method.
    pub fn handshake_with_auth(
        &mut self,
        username: &str,
        auth_method: ClientAuthMethod<'_>,
    ) -> Result<(), SshError> {
        self.handshake_with_auth_and_host_key(username, auth_method, |_| Ok(()))
    }

    /// Perform the full SSH handshake with caller-supplied host-key verification.
    pub fn handshake_with_auth_and_host_key<F>(
        &mut self,
        username: &str,
        auth_method: ClientAuthMethod<'_>,
        verify_host_key: F,
    ) -> Result<(), SshError>
    where
        F: FnOnce(&ServerHostKeyInfo) -> Result<(), SshError>,
    {
        // 1. Version exchange
        self.io.write_line(BSSH_VERSION).map_err(|_| SshError::Io)?;
        self.server_version = self.io.read_line().map_err(|_| SshError::Io)?;

        if !self.server_version.starts_with("SSH-2.0-") {
            return Err(SshError::Protocol("unsupported SSH version"));
        }

        // 2. Key exchange
        let mut rng = Csprng::new();
        let my_kexinit = kex::build_client_kexinit(&mut rng);
        self.kex.my_kexinit = my_kexinit.clone();
        self.io.send_packet(&my_kexinit).map_err(|_| SshError::Io)?;

        // Receive server's KEXINIT
        let peer_kexinit = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if peer_kexinit.is_empty() || peer_kexinit[0] != SSH_MSG_KEXINIT {
            return Err(SshError::Protocol("expected KEXINIT"));
        }
        self.kex.peer_kexinit = peer_kexinit;

        // Perform client-side DH
        let (exchange_hash, shared_secret, host_key) = kex::client_kex_ecdh(
            &mut self.io,
            &mut self.kex,
            BSSH_VERSION,
            &self.server_version,
        )?;
        verify_host_key(&host_key)?;

        // Receive server's NEWKEYS
        let newkeys = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if newkeys.is_empty() || newkeys[0] != SSH_MSG_NEWKEYS {
            return Err(SshError::Protocol("expected NEWKEYS"));
        }

        // Send our NEWKEYS
        self.io
            .send_packet(&[SSH_MSG_NEWKEYS])
            .map_err(|_| SshError::Io)?;

        // Install cipher
        let session_id = self.kex.session_id.as_ref().unwrap();
        let (send_cipher, recv_cipher) = kex::negotiate_ciphers(&self.kex.my_kexinit, &self.kex.peer_kexinit)?;
        let (cipher_c2s, cipher_s2c) = kex::derive_keys(
            &shared_secret, &exchange_hash, session_id, false, send_cipher, recv_cipher,
        );

        self.io.set_cipher_send(cipher_c2s); // client→server = our send
        self.io.set_cipher_recv(cipher_s2c); // server→client = our recv

        // 3. Authentication
        auth::client_request_service(&mut self.io)?;
        match auth_method {
            ClientAuthMethod::Password(password) => {
                auth::client_auth_password(&mut self.io, username, password)?;
            }
            ClientAuthMethod::PublicKey { identity, wrong_signer } => {
                auth::client_auth_publickey(&mut self.io, username, session_id, identity, wrong_signer)?;
            }
        }

        Ok(())
    }

    /// Open a session channel, request a PTY, and start a shell.
    pub fn open_shell(&mut self) -> Result<(), SshError> {
        let ch = channel::client_open_session(&mut self.io)?;
        self.channel = Some(ch);

        let ch = self.channel.as_ref().unwrap();
        channel::client_request_pty(&mut self.io, ch, "xterm-256color", 80, 24)?;

        let ch = self.channel.as_ref().unwrap();
        channel::client_request_shell(&mut self.io, ch)?;

        Ok(())
    }

    /// Open a session channel and execute a command.
    pub fn open_exec(&mut self, command: &str) -> Result<(), SshError> {
        let ch = channel::client_open_session(&mut self.io)?;
        self.channel = Some(ch);

        let ch = self.channel.as_ref().unwrap();
        channel::client_request_exec(&mut self.io, ch, command)?;

        Ok(())
    }

    /// Send data on the channel.
    pub fn send_data(&mut self, data: &[u8]) -> Result<(), SshError> {
        if let Some(ref mut ch) = self.channel {
            channel::send_channel_data(&mut self.io, ch, data)
        } else {
            Err(SshError::ChannelNotFound)
        }
    }

    /// Receive data from the channel.
    pub fn recv_data(&mut self) -> Result<Option<Vec<u8>>, SshError> {
        let msg = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if msg.is_empty() {
            return Err(SshError::Disconnected);
        }

        match msg[0] {
            SSH_MSG_CHANNEL_DATA | SSH_MSG_CHANNEL_EXTENDED_DATA => {
                let mut pos = 1;
                let _channel_id = SshBuf::get_u32(&msg, &mut pos);
                self.data_is_stderr = msg[0] == SSH_MSG_CHANNEL_EXTENDED_DATA;
                if self.data_is_stderr && SshBuf::get_u32(&msg, &mut pos) != Some(1) {
                    return Err(SshError::Protocol("unsupported extended channel data"));
                }
                let data = SshBuf::get_string(&msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel data"))?;

                if let Some(ref mut ch) = self.channel {
                    ch.recv_window = ch.recv_window.checked_sub(data.len() as u32)
                        .ok_or(SshError::Protocol("channel receive window exceeded"))?;
                    if ch.recv_window < 512 * 1024 {
                        channel::send_window_adjust(&mut self.io, ch, 1024 * 1024)?;
                    }
                }

                Ok(Some(data.to_vec()))
            }
            SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                if let Some(ref mut ch) = self.channel {
                    let mut pos = 1;
                    let _id = SshBuf::get_u32(&msg, &mut pos);
                    let bytes = SshBuf::get_u32(&msg, &mut pos).unwrap_or(0);
                    ch.send_window = ch.send_window.saturating_add(bytes);
                }
                Ok(None)
            }
            SSH_MSG_CHANNEL_EOF => {
                if let Some(ref mut ch) = self.channel {
                    ch.eof_received = true;
                }
                Ok(None)
            }
            SSH_MSG_CHANNEL_CLOSE => {
                if let Some(ref mut ch) = self.channel {
                    ch.closed = true;
                    channel::send_channel_close(&mut self.io, ch)?;
                }
                Err(SshError::Disconnected)
            }
            SSH_MSG_DISCONNECT => Err(SshError::Disconnected),
            SSH_MSG_IGNORE | SSH_MSG_UNIMPLEMENTED => Ok(None),
            SSH_MSG_CHANNEL_REQUEST => {
                let mut pos = 1;
                let _recipient = SshBuf::get_u32(&msg, &mut pos);
                let request_type = SshBuf::get_string(&msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel request type"))?;
                let want_reply = SshBuf::get_bool(&msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel request want_reply"))?;

                if request_type == b"exit-status" {
                    let status = SshBuf::get_u32(&msg, &mut pos)
                        .ok_or(SshError::Protocol("bad exit-status"))?;
                    self.exit_status = Some(status as i32);
                }

                if want_reply {
                    if let Some(ref ch) = self.channel {
                        let mut reply = Vec::with_capacity(5);
                        reply.push(SSH_MSG_CHANNEL_SUCCESS);
                        SshBuf::put_u32(&mut reply, ch.remote_id);
                        self.io.send_packet(&reply).map_err(|_| SshError::Io)?;
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Whether the most recently returned data is RFC 4254 stderr (type 1).
    pub fn data_is_stderr(&self) -> bool {
        self.data_is_stderr
    }

    /// Last exit status reported by the remote exec channel.
    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Close the session.
    pub fn close(&mut self) {
        if let Some(ref mut ch) = self.channel {
            let _ = channel::send_channel_eof(&mut self.io, ch);
            let _ = channel::send_channel_close(&mut self.io, ch);
        }
    }

    /// Get the underlying packet I/O (for advanced use).
    pub fn io(&mut self) -> &mut PacketIo {
        &mut self.io
    }
}
