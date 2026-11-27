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
//! No lock and no device access of its own: the driver holds it under its own
//! lock and supplies its ring and doorbell writes as [`TxRings`], and the hold
//! decides their order.

/// The device operations whose order the hold decides, implemented by the
/// driver on its rings.
pub trait TxRings {
    /// Put the filled TX descriptor in `slot` on the TX avail ring.
    fn publish_tx(&mut self, slot: u16);
    /// Ring the TX queue's doorbell.
    fn notify_tx(&mut self);
    /// Offer the consumed RX buffers back on the RX avail ring.
    fn repost_rx(&mut self);
    /// Ring the RX queue's doorbell.
    fn notify_rx(&mut self);
}

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

    /// A filled TX descriptor in `slot`: published and notified now, or held
    /// until [`finish_rx`](Self::finish_rx) if RX is being consumed.
    pub fn transmit<R: TxRings>(&mut self, rings: &mut R, slot: u16) {
        if let Some(slot) = self.submit(slot) {
            rings.publish_tx(slot);
            rings.notify_tx();
        }
    }

    /// RX consumption is over: re-post and notify the RX buffers first, then
    /// publish the held slots in order and notify TX once, if any were held.
    pub fn finish_rx<R: TxRings>(&mut self, rings: &mut R) {
        rings.repost_rx();
        rings.notify_rx();
        let held = self.end_rx();
        for &slot in held {
            rings.publish_tx(slot);
        }
        if !held.is_empty() {
            rings.notify_tx();
        }
    }
}
