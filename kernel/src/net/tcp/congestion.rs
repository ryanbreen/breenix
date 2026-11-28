//! The TCP sender's congestion policy: how large a window it may keep in flight
//! and how that window answers acknowledgements, loss and idleness.
//!
//! The TCP core owns sequence numbers, the peer's window, the retransmission
//! timer and the choice of which range to repair. It reports events here: bytes
//! newly acknowledged, the flight size, the sender's maximum segment size
//! (SMSS), the start, progress and end of a loss recovery, a retransmission
//! timeout and an idle restart. The policy answers with the congestion window and the
//! slow-start threshold, the loss target recovery reduces to. Dispatch stays
//! inside the TCP module: a connection selects its policy when it is created,
//! and each policy answers the same events, so a later one (CUBIC) slots in
//! beside NewReno.
//!
//! NewReno follows RFC 5681 sections 3.1 and 3.2 and RFC 6582 section 3.2:
//! the initial window of RFC 5681 section 3.1 equation 1, slow start, additive
//! increase counted in acknowledged bytes (RFC 3465), the reduction to
//! `max(FlightSize / 2, 2 * SMSS)` on loss, window inflation by one SMSS per
//! duplicate acknowledgement in fast recovery, deflation on a partial
//! acknowledgement, and the restart window after an idle period (RFC 5681
//! section 4.1).

/// The initial window for `smss` (RFC 5681 section 3.1, equation 1).
pub(super) fn initial_window(smss: u32) -> u32 {
    let smss = smss.max(1);
    if smss > 2190 {
        2 * smss
    } else if smss > 1095 {
        3 * smss
    } else {
        4 * smss
    }
}

/// A connection's congestion policy, chosen when the connection is created.
pub(super) enum Controller {
    NewReno(NewReno),
}

impl Controller {
    /// The policy a new connection starts with.
    pub fn new_reno() -> Self {
        Controller::NewReno(NewReno::new())
    }

    /// The handshake set the SMSS: start from the initial window.
    pub fn start(&mut self, smss: u32) {
        match self {
            Controller::NewReno(p) => p.start(smss),
        }
    }

    /// Bytes the sender may have in flight.
    pub fn cwnd(&self) -> u32 {
        match self {
            Controller::NewReno(p) => p.cwnd,
        }
    }

    /// The slow-start threshold.
    pub fn ssthresh(&self) -> u32 {
        match self {
            Controller::NewReno(p) => p.ssthresh,
        }
    }

    /// An acknowledgement outside loss recovery newly acknowledged `acked`
    /// data bytes (SYN and FIN excluded).
    pub fn on_ack(&mut self, acked: u32, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_ack(acked, smss),
        }
    }

    /// Fast retransmit began with `flight` data bytes outstanding.
    pub fn on_enter_recovery(&mut self, flight: u32, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_enter_recovery(flight, smss),
        }
    }

    /// A further duplicate acknowledgement arrived during fast recovery.
    pub fn on_recovery_dupack(&mut self, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_recovery_dupack(smss),
        }
    }

    /// A partial acknowledgement newly acknowledged `acked` data bytes.
    pub fn on_partial_ack(&mut self, acked: u32, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_partial_ack(acked, smss),
        }
    }

    /// A full acknowledgement ended fast recovery with `flight` data bytes
    /// still outstanding.
    pub fn on_exit_recovery(&mut self, flight: u32, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_exit_recovery(flight, smss),
        }
    }

    /// The retransmission timer expired with `flight` data bytes outstanding.
    /// `first` is the first expiry for the oldest unacknowledged segment.
    pub fn on_timeout(&mut self, flight: u32, smss: u32, first: bool) {
        match self {
            Controller::NewReno(p) => p.on_timeout(flight, smss, first),
        }
    }

    /// The sender is about to send after an idle period longer than the
    /// retransmission timeout.
    pub fn on_idle_restart(&mut self, smss: u32) {
        match self {
            Controller::NewReno(p) => p.on_idle_restart(smss),
        }
    }
}

/// NewReno (RFC 5681, RFC 6582).
pub(super) struct NewReno {
    cwnd: u32,
    ssthresh: u32,
    /// Bytes acknowledged in congestion avoidance since cwnd last grew.
    bytes_acked: u32,
}

impl NewReno {
    fn new() -> Self {
        // Until the handshake gives the SMSS, the window the default 536-byte
        // segment would start with. The threshold starts arbitrarily high
        // (RFC 5681 section 3.1).
        NewReno {
            cwnd: initial_window(536),
            ssthresh: u32::MAX,
            bytes_acked: 0,
        }
    }

    fn start(&mut self, smss: u32) {
        self.cwnd = initial_window(smss);
        self.bytes_acked = 0;
    }

    fn loss_threshold(flight: u32, smss: u32) -> u32 {
        (flight / 2).max(smss.saturating_mul(2))
    }

    fn on_ack(&mut self, acked: u32, smss: u32) {
        if acked == 0 {
            return;
        }
        if self.cwnd < self.ssthresh {
            // Slow start: at most one SMSS per acknowledgement (equation 2).
            self.cwnd = self.cwnd.saturating_add(acked.min(smss));
        } else {
            // Congestion avoidance: one SMSS per window of acknowledged bytes.
            self.bytes_acked = self.bytes_acked.saturating_add(acked);
            if self.bytes_acked >= self.cwnd {
                self.bytes_acked -= self.cwnd;
                self.cwnd = self.cwnd.saturating_add(smss);
            }
        }
    }

    fn on_enter_recovery(&mut self, flight: u32, smss: u32) {
        self.ssthresh = Self::loss_threshold(flight, smss);
        self.cwnd = self.ssthresh.saturating_add(smss.saturating_mul(3));
        self.bytes_acked = 0;
    }

    fn on_recovery_dupack(&mut self, smss: u32) {
        self.cwnd = self.cwnd.saturating_add(smss);
    }

    fn on_partial_ack(&mut self, acked: u32, smss: u32) {
        self.cwnd = self.cwnd.saturating_sub(acked);
        if acked >= smss {
            self.cwnd = self.cwnd.saturating_add(smss);
        }
    }

    fn on_exit_recovery(&mut self, flight: u32, smss: u32) {
        self.cwnd = self.ssthresh.min(flight.max(smss).saturating_add(smss));
        self.bytes_acked = 0;
    }

    fn on_timeout(&mut self, flight: u32, smss: u32, first: bool) {
        if first {
            self.ssthresh = Self::loss_threshold(flight, smss);
        }
        // The loss window: one segment (RFC 5681 section 3.1).
        self.cwnd = smss;
        self.bytes_acked = 0;
    }

    fn on_idle_restart(&mut self, smss: u32) {
        self.cwnd = self.cwnd.min(initial_window(smss));
        self.bytes_acked = 0;
    }
}
