//! SSH connection layer — channels (RFC 4254)
//!
//! Implements SSH session channels with PTY allocation and shell execution.

use super::packet::PacketIo;
use super::{SshBuf, SshError};
use super::{
    SSH_MSG_CHANNEL_CLOSE, SSH_MSG_CHANNEL_DATA, SSH_MSG_CHANNEL_EOF, SSH_MSG_CHANNEL_FAILURE,
    SSH_MSG_CHANNEL_OPEN, SSH_MSG_CHANNEL_OPEN_CONFIRMATION, SSH_MSG_CHANNEL_REQUEST,
    SSH_MSG_CHANNEL_SUCCESS, SSH_MSG_CHANNEL_WINDOW_ADJUST,
};

/// Default initial window size (1 MB).
const INITIAL_WINDOW_SIZE: u32 = 1024 * 1024;

/// Maximum packet size for channel data.
const MAX_PACKET_SIZE: u32 = 32768;

/// SSH channel state.
pub struct Channel {
    /// Our channel ID.
    pub local_id: u32,
    /// Peer's channel ID.
    pub remote_id: u32,
    /// Remaining send window (bytes the peer is willing to accept).
    pub send_window: u32,
    /// Remaining receive window (bytes we are willing to accept).
    pub recv_window: u32,
    /// Maximum packet size for this channel.
    pub max_packet: u32,
    /// Whether the channel has received EOF.
    pub eof_received: bool,
    /// Whether the channel has been closed.
    pub closed: bool,
    /// PTY terminal type (if a PTY was requested).
    pub pty_term: Option<String>,
    /// PTY dimensions (cols, rows).
    pub pty_size: (u32, u32),
}

impl Channel {
    /// Create a new channel with default settings.
    pub fn new(local_id: u32, remote_id: u32, send_window: u32, max_packet: u32) -> Self {
        Self {
            local_id,
            remote_id,
            send_window,
            recv_window: INITIAL_WINDOW_SIZE,
            max_packet,
            eof_received: false,
            closed: false,
            pty_term: None,
            pty_size: (80, 24),
        }
    }
}

/// Handle an incoming "session" channel open request (server side).
///
/// Returns the new Channel on success.
pub fn server_handle_channel_open(
    io: &mut PacketIo,
    msg: &[u8],
    local_id: u32,
) -> Result<Channel, SshError> {
    let mut pos = 1; // skip message type
    let channel_type = SshBuf::get_string(msg, &mut pos)
        .ok_or(SshError::Protocol("bad channel type"))?;
    let sender_channel = SshBuf::get_u32(msg, &mut pos)
        .ok_or(SshError::Protocol("bad sender channel"))?;
    let initial_window = SshBuf::get_u32(msg, &mut pos)
        .ok_or(SshError::Protocol("bad initial window"))?;
    let max_packet = SshBuf::get_u32(msg, &mut pos)
        .ok_or(SshError::Protocol("bad max packet"))?;

    if channel_type != b"session" {
        // Send channel open failure
        let mut fail = Vec::with_capacity(32);
        fail.push(SSH_MSG_CHANNEL_OPEN_CONFIRMATION + 1); // CHANNEL_OPEN_FAILURE = 92
        SshBuf::put_u32(&mut fail, sender_channel);
        SshBuf::put_u32(&mut fail, 3); // SSH_OPEN_UNKNOWN_CHANNEL_TYPE
        SshBuf::put_string(&mut fail, b"unsupported channel type");
        SshBuf::put_string(&mut fail, b"");
        io.send_packet(&fail).map_err(|_| SshError::Io)?;
        return Err(SshError::Protocol("unsupported channel type"));
    }

    let channel = Channel::new(local_id, sender_channel, initial_window, max_packet);

    // Send CHANNEL_OPEN_CONFIRMATION
    let mut confirm = Vec::with_capacity(32);
    confirm.push(SSH_MSG_CHANNEL_OPEN_CONFIRMATION);
    SshBuf::put_u32(&mut confirm, sender_channel); // recipient channel
    SshBuf::put_u32(&mut confirm, local_id); // sender channel
    SshBuf::put_u32(&mut confirm, INITIAL_WINDOW_SIZE); // initial window
    SshBuf::put_u32(&mut confirm, MAX_PACKET_SIZE); // maximum packet size
    io.send_packet(&confirm).map_err(|_| SshError::Io)?;

    Ok(channel)
}

