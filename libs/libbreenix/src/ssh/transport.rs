//! SSH transport layer — high-level session management
//!
//! Ties together the packet, key exchange, cipher, authentication, and channel
//! layers into a complete SSH server or client session.

use crate::crypto::rand::Csprng;
use crate::types::Fd;

use super::auth;
use super::channel::{self, Channel};
use super::kex::{self, KexState};
use super::keys::HostKey;
use super::packet::PacketIo;
use super::{SshBuf, SshError, BSSH_VERSION};
use super::{
    SSH_MSG_CHANNEL_CLOSE, SSH_MSG_CHANNEL_DATA, SSH_MSG_CHANNEL_EOF, SSH_MSG_CHANNEL_OPEN,
    SSH_MSG_CHANNEL_REQUEST, SSH_MSG_CHANNEL_WINDOW_ADJUST, SSH_MSG_DISCONNECT,
    SSH_MSG_GLOBAL_REQUEST, SSH_MSG_IGNORE, SSH_MSG_KEXINIT, SSH_MSG_NEWKEYS,
    SSH_MSG_UNIMPLEMENTED,
};

/// An SSH server session on an accepted TCP connection.
pub struct ServerSession {
    io: PacketIo,
    host_key: HostKey,
    kex: KexState,
    client_version: String,
    channel: Option<Channel>,
    username: String,
}

impl ServerSession {
    /// Create a new server session from an accepted TCP connection fd.
    pub fn new(fd: Fd) -> Self {
        Self {
            io: PacketIo::new(fd),
            host_key: HostKey::load(),
            kex: KexState::new(),
            client_version: String::new(),
            channel: None,
            username: String::new(),
        }
    }

