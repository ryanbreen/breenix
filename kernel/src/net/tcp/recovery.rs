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
    /// fast retransmit or retransmission timeout began, while it still guards
    /// recovery entry. Unset before the first retransmission, and once the
    /// cumulative ACK has passed the marker outside fast recovery (RFC 6582
    /// section 6): an ACK beyond it would pass the guard anyway, and a marker
    /// kept after 2^31 bytes of progress would compare as ahead of the ACK.
    recover: Option<u32>,
    /// Qualifying duplicate acknowledgements since the cumulative ACK last moved.
    dupacks: u32,
    /// Data bytes Limited Transmit sent since the cumulative ACK last moved,
    /// which recovery entry leaves out of FlightSize (RFC 3042 section 2).
    limited_sent: u32,
    /// In fast recovery.
    active: bool,
    /// This recovery episode has seen a partial acknowledgement.
    partial_seen: bool,
}

impl Recovery {
    pub fn new() -> Self {
        Recovery {
            recover: None,
            dupacks: 0,
            limited_sent: 0,
            active: false,
            partial_seen: false,
        }
    }

    pub fn recover(&self) -> Option<u32> {
        self.recover
    }

    pub fn dupacks(&self) -> u32 {
        self.dupacks
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// Limited Transmit (RFC 3042 section 2, RFC 5681 section 3.2): outside
    /// fast recovery, each of the first two duplicate acknowledgements lets
    /// one new segment of `len` bytes leave beyond the congestion window,
    /// which keeps the flight within cwnd + 2 * SMSS.
    pub fn limited_transmit_allows(&self, len: u32, smss: u32) -> bool {
        !self.active
            && (1..DUPACK_THRESHOLD).contains(&self.dupacks)
            && self.limited_sent.saturating_add(len) <= self.dupacks.saturating_mul(smss)
    }

    /// Limited Transmit sent `len` new data bytes.
    pub fn limited_transmit_sent(&mut self, len: u32) {
        self.limited_sent = self.limited_sent.saturating_add(len);
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
        if self.dupacks != DUPACK_THRESHOLD || !self.beyond_recover(ack) {
            return false;
        }
        self.recover = Some(highest_sent);
        self.active = true;
        self.partial_seen = false;
        // The threshold comes from the flight without the Limited Transmit
        // segments (RFC 3042 section 2).
        let flight = flight.saturating_sub(self.limited_sent);
        self.limited_sent = 0;
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
        self.limited_sent = 0;
        if !self.active {
            if self.beyond_recover(ack) {
                // Past the marker: it no longer guards anything, and if kept
                // it would go stale (RFC 6582 section 6).
                self.recover = None;
            }
            cc.on_ack(acked, smss);
            return Response { repair: false, restart_timer: true };
        }
        if !self.recover.is_some_and(|recover| seq_lt(ack.wrapping_sub(1), recover)) {
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
        self.recover = Some(highest_sent);
        self.dupacks = 0;
        self.limited_sent = 0;
        self.active = false;
        self.partial_seen = false;
    }

    /// The acknowledgement `ack` covers more than `recover` (RFC 6582
    /// section 4: `ack - 1 > recover`), or no marker guards entry.
    fn beyond_recover(&self, ack: u32) -> bool {
        self.recover
            .is_none_or(|recover| seq_lt(recover, ack.wrapping_sub(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMSS: u32 = 1460;

    /// RFC 6582 section 6: once the cumulative ACK has moved more than 2^31
    /// bytes past a `recover` set by a timeout, wrap-safe comparison puts
    /// that old marker ahead of the ACK. Three duplicates beyond everything
    /// the timeout marked must still start fast retransmit.
    #[test]
    fn a_marker_left_behind_by_two_gib_of_progress_does_not_block_recovery() {
        let mut cc = Controller::new_reno();
        cc.start(SMSS);
        let mut recovery = Recovery::new();
        let marked = 10 * SMSS - 1;
        recovery.timeout(&mut cc, marked, 10 * SMSS, SMSS, true);

        // The flight the timeout marked is acknowledged, then the ACK moves on
        // in steps of under 2^31 to just over 2^31 past the marker.
        let mut ack = marked.wrapping_add(1);
        recovery.advance(&mut cc, ack, 10 * SMSS, 0, SMSS);
        let target = marked.wrapping_add(1 << 31).wrapping_add(4 * SMSS);
        while ack != target {
            let step = target.wrapping_sub(ack).min(1 << 30);
            ack = ack.wrapping_add(step);
            recovery.advance(&mut cc, ack, step, 4 * SMSS, SMSS);
        }

        let highest_sent = ack.wrapping_add(4 * SMSS - 1);
        let entered = (0..3)
            .map(|_| recovery.duplicate(&mut cc, ack, highest_sent, 4 * SMSS, SMSS))
            .collect::<alloc::vec::Vec<_>>();
        assert_eq!(entered, [false, false, true], "the third duplicate's fast retransmit");
        assert!(recovery.active());
    }
}
