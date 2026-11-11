//! Pure, length-delimited collaboration messages. Integers are little-endian.
extern crate alloc;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    BadMagic,
    BadLength,
    UnknownType(u8),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawOp {
    Pencil {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        color: [u8; 3],
    },
    Brush {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        radius: u8,
        color: [u8; 3],
    },
    Eraser {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        radius: u8,
    },
    Line {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        color: [u8; 3],
    },
    Rectangle {
        x: i16,
        y: i16,
        width: i16,
        height: i16,
        color: [u8; 3],
    },
    Circle {
        x: i16,
        y: i16,
        radius: i16,
        color: [u8; 3],
    },
    Fill {
        x: i16,
        y: i16,
        color: [u8; 3],
    },
    Clear,
    Undo,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello {
        version: u16,
        name: Vec<u8>,
    },
    Welcome {
        peer: u8,
        width: u16,
        height: u16,
    },
    JoinRefused {
        reason: Vec<u8>,
    },
    SyncChunk {
        offset: u32,
        data: Vec<u8>,
    },
    SyncEnd,
    Op {
        local: u32,
        op: DrawOp,
    },
    Ack {
        local: u32,
        global: u64,
    },
    Refused {
        local: u32,
        reason: Vec<u8>,
    },
    Apply {
        global: u64,
        peer: u8,
        local: u32,
        op: DrawOp,
    },
    PeerJoined {
        peer: u8,
        name: Vec<u8>,
    },
    PeerLeft {
        peer: u8,
        reason: Vec<u8>,
    },
    Bye,
}
fn put_drawop(v: &DrawOp, b: &mut Vec<u8>) {
    match v {
        DrawOp::Pencil {
            x0,
            y0,
            x1,
            y1,
            color,
        } => {
            b.push(1);
            b.extend_from_slice(&x0.to_le_bytes());
            b.extend_from_slice(&y0.to_le_bytes());
            b.extend_from_slice(&x1.to_le_bytes());
            b.extend_from_slice(&y1.to_le_bytes());
            b.extend_from_slice(color);
        }
        DrawOp::Brush {
            x0,
            y0,
            x1,
            y1,
            radius,
            color,
        } => {
            b.push(2);
            b.extend_from_slice(&x0.to_le_bytes());
            b.extend_from_slice(&y0.to_le_bytes());
            b.extend_from_slice(&x1.to_le_bytes());
            b.extend_from_slice(&y1.to_le_bytes());
            b.push(*radius);
            b.extend_from_slice(color);
        }
        DrawOp::Eraser {
            x0,
            y0,
            x1,
            y1,
            radius,
        } => {
            b.push(3);
            b.extend_from_slice(&x0.to_le_bytes());
            b.extend_from_slice(&y0.to_le_bytes());
            b.extend_from_slice(&x1.to_le_bytes());
            b.extend_from_slice(&y1.to_le_bytes());
            b.push(*radius);
        }
        DrawOp::Line {
            x0,
            y0,
            x1,
            y1,
            color,
        } => {
            b.push(4);
            b.extend_from_slice(&x0.to_le_bytes());
            b.extend_from_slice(&y0.to_le_bytes());
            b.extend_from_slice(&x1.to_le_bytes());
            b.extend_from_slice(&y1.to_le_bytes());
            b.extend_from_slice(color);
        }
        DrawOp::Rectangle {
            x,
            y,
            width,
            height,
            color,
        } => {
            b.push(5);
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(&width.to_le_bytes());
            b.extend_from_slice(&height.to_le_bytes());
            b.extend_from_slice(color);
        }
        DrawOp::Circle {
            x,
            y,
            radius,
            color,
        } => {
            b.push(6);
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(&radius.to_le_bytes());
            b.extend_from_slice(color);
        }
        DrawOp::Fill { x, y, color } => {
            b.push(7);
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(color);
        }
        DrawOp::Clear => {
            b.push(8);
        }
        DrawOp::Undo => {
            b.push(9);
        }
    }
}
fn put_message(v: &Message, b: &mut Vec<u8>) -> u8 {
    match v {
        Message::Hello { version, name } => {
            b.extend_from_slice(&version.to_le_bytes());
            b.extend_from_slice(name);
            1
        }
        Message::Welcome {
            peer,
            width,
            height,
        } => {
            b.push(*peer);
            b.extend_from_slice(&width.to_le_bytes());
            b.extend_from_slice(&height.to_le_bytes());
            2
        }
        Message::JoinRefused { reason } => {
            b.extend_from_slice(reason);
            3
        }
        Message::SyncChunk { offset, data } => {
            b.extend_from_slice(&offset.to_le_bytes());
            b.extend_from_slice(data);
            4
        }
        Message::SyncEnd => 5,
        Message::Op { local, op } => {
            b.extend_from_slice(&local.to_le_bytes());
            put_drawop(op, b);
            6
        }
        Message::Ack { local, global } => {
            b.extend_from_slice(&local.to_le_bytes());
            b.extend_from_slice(&global.to_le_bytes());
            7
        }
        Message::Refused { local, reason } => {
            b.extend_from_slice(&local.to_le_bytes());
            b.extend_from_slice(reason);
            8
        }
        Message::Apply {
            global,
            peer,
            local,
            op,
        } => {
            b.extend_from_slice(&global.to_le_bytes());
            b.push(*peer);
            b.extend_from_slice(&local.to_le_bytes());
            put_drawop(op, b);
            9
        }
        Message::PeerJoined { peer, name } => {
            b.push(*peer);
            b.extend_from_slice(name);
            10
        }
        Message::PeerLeft { peer, reason } => {
            b.push(*peer);
            b.extend_from_slice(reason);
            11
        }
        Message::Bye => 12,
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let end = self.pos.checked_add(N).ok_or(DecodeError::BadLength)?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(DecodeError::BadLength)?;
        self.pos = end;
        bytes.try_into().map_err(|_| DecodeError::BadLength)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }
    fn rest(&mut self) -> &'a [u8] {
        let rest = &self.bytes[self.pos..];
        self.pos = self.bytes.len();
        rest
    }
}
fn get_drawop(tag: u8, r: &mut Reader<'_>) -> Result<DrawOp, DecodeError> {
    Ok(match tag {
        1 => DrawOp::Pencil {
            x0: i16::from_le_bytes(r.array()?),
            y0: i16::from_le_bytes(r.array()?),
            x1: i16::from_le_bytes(r.array()?),
            y1: i16::from_le_bytes(r.array()?),
            color: r.array()?,
        },
        2 => DrawOp::Brush {
            x0: i16::from_le_bytes(r.array()?),
            y0: i16::from_le_bytes(r.array()?),
            x1: i16::from_le_bytes(r.array()?),
            y1: i16::from_le_bytes(r.array()?),
            radius: r.u8()?,
            color: r.array()?,
        },
        3 => DrawOp::Eraser {
            x0: i16::from_le_bytes(r.array()?),
            y0: i16::from_le_bytes(r.array()?),
            x1: i16::from_le_bytes(r.array()?),
            y1: i16::from_le_bytes(r.array()?),
            radius: r.u8()?,
        },
        4 => DrawOp::Line {
            x0: i16::from_le_bytes(r.array()?),
            y0: i16::from_le_bytes(r.array()?),
            x1: i16::from_le_bytes(r.array()?),
            y1: i16::from_le_bytes(r.array()?),
            color: r.array()?,
        },
        5 => DrawOp::Rectangle {
            x: i16::from_le_bytes(r.array()?),
            y: i16::from_le_bytes(r.array()?),
            width: i16::from_le_bytes(r.array()?),
            height: i16::from_le_bytes(r.array()?),
            color: r.array()?,
        },
        6 => DrawOp::Circle {
            x: i16::from_le_bytes(r.array()?),
            y: i16::from_le_bytes(r.array()?),
            radius: i16::from_le_bytes(r.array()?),
            color: r.array()?,
        },
        7 => DrawOp::Fill {
            x: i16::from_le_bytes(r.array()?),
            y: i16::from_le_bytes(r.array()?),
            color: r.array()?,
        },
        8 => DrawOp::Clear,
        9 => DrawOp::Undo,
        _ => return Err(DecodeError::UnknownType(tag)),
    })
}
fn get_message(tag: u8, r: &mut Reader<'_>) -> Result<Message, DecodeError> {
    Ok(match tag {
        1 => Message::Hello {
            version: u16::from_le_bytes(r.array()?),
            name: r.rest().to_vec(),
        },
        2 => Message::Welcome {
            peer: r.u8()?,
            width: u16::from_le_bytes(r.array()?),
            height: u16::from_le_bytes(r.array()?),
        },
        3 => Message::JoinRefused {
            reason: r.rest().to_vec(),
        },
        4 => Message::SyncChunk {
            offset: u32::from_le_bytes(r.array()?),
            data: r.rest().to_vec(),
        },
        5 => Message::SyncEnd,
        6 => Message::Op {
            local: u32::from_le_bytes(r.array()?),
            op: {
                let tag = r.u8()?;
                get_drawop(tag, r)?
            },
        },
        7 => Message::Ack {
            local: u32::from_le_bytes(r.array()?),
            global: u64::from_le_bytes(r.array()?),
        },
        8 => Message::Refused {
            local: u32::from_le_bytes(r.array()?),
            reason: r.rest().to_vec(),
        },
        9 => Message::Apply {
            global: u64::from_le_bytes(r.array()?),
            peer: r.u8()?,
            local: u32::from_le_bytes(r.array()?),
            op: {
                let tag = r.u8()?;
                get_drawop(tag, r)?
            },
        },
        10 => Message::PeerJoined {
            peer: r.u8()?,
            name: r.rest().to_vec(),
        },
        11 => Message::PeerLeft {
            peer: r.u8()?,
            reason: r.rest().to_vec(),
        },
        12 => Message::Bye,
        _ => return Err(DecodeError::UnknownType(tag)),
    })
}

/// Encode a message, rejecting payloads that cannot fit in the wire length.
pub fn encode(message: &Message) -> Result<Vec<u8>, EncodeError> {
    let mut payload = Vec::new();
    let tag = put_message(message, &mut payload);
    let len = u16::try_from(payload.len()).map_err(|_| EncodeError)?;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&[0xbc, tag]);
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}
/// Decode one frame and return the number of bytes consumed, or wait for more input.
pub fn decode(bytes: &[u8]) -> Result<Option<(Message, usize)>, DecodeError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes[0] != 0xbc {
        return Err(DecodeError::BadMagic);
    }
    if bytes.len() < 4 {
        return Ok(None);
    }
    let tag = bytes[1];
    if !(1..=12).contains(&tag) {
        return Err(DecodeError::UnknownType(tag));
    }
    let len = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    let end = 4 + len;
    if bytes.len() < end {
        return Ok(None);
    }
    let mut r = Reader {
        bytes: &bytes[4..end],
        pos: 0,
    };
    let message = get_message(tag, &mut r)?;
    if r.pos != len {
        return Err(DecodeError::BadLength);
    }
    Ok(Some((message, end)))
}
