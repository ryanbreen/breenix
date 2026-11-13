#![no_std]

//! Pure collaboration protocol types and framing.
pub mod endpoint;
pub mod host;
pub mod queue;
#[cfg(feature = "net")]
pub mod net;
pub mod wire;

extern crate alloc;
pub mod peer;

/// Return the surviving drawing operations in their original global order.
pub fn live_ops(log: &[wire::Message]) -> alloc::vec::Vec<wire::Message> {
    let mut live: alloc::vec::Vec<wire::Message> = alloc::vec::Vec::new();
    for entry in log {
        if let wire::Message::Apply { peer, op, .. } = entry {
            if *op == wire::DrawOp::Undo {
                if let Some(index) = live.iter().rposition(
                    |m| matches!(m, wire::Message::Apply { peer: owner, .. } if owner == peer),
                ) {
                    let target = gesture_key(&live[index]);
                    live.retain(|m| gesture_key(m) != target);
                }
            } else {
                live.push(entry.clone());
            }
        }
    }
    live
}

/// Drawing identity: segmented freehand gestures use the issuer's first local id;
/// other drawing operations retain their unique global identity.
pub fn gesture_key(message: &wire::Message) -> Option<(u8, bool, u64)> {
    match message {
        wire::Message::Apply { peer, op: wire::DrawOp::StrokeSegment { gesture, .. }, .. } =>
            Some((*peer, true, u64::from(*gesture))),
        wire::Message::Apply { peer, global, .. } => Some((*peer, false, *global)),
        _ => None,
    }
}