/// Handle a channel request (server side).
///
/// Processes "pty-req", "shell", "exec", and "env" requests.
/// Returns the request type name for the caller to act on.
pub fn server_handle_channel_request(
    io: &mut PacketIo,
    msg: &[u8],
    channel: &mut Channel,
) -> Result<String, SshError> {
    let mut pos = 1;
    let _recipient = SshBuf::get_u32(msg, &mut pos)
        .ok_or(SshError::Protocol("bad channel id in request"))?;
    let request_type = SshBuf::get_string(msg, &mut pos)
        .ok_or(SshError::Protocol("bad request type"))?;
    let want_reply = SshBuf::get_bool(msg, &mut pos)
        .ok_or(SshError::Protocol("bad want_reply"))?;

    let req_name = String::from_utf8_lossy(request_type).into_owned();

    match request_type {
        b"pty-req" => {
            let term = SshBuf::get_string(msg, &mut pos).unwrap_or(b"xterm");
            let cols = SshBuf::get_u32(msg, &mut pos).unwrap_or(80);
            let rows = SshBuf::get_u32(msg, &mut pos).unwrap_or(24);
            // width_px, height_px, terminal modes (ignored)
            channel.pty_term = Some(String::from_utf8_lossy(term).into_owned());
            channel.pty_size = (cols, rows);

            if want_reply {
                send_channel_success(io, channel)?;
            }
        }
        b"shell" | b"exec" => {
            if want_reply {
                send_channel_success(io, channel)?;
            }
        }
        b"env" | b"window-change" => {
            if request_type == b"window-change" {
                let cols = SshBuf::get_u32(msg, &mut pos).unwrap_or(80);
                let rows = SshBuf::get_u32(msg, &mut pos).unwrap_or(24);
                channel.pty_size = (cols, rows);
            }
            if want_reply {
                send_channel_success(io, channel)?;
            }
        }
        _ => {
            if want_reply {
                send_channel_failure(io, channel)?;
            }
        }
    }

    Ok(req_name)
}

/// Send channel data to the peer.
pub fn send_channel_data(
    io: &mut PacketIo,
    channel: &mut Channel,
    data: &[u8],
) -> Result<(), SshError> {
    let mut offset = 0;
    while offset < data.len() {
        let chunk_size = core::cmp::min(
            data.len() - offset,
            core::cmp::min(channel.send_window as usize, channel.max_packet as usize),
        );

        if chunk_size == 0 {
            // Window exhausted — in a full implementation we'd wait for
            // WINDOW_ADJUST. For now, send anyway (most clients have large windows).
            break;
        }

        let mut msg = Vec::with_capacity(9 + chunk_size);
        msg.push(SSH_MSG_CHANNEL_DATA);
        SshBuf::put_u32(&mut msg, channel.remote_id);
        SshBuf::put_string(&mut msg, &data[offset..offset + chunk_size]);
        io.send_packet(&msg).map_err(|_| SshError::Io)?;

        channel.send_window = channel.send_window.saturating_sub(chunk_size as u32);
        offset += chunk_size;
    }
    Ok(())
}

/// Send a window adjust to the peer (increase their send window).
pub fn send_window_adjust(
    io: &mut PacketIo,
    channel: &mut Channel,
    bytes_to_add: u32,
) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(9);
    msg.push(SSH_MSG_CHANNEL_WINDOW_ADJUST);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    SshBuf::put_u32(&mut msg, bytes_to_add);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;
    channel.recv_window += bytes_to_add;
    Ok(())
}

