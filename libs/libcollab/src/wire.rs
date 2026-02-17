//! Wire protocol: message framing, types, encode/decode, and stream decoder.
//!
//! Frame format (8-byte header + payload):
//! ```text
//! [magic:u8=0xBC] [type:u8] [length:u16 LE] [seqno:u32 LE] [payload: length bytes]
//! ```

/// Magic byte identifying BCP frames
pub const MAGIC: u8 = 0xBC;

/// Maximum payload size
pub const MAX_PAYLOAD: usize = 65535;

/// Header size in bytes
pub const HEADER_SIZE: usize = 8;

/// Protocol version
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum display name length
pub const MAX_NAME_LEN: usize = 32;

/// Message type identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    // Session (0x01-0x06)
    Hello = 0x01,
    Welcome = 0x02,
    PeerJoined = 0x03,
    PeerLeft = 0x04,
    Bye = 0x05,

    // State Sync (0x10-0x12)
    SyncMeta = 0x10,
    SyncChunk = 0x11,
    SyncEnd = 0x12,

    // Drawing Operations (0x20-0x27)
    OpPencil = 0x20,
    OpBrush = 0x21,
    OpEraser = 0x22,
    OpLine = 0x23,
    OpRect = 0x24,
    OpCircle = 0x25,
    OpFill = 0x26,
    OpClear = 0x27,

    // Presence (0x40-0x41)
    Cursor = 0x40,
    ToolChange = 0x41,

    // Keepalive (0x50-0x51)
    Ping = 0x50,
    Pong = 0x51,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Hello),
            0x02 => Some(Self::Welcome),
            0x03 => Some(Self::PeerJoined),
            0x04 => Some(Self::PeerLeft),
            0x05 => Some(Self::Bye),
            0x10 => Some(Self::SyncMeta),
            0x11 => Some(Self::SyncChunk),
            0x12 => Some(Self::SyncEnd),
            0x20 => Some(Self::OpPencil),
            0x21 => Some(Self::OpBrush),
            0x22 => Some(Self::OpEraser),
            0x23 => Some(Self::OpLine),
            0x24 => Some(Self::OpRect),
            0x25 => Some(Self::OpCircle),
            0x26 => Some(Self::OpFill),
            0x27 => Some(Self::OpClear),
            0x40 => Some(Self::Cursor),
            0x41 => Some(Self::ToolChange),
            0x50 => Some(Self::Ping),
            0x51 => Some(Self::Pong),
            _ => None,
        }
    }
}

/// Parsed message header
#[derive(Debug, Clone, Copy)]
pub struct MsgHeader {
    pub msg_type: MessageType,
    pub length: u16,
    pub seqno: u32,
}

/// Encode a message into a buffer. Returns the total number of bytes written
/// (header + payload), or None if the buffer is too small.
pub fn encode(buf: &mut [u8], msg_type: MessageType, seqno: u32, payload: &[u8]) -> Option<usize> {
    let total = HEADER_SIZE + payload.len();
    if total > buf.len() || payload.len() > MAX_PAYLOAD {
        return None;
    }
    buf[0] = MAGIC;
    buf[1] = msg_type as u8;
    let len_bytes = (payload.len() as u16).to_le_bytes();
    buf[2] = len_bytes[0];
    buf[3] = len_bytes[1];
    let seq_bytes = seqno.to_le_bytes();
    buf[4] = seq_bytes[0];
    buf[5] = seq_bytes[1];
    buf[6] = seq_bytes[2];
    buf[7] = seq_bytes[3];
    buf[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
    Some(total)
}

/// Decode a header from a buffer. Buffer must be at least HEADER_SIZE bytes.
pub fn decode_header(buf: &[u8]) -> Option<MsgHeader> {
    if buf.len() < HEADER_SIZE || buf[0] != MAGIC {
        return None;
    }
    let msg_type = MessageType::from_u8(buf[1])?;
    let length = u16::from_le_bytes([buf[2], buf[3]]);
    let seqno = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    Some(MsgHeader {
        msg_type,
        length,
        seqno,
    })
}

/// Incremental stream decoder that handles partial reads.
///
/// Call `feed()` with new data, then `next_message()` to extract complete frames.
pub struct StreamDecoder {
    buf: Vec<u8>,
    pos: usize,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            pos: 0,
        }
    }

    /// Feed new data from a socket read.
    pub fn feed(&mut self, data: &[u8]) {
        // Compact buffer if we've consumed a lot
        if self.pos > 0 && self.pos > self.buf.len() / 2 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(data);
    }

    /// Try to extract the next complete message.
    /// Returns (header, payload_slice) if a complete message is available.
    pub fn next_message(&mut self) -> Option<(MsgHeader, Vec<u8>)> {
        let remaining = &self.buf[self.pos..];
        if remaining.len() < HEADER_SIZE {
            return None;
        }

        // Scan for magic byte (in case of stream corruption)
        if remaining[0] != MAGIC {
            // Skip bytes until we find magic or exhaust buffer
            for i in 1..remaining.len() {
                if remaining[i] == MAGIC {
                    self.pos += i;
                    return self.next_message();
                }
            }
            // No magic found, discard everything
            self.pos = self.buf.len();
            return None;
        }

        let header = decode_header(remaining)?;
        let total = HEADER_SIZE + header.length as usize;
        if remaining.len() < total {
            return None; // Incomplete message
        }

        let payload = remaining[HEADER_SIZE..total].to_vec();
        self.pos += total;
        Some((header, payload))
    }

}

// ---- Payload encoding helpers ----

/// Write a u8 to buf at offset, return new offset
#[inline]
pub fn put_u8(buf: &mut [u8], off: usize, v: u8) -> usize {
    buf[off] = v;
    off + 1
}

/// Write a u16 LE to buf at offset, return new offset
#[inline]
pub fn put_u16(buf: &mut [u8], off: usize, v: u16) -> usize {
    let b = v.to_le_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
    off + 2
}

/// Write an i16 LE to buf at offset, return new offset
#[inline]
pub fn put_i16(buf: &mut [u8], off: usize, v: i16) -> usize {
    let b = v.to_le_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
    off + 2
}

/// Write a u32 LE to buf at offset, return new offset
#[inline]
pub fn put_u32(buf: &mut [u8], off: usize, v: u32) -> usize {
    let b = v.to_le_bytes();
    buf[off..off + 4].copy_from_slice(&b);
    off + 4
}

/// Read a u8 from buf at offset
#[inline]
pub fn get_u8(buf: &[u8], off: usize) -> u8 {
    buf[off]
}

/// Read a u16 LE from buf at offset
#[inline]
pub fn get_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

/// Read an i16 LE from buf at offset
#[inline]
pub fn get_i16(buf: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([buf[off], buf[off + 1]])
}

/// Read a u32 LE from buf at offset
#[inline]
pub fn get_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}
