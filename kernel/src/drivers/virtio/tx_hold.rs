//! Hold virtio-net transmissions while the RX ring is being consumed.
//!
//! Parallels' legacy virtio-net PCI device (1af4:1000) drops a transmission
//! that is published to the TX ring while the driver is consuming the RX ring:
//! from the first used RX buffer handed up until the buffers are re-posted.
//! The driver therefore fills the TX descriptor as usual but, inside that
//! window, keeps the slot here instead of publishing it, and publishes the
//! held slots in submission order once the RX buffers are back with the
//! device. Outside the window a transmission is published at once.
//!
//! Pure bookkeeping: no lock, no device access. The driver holds it under its
//! own lock and does the publishing.

/// Up to `N` held TX slots, where `N` is the TX pool size: each held entry
/// owns a claimed slot, so the hold cannot need more room than that.
pub struct TxHold<const N: usize> {
    consuming: bool,
    held: [u16; N],
    len: usize,
}

impl<const N: usize> TxHold<N> {
    pub const fn new() -> Self {
        Self { consuming: false, held: [0; N], len: 0 }
    }

    /// The driver handed a received buffer up: RX consumption is under way.
    pub fn begin_rx(&mut self) {
        self.consuming = true;
    }

    /// A filled TX descriptor in `slot`. Returns the slot to publish now, or
    /// keeps it for [`end_rx`](Self::end_rx).
    pub fn submit(&mut self, slot: u16) -> Option<u16> {
        if !self.consuming {
            return Some(slot);
        }
        assert!(self.len < N, "a held TX slot was submitted twice");
        self.held[self.len] = slot;
        self.len += 1;
        None
    }

    /// The RX buffers are re-posted. Returns the held slots, oldest first,
    /// for the driver to publish now; the hold is empty afterwards.
    pub fn end_rx(&mut self) -> &[u16] {
        self.consuming = false;
        let held = self.len;
        self.len = 0;
        &self.held[..held]
    }
}