    /// Create a server session with the version exchange already done.
    ///
    /// Used when bsshd handles the version exchange manually (e.g. for
    /// diagnostics) before creating the session.
    pub fn new_after_version(fd: Fd, client_version: &str) -> Self {
        Self {
            io: PacketIo::new(fd),
            host_key: HostKey::load(),
            kex: KexState::new(),
            client_version: client_version.to_string(),
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
        // Read client version FIRST — this blocks until the TCP 3-way handshake
        // completes (the client's ACK + version string arrive together). Sending
        // our version first would fail because the connection may still be in
        // SynReceived state after accept().
        self.client_version = match self.io.read_line() {
            Ok(v) => v,
            Err(e) => {
                // Use println since this is userspace
                println!("bsshd: read_line FAILED: {:?}", e);
                return Err(SshError::Io);
            }
        };
        println!("bsshd: client version: '{}'", self.client_version);
        if let Err(e) = self.io.write_line(BSSH_VERSION) {
            println!("bsshd: write_line FAILED: {:?}", e);
            return Err(SshError::Io);
        }
        println!("bsshd: server version sent");

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
        self.io
            .send_packet(&my_kexinit)
            .map_err(|_| SshError::Io)?;

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
        } else { 0 };

        let (exchange_hash, shared_secret) = if c_init_len == 1216 {
            println!("bsshd: KEX mlkem768x25519-sha256 (post-quantum)");
            kex::server_kex_hybrid(
                &mut self.io, &self.host_key, &mut self.kex,
                &self.client_version, BSSH_VERSION, &kex_init,
            )?
        } else {
            println!("bsshd: KEX curve25519-sha256");
            kex::server_kex_ecdh(
                &mut self.io, &self.host_key, &mut self.kex,
                &self.client_version, BSSH_VERSION, &kex_init,
            )?
        };

        // Receive client's NEWKEYS
        let newkeys = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if newkeys.is_empty() || newkeys[0] != SSH_MSG_NEWKEYS {
            return Err(SshError::Protocol("expected NEWKEYS"));
        }

        // Derive and install session keys
        let session_id = self.kex.session_id.as_ref().unwrap();
        let is_hybrid = c_init_len == 1216;
        let (cipher_c2s, cipher_s2c) = if is_hybrid {
            kex::derive_keys_hybrid(&shared_secret, &exchange_hash, session_id)
        } else {
            kex::derive_keys(&shared_secret, &exchange_hash, session_id)
        };

        self.io.set_cipher_recv(cipher_c2s); // client→server = our recv
        self.io.set_cipher_send(cipher_s2c); // server→client = our send

        // Send EXT_INFO (RFC 8308) to advertise supported pubkey algorithms.
        // Without this, modern OpenSSH (which disables ssh-rsa SHA-1 by default)
        // won't offer RSA keys for publickey auth — it needs to know we support
        // rsa-sha2-256 for signature verification.
        {
            let mut ext_info = Vec::with_capacity(64);
            ext_info.push(7); // SSH_MSG_EXT_INFO
            SshBuf::put_u32(&mut ext_info, 1); // nr-extensions = 1
            SshBuf::put_string(&mut ext_info, b"server-sig-algs");
            SshBuf::put_string(&mut ext_info, b"rsa-sha2-256,rsa-sha2-512,ssh-rsa");
            self.io.send_packet(&ext_info).map_err(|_| SshError::Io)?;
        }

        // 3. Authentication
        auth::server_accept_service(&mut self.io)?;
        self.username = auth::server_authenticate(&mut self.io, session_id)?;

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

    /// Receive a message from the client.
    ///
    /// Returns Some(data) for channel data, None for non-data messages
    /// (window adjust, etc.), or Err for disconnect/errors.
    pub fn recv_data(&mut self) -> Result<Option<Vec<u8>>, SshError> {
        let msg = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if msg.is_empty() {
            return Err(SshError::Disconnected);
        }

        match msg[0] {
            SSH_MSG_CHANNEL_DATA => {
                let mut pos = 1;
                let _channel_id = SshBuf::get_u32(&msg, &mut pos);
                let data = SshBuf::get_string(&msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel data"))?;

                // Adjust receive window
                if let Some(ref mut ch) = self.channel {
                    ch.recv_window = ch.recv_window.saturating_sub(data.len() as u32);
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
            SSH_MSG_CHANNEL_EOF | SSH_MSG_CHANNEL_CLOSE => {
                if msg[0] == SSH_MSG_CHANNEL_CLOSE {
                    // Send close back
                    if let Some(ref ch) = self.channel {
                        let _ = channel::send_channel_close(&mut self.io, ch);
                    }
                }
                Err(SshError::Disconnected)
            }
            SSH_MSG_DISCONNECT => Err(SshError::Disconnected),
            SSH_MSG_IGNORE | SSH_MSG_UNIMPLEMENTED => Ok(None),
            SSH_MSG_CHANNEL_REQUEST => {
                // Handle window-change during session
                if let Some(ref mut ch) = self.channel {
                    let _ =
                        channel::server_handle_channel_request(&mut self.io, &msg, ch);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Send channel EOF and close.
    pub fn close(&mut self) {
        if let Some(ref ch) = self.channel {
            let _ = channel::send_channel_eof(&mut self.io, ch);
            let _ = channel::send_channel_close(&mut self.io, ch);
        }
    }

    /// Get the underlying packet I/O (for advanced use).
    pub fn io(&mut self) -> &mut PacketIo {
        &mut self.io
    }
}

/// An SSH client session.
pub struct ClientSession {
    io: PacketIo,
    kex: KexState,
    server_version: String,
    channel: Option<Channel>,
}

impl ClientSession {
    /// Create a new client session from a connected TCP socket fd.
    pub fn new(fd: Fd) -> Self {
        Self {
            io: PacketIo::new(fd),
            kex: KexState::new(),
            server_version: String::new(),
            channel: None,
        }
    }

    /// Perform the full SSH handshake and authentication.
    pub fn handshake(&mut self, username: &str, password: &str) -> Result<(), SshError> {
        // 1. Version exchange
        self.io
            .write_line(BSSH_VERSION)
            .map_err(|_| SshError::Io)?;
        self.server_version = self.io.read_line().map_err(|_| SshError::Io)?;

        if !self.server_version.starts_with("SSH-2.0-") {
            return Err(SshError::Protocol("unsupported SSH version"));
        }

        // 2. Key exchange
        let mut rng = Csprng::new();
        let my_kexinit = kex::build_kexinit(&mut rng);
        self.kex.my_kexinit = my_kexinit.clone();
        self.io
            .send_packet(&my_kexinit)
            .map_err(|_| SshError::Io)?;

        // Receive server's KEXINIT
        let peer_kexinit = self.io.recv_packet().map_err(|_| SshError::Io)?;
        if peer_kexinit.is_empty() || peer_kexinit[0] != SSH_MSG_KEXINIT {
            return Err(SshError::Protocol("expected KEXINIT"));
        }
        self.kex.peer_kexinit = peer_kexinit;

        // Perform client-side DH
        let (exchange_hash, shared_secret, _host_key_blob) = kex::client_kex_ecdh(
            &mut self.io,
            &mut self.kex,
            BSSH_VERSION,
            &self.server_version,
        )?;

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
        let (cipher_c2s, cipher_s2c) =
            kex::derive_keys(&shared_secret, &exchange_hash, session_id);

        self.io.set_cipher_send(cipher_c2s); // client→server = our send
        self.io.set_cipher_recv(cipher_s2c); // server→client = our recv

        // 3. Authentication
        auth::client_request_service(&mut self.io)?;
        auth::client_auth_password(&mut self.io, username, password)?;

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
            SSH_MSG_CHANNEL_DATA => {
                let mut pos = 1;
                let _channel_id = SshBuf::get_u32(&msg, &mut pos);
                let data = SshBuf::get_string(&msg, &mut pos)
                    .ok_or(SshError::Protocol("bad channel data"))?;

                if let Some(ref mut ch) = self.channel {
                    ch.recv_window = ch.recv_window.saturating_sub(data.len() as u32);
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
            SSH_MSG_CHANNEL_EOF | SSH_MSG_CHANNEL_CLOSE => Err(SshError::Disconnected),
            SSH_MSG_DISCONNECT => Err(SshError::Disconnected),
            SSH_MSG_IGNORE | SSH_MSG_UNIMPLEMENTED => Ok(None),
            _ => Ok(None),
        }
    }

    /// Close the session.
    pub fn close(&mut self) {
        if let Some(ref ch) = self.channel {
            let _ = channel::send_channel_eof(&mut self.io, ch);
            let _ = channel::send_channel_close(&mut self.io, ch);
        }
    }

    /// Get the underlying packet I/O (for advanced use).
    pub fn io(&mut self) -> &mut PacketIo {
        &mut self.io
    }
}
