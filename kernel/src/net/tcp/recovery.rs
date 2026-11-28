//! NewReno loss recovery for a sender without usable SACK feedback: when to
//! fast retransmit, when a recovery episode ends, and which acknowledgements
//! may not start another (RFC 5681 section 3.2, RFC 6582 sections 3.2 and 4).
//!
//! This decides which range to repair (the first unacknowledged segment,
//! for NewReno) and reports each event to the connection's congestion policy,
//! which sets the window. The TCP core validates the acknowledgement first and
//! sends what this asks for.
use super::congestion::Controller;
use super::seq_lt;

/// The duplicate acknowledgement that triggers fast retransmit.
const DUPACK_THRESHOLD: u32 = 3;

/// What the sender owes after an acknowledgement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Response {
    /// Retransmit the first unacknowledged segment.
    pub repair: bool,
    /// Restart the retransmission timer (RFC 6298 section 5.3). Within fast
    /// recovery only the first partial acknowledgement restarts it (RFC 6582
    /// section 3.2 step 3, the Impatient variant of section 4).
    pub restart_timer: bool,
}

pub(super) struct Recovery {
    /// RFC 6582's `recover`: the highest sequence number sent when the last
    /// fast retransmit or retransmission timeout began. It starts at the
    /// initial send sequence number.
    recover: u32,
    /// Qualifying duplicate acknowledgements since the cumulative ACK last moved.
    dupacks: u32,
    /// In fast recovery.
    active: bool,
    /// This recovery episode has seen a partial acknowledgement.
    partial_seen: bool,
}

impl Recovery {
    pub fn new(initial_seq: u32) -> Self {
        Recovery {
            recover: initial_seq,
            dupacks: 0,
            active: false,
            partial_seen: false,
        }
    }

    pub fn recover(&self) -> u32 {
        self.recover
    }

    pub fn dupacks(&self) -> u32 {
        self.dupacks
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// A qualifying duplicate acknowledgement (RFC 5681 section 2) of `ack`,
    /// with `flight` data bytes outstanding and `highest_sent` the highest
    /// sequence number sent. True when it starts fast retransmit: the caller
    /// repairs the first unacknowledged segment. During fast recovery each one
    /// inflates the window by one SMSS, which may admit new data.
    pub fn duplicate(
        &mut self,
        cc: &mut Controller,
        ack: u32,
        highest_sent: u32,
        flight: u32,
        smss: u32,
    ) -> bool {
        if self.active {
            cc.on_recovery_dupack(smss);
            return false;
        }
        self.dupacks = self.dupacks.saturating_add(1);
        // Only an acknowledgement beyond `recover` may start recovery, so the
        // duplicates a timeout's resends draw cannot (RFC 6582 section 3.2
        // step 1, section 4).
        if self.dupacks != DUPACK_THRESHOLD || !seq_lt(self.recover, ack.wrapping_sub(1)) {
            return false;
        }
        self.recover = highest_sent;
        self.active = true;
        self.partial_seen = false;
        cc.on_enter_recovery(flight, smss);
        true
    }

    /// The cumulative acknowledgement moved to `ack`, newly acknowledging
    /// `acked` data bytes and leaving `flight` outstanding.
    pub fn advance(
        &mut self,
        cc: &mut Controller,
        ack: u32,
        acked: u32,
        flight: u32,
        smss: u32,
    ) -> Response {
        self.dupacks = 0;
        if !self.active {
            cc.on_ack(acked, smss);
            return Response { repair: false, restart_timer: true };
        }
        if !seq_lt(ack.wrapping_sub(1), self.recover) {
            // A full acknowledgement: everything through `recover`.
            self.active = false;
            cc.on_exit_recovery(flight, smss);
            return Response { repair: false, restart_timer: true };
        }
        // A partial acknowledgement: the next hole is lost as well.
        cc.on_partial_ack(acked, smss);
        let first = !self.partial_seen;
        self.partial_seen = true;
        Response { repair: true, restart_timer: first }
    }

    /// The retransmission timer expired on data with `flight` bytes
    /// outstanding and `highest_sent` the highest sequence number sent. `first`
    /// is the first expiry for the oldest unacknowledged segment, the one that
    /// sets the threshold; later expiries for it hold the threshold
    /// (RFC 5681 section 3.1).
    pub fn timeout(
        &mut self,
        cc: &mut Controller,
        highest_sent: u32,
        flight: u32,
        smss: u32,
        first: bool,
    ) {
        cc.on_timeout(flight, smss, first);
        self.recover = highest_sent;
        self.dupacks = 0;
        self.active = false;
        self.partial_seen = false;
    }
}
