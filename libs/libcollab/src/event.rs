//! Collaboration events and drawing operations with serialization.

use crate::wire::{self, MessageType};

/// A drawing operation that can be applied to a canvas.
#[derive(Debug, Clone)]
pub enum DrawOp {
    Pencil {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        r: u8,
        g: u8,
        b: u8,
    },
    Brush {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        radius: u8,
        r: u8,
        g: u8,
        b: u8,
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
        r: u8,
        g: u8,
        b: u8,
    },
    Rect {
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        r: u8,
        g: u8,
        b: u8,
    },
    Circle {
        cx: i16,
        cy: i16,
        radius: i16,
        r: u8,
        g: u8,
        b: u8,
    },
    Fill {
        x: i16,
        y: i16,
        r: u8,
        g: u8,
        b: u8,
    },
    Clear,
}

/// An event received from the collaboration session.
#[derive(Debug, Clone)]
pub enum CollabEvent {
    PeerJoined {
        peer_id: u8,
        name: [u8; 32],
        name_len: u8,
    },
    PeerLeft {
        peer_id: u8,
    },
    DrawOp(DrawOp),
    CursorMoved {
        peer_id: u8,
        x: i16,
        y: i16,
        visible: bool,
    },
    ToolChanged {
        peer_id: u8,
        tool: u8,
        brush_size: u8,
        r: u8,
        g: u8,
        b: u8,
    },
    SyncChunk {
        offset: u32,
        data: Vec<u8>,
    },
    SyncComplete,
    SessionEnded,
}

// ---- DrawOp serialization ----

impl DrawOp {
    /// Encode a DrawOp into a payload buffer. Returns (MessageType, payload_len).
    pub fn encode(&self, buf: &mut [u8]) -> (MessageType, usize) {
        match self {
            DrawOp::Pencil {
                x0,
                y0,
                x1,
                y1,
                r,
                g,
                b,
            } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x0);
                off = wire::put_i16(buf, off, *y0);
                off = wire::put_i16(buf, off, *x1);
                off = wire::put_i16(buf, off, *y1);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpPencil, off)
            }
            DrawOp::Brush {
                x0,
                y0,
                x1,
                y1,
                radius,
                r,
                g,
                b,
            } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x0);
                off = wire::put_i16(buf, off, *y0);
                off = wire::put_i16(buf, off, *x1);
                off = wire::put_i16(buf, off, *y1);
                off = wire::put_u8(buf, off, *radius);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpBrush, off)
            }
            DrawOp::Eraser {
                x0,
                y0,
                x1,
                y1,
                radius,
            } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x0);
                off = wire::put_i16(buf, off, *y0);
                off = wire::put_i16(buf, off, *x1);
                off = wire::put_i16(buf, off, *y1);
                off = wire::put_u8(buf, off, *radius);
                (MessageType::OpEraser, off)
            }
            DrawOp::Line {
                x0,
                y0,
                x1,
                y1,
                r,
                g,
                b,
            } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x0);
                off = wire::put_i16(buf, off, *y0);
                off = wire::put_i16(buf, off, *x1);
                off = wire::put_i16(buf, off, *y1);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpLine, off)
            }
            DrawOp::Rect { x, y, w, h, r, g, b } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x);
                off = wire::put_i16(buf, off, *y);
                off = wire::put_i16(buf, off, *w);
                off = wire::put_i16(buf, off, *h);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpRect, off)
            }
            DrawOp::Circle {
                cx,
                cy,
                radius,
                r,
                g,
                b,
            } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *cx);
                off = wire::put_i16(buf, off, *cy);
                off = wire::put_i16(buf, off, *radius);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpCircle, off)
            }
            DrawOp::Fill { x, y, r, g, b } => {
                let mut off = 0;
                off = wire::put_i16(buf, off, *x);
                off = wire::put_i16(buf, off, *y);
                off = wire::put_u8(buf, off, *r);
                off = wire::put_u8(buf, off, *g);
                off = wire::put_u8(buf, off, *b);
                (MessageType::OpFill, off)
            }
            DrawOp::Clear => (MessageType::OpClear, 0),
        }
    }

    /// Decode a DrawOp from a message type and payload.
    pub fn decode(msg_type: MessageType, payload: &[u8]) -> Option<Self> {
        match msg_type {
            MessageType::OpPencil if payload.len() >= 11 => Some(DrawOp::Pencil {
                x0: wire::get_i16(payload, 0),
                y0: wire::get_i16(payload, 2),
                x1: wire::get_i16(payload, 4),
                y1: wire::get_i16(payload, 6),
                r: wire::get_u8(payload, 8),
                g: wire::get_u8(payload, 9),
                b: wire::get_u8(payload, 10),
            }),
            MessageType::OpBrush if payload.len() >= 12 => Some(DrawOp::Brush {
                x0: wire::get_i16(payload, 0),
                y0: wire::get_i16(payload, 2),
                x1: wire::get_i16(payload, 4),
                y1: wire::get_i16(payload, 6),
                radius: wire::get_u8(payload, 8),
                r: wire::get_u8(payload, 9),
                g: wire::get_u8(payload, 10),
                b: wire::get_u8(payload, 11),
            }),
            MessageType::OpEraser if payload.len() >= 9 => Some(DrawOp::Eraser {
                x0: wire::get_i16(payload, 0),
                y0: wire::get_i16(payload, 2),
                x1: wire::get_i16(payload, 4),
                y1: wire::get_i16(payload, 6),
                radius: wire::get_u8(payload, 8),
            }),
            MessageType::OpLine if payload.len() >= 11 => Some(DrawOp::Line {
                x0: wire::get_i16(payload, 0),
                y0: wire::get_i16(payload, 2),
                x1: wire::get_i16(payload, 4),
                y1: wire::get_i16(payload, 6),
                r: wire::get_u8(payload, 8),
                g: wire::get_u8(payload, 9),
                b: wire::get_u8(payload, 10),
            }),
            MessageType::OpRect if payload.len() >= 11 => Some(DrawOp::Rect {
                x: wire::get_i16(payload, 0),
                y: wire::get_i16(payload, 2),
                w: wire::get_i16(payload, 4),
                h: wire::get_i16(payload, 6),
                r: wire::get_u8(payload, 8),
                g: wire::get_u8(payload, 9),
                b: wire::get_u8(payload, 10),
            }),
            MessageType::OpCircle if payload.len() >= 9 => Some(DrawOp::Circle {
                cx: wire::get_i16(payload, 0),
                cy: wire::get_i16(payload, 2),
                radius: wire::get_i16(payload, 4),
                r: wire::get_u8(payload, 6),
                g: wire::get_u8(payload, 7),
                b: wire::get_u8(payload, 8),
            }),
            MessageType::OpFill if payload.len() >= 7 => Some(DrawOp::Fill {
                x: wire::get_i16(payload, 0),
                y: wire::get_i16(payload, 2),
                r: wire::get_u8(payload, 4),
                g: wire::get_u8(payload, 5),
                b: wire::get_u8(payload, 6),
            }),
            MessageType::OpClear => Some(DrawOp::Clear),
            _ => None,
        }
    }

    /// Check if a message type is a draw operation.
    pub fn is_draw_op(msg_type: MessageType) -> bool {
        matches!(
            msg_type,
            MessageType::OpPencil
                | MessageType::OpBrush
                | MessageType::OpEraser
                | MessageType::OpLine
                | MessageType::OpRect
                | MessageType::OpCircle
                | MessageType::OpFill
                | MessageType::OpClear
        )
    }
}