/// Send channel EOF.
pub fn send_channel_eof(io: &mut PacketIo, channel: &Channel) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(5);
    msg.push(SSH_MSG_CHANNEL_EOF);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;
    Ok(())
}

/// Send channel close.
pub fn send_channel_close(io: &mut PacketIo, channel: &Channel) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(5);
    msg.push(SSH_MSG_CHANNEL_CLOSE);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;
    Ok(())
}

fn send_channel_success(io: &mut PacketIo, channel: &Channel) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(5);
    msg.push(SSH_MSG_CHANNEL_SUCCESS);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;
    Ok(())
}

fn send_channel_failure(io: &mut PacketIo, channel: &Channel) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(5);
    msg.push(SSH_MSG_CHANNEL_FAILURE);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;
    Ok(())
}

// ============================================================================
// Client-side channel operations
// ============================================================================

/// Open a session channel (client side).
pub fn client_open_session(io: &mut PacketIo) -> Result<Channel, SshError> {
    let local_id: u32 = 0;

    let mut msg = Vec::with_capacity(32);
    msg.push(SSH_MSG_CHANNEL_OPEN);
    SshBuf::put_string(&mut msg, b"session");
    SshBuf::put_u32(&mut msg, local_id);
    SshBuf::put_u32(&mut msg, INITIAL_WINDOW_SIZE);
    SshBuf::put_u32(&mut msg, MAX_PACKET_SIZE);
    io.send_packet(&msg).map_err(|_| SshError::Io)?;

    // Wait for confirmation
    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_CHANNEL_OPEN_CONFIRMATION {
        return Err(SshError::Protocol("channel open failed"));
    }

    let mut pos = 1;
    let _recipient = SshBuf::get_u32(&reply, &mut pos);
    let sender = SshBuf::get_u32(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad channel confirmation"))?;
    let initial_window = SshBuf::get_u32(&reply, &mut pos).unwrap_or(INITIAL_WINDOW_SIZE);
    let max_packet = SshBuf::get_u32(&reply, &mut pos).unwrap_or(MAX_PACKET_SIZE);

    Ok(Channel::new(local_id, sender, initial_window, max_packet))
}

/// Request a PTY (client side).
pub fn client_request_pty(
    io: &mut PacketIo,
    channel: &Channel,
    term: &str,
    cols: u32,
    rows: u32,
) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(64);
    msg.push(SSH_MSG_CHANNEL_REQUEST);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    SshBuf::put_string(&mut msg, b"pty-req");
    SshBuf::put_bool(&mut msg, true); // want reply
    SshBuf::put_string(&mut msg, term.as_bytes());
    SshBuf::put_u32(&mut msg, cols);
    SshBuf::put_u32(&mut msg, rows);
    SshBuf::put_u32(&mut msg, 0); // width_px
    SshBuf::put_u32(&mut msg, 0); // height_px
    SshBuf::put_string(&mut msg, &[]); // terminal modes (empty)
    io.send_packet(&msg).map_err(|_| SshError::Io)?;

    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_CHANNEL_SUCCESS {
        return Err(SshError::Protocol("pty request failed"));
    }

    Ok(())
}

/// Request a shell (client side).
pub fn client_request_shell(io: &mut PacketIo, channel: &Channel) -> Result<(), SshError> {
    let mut msg = Vec::with_capacity(16);
    msg.push(SSH_MSG_CHANNEL_REQUEST);
    SshBuf::put_u32(&mut msg, channel.remote_id);
    SshBuf::put_string(&mut msg, b"shell");
    SshBuf::put_bool(&mut msg, true); // want reply
    io.send_packet(&msg).map_err(|_| SshError::Io)?;

    let reply = io.recv_packet().map_err(|_| SshError::Io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_CHANNEL_SUCCESS {
        return Err(SshError::Protocol("shell request failed"));
    }

    Ok(())
}
