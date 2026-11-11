#![no_std]

//! Pure collaboration protocol types and framing.
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
                    live.remove(index);
                }
            } else {
                live.push(entry.clone());
            }
        }
    }
    live
}
