//! NewReno on the kernel's TCP sender (RFC 5681 sections 3.1-3.2 and 4.1,
//! RFC 6582 section 3.2, RFC 6298 sections 2 and 5), driven through its receive
//! path and timer pass with a fixed clock and scripted loss. SMSS is 1460
//! bytes, so the initial window is three segments.

use crate::harness::{self, data, stat, Flow, SMSS};
use crate::net::tcp;

/// One round trip of the scripted path.
const RTT_MS: u64 = 10;

/// Open from `port`, then grow the congestion window to `segments` SMSS in
/// slow start by sending that many segments less the initial three and
/// acknowledging each one alone. The data written is acknowledged by the end.
fn warmed_flow(port: u16, segments: u32) -> Flow {
    let flow = Flow::connect(port);
    let earlier = segments - 3;
    flow.write((earlier * SMSS) as usize);
    let sent = data(&flow.sent());
    // Admission: of the earlier data, only the initial window leaves at once.
    assert_eq!(
        sent,
        (0..3).map(|n| (flow.at(n), SMSS as usize)).collect::<Vec<_>>(),
        "the first write left beyond the three-segment initial window"
    );
    assert_eq!(flow.info().cwnd, 3 * SMSS);
    for n in 1..=earlier {
        harness::advance(RTT_MS);
        flow.ack(flow.at(n));
        // Slow start: each ACK of one SMSS opens the window by one SMSS.
        assert_eq!(flow.info().cwnd, (3 + n) * SMSS, "cwnd after ACK {n}");
    }
    assert_eq!(
        data(&flow.sent()).len() as u32,
        earlier - 3,
        "the rest of the earlier data left as ACKs opened the window"
    );
    let info = flow.info();
    assert_eq!((info.cwnd, info.ssthresh), (segments * SMSS, u32::MAX));
    assert_eq!(info.send_unack, flow.at(earlier));
    // RFC 6298 2.4: a computed timeout has a one-second floor.
    assert_eq!(info.rto_ms, 1_000, "the computed timeout's floor");
    flow
}

#[test]
fn three_duplicates_repair_the_first_loss_and_partial_acks_the_next() {
    let _serial = harness::lock(1_000_000);
    let mut flow = warmed_flow(40001, 8);
    let base = flow.at(5);
    let fast_before = stat("fast_retransmits");
    let partial_before = stat("partial_ack_retransmits");
    let recoveries_before = stat("fast_recoveries");

    // An eight-SMSS flight, and no further data queued.
    flow.write((8 * SMSS) as usize);
    let flight: Vec<_> = (0..8).map(|n| (base + n * SMSS, SMSS as usize)).collect();
    assert_eq!(data(&flow.sent()), flight, "the eight-segment flight the window admits");

    // Segments 1 and 4 are lost. Segments 2 and 3 draw two duplicates.
    harness::advance(RTT_MS);
    flow.ack(base);
    flow.ack(base);
    assert_eq!(flow.info().dupacks, 2);
    // A window update repeats the cumulative ACK but is not a duplicate.
    flow.set_window(65000);
    flow.ack(base);
    assert_eq!(flow.info().dupacks, 2, "a window update counted as a duplicate");
    assert!(data(&flow.sent()).is_empty(), "sent before the third duplicate");

    // Segment 5's duplicate is the third. IP output refuses the repair at
    // first: it stays owed and the flight is unchanged.
    harness::refuse_output(true);
    flow.ack(base);
    let info = flow.info();
    assert!(info.in_recovery, "the third duplicate did not start fast recovery");
    assert_eq!((info.cwnd, info.ssthresh), (7 * SMSS, 4 * SMSS), "(cwnd, ssthresh) on entry");
    assert_eq!(info.recover, base + 8 * SMSS - 1, "recover is the highest byte sent");
    assert_eq!((info.send_next, info.send_max), (base + 8 * SMSS, base + 8 * SMSS));
    harness::refuse_output(false);
    tcp::drain_deferred_tx();
    assert_eq!(data(&flow.sent()), vec![(base, SMSS as usize)], "the fast retransmit");

    // Segment 6's duplicate inflates the window; with no data queued, no segment goes.
    flow.ack(base);
    assert_eq!(flow.info().cwnd, 8 * SMSS);
    assert!(data(&flow.sent()).is_empty());

    // The repair arrives: a partial ACK of three SMSS. The window deflates by
    // three and regains one, the next hole is repaired, the timer restarts.
    harness::advance(RTT_MS);
    flow.ack(base + 3 * SMSS);
    let info = flow.info();
    assert!(info.in_recovery, "a partial ACK ended fast recovery");
    assert_eq!(info.cwnd, 6 * SMSS, "cwnd after the partial ACK");
    assert_eq!(info.ssthresh, 4 * SMSS);
    assert_eq!(info.rtx_deadline_ms, harness::now_ms() + info.rto_ms, "the first partial ACK restarts the timer");
    assert_eq!(data(&flow.sent()), vec![(base + 3 * SMSS, SMSS as usize)], "the partial ACK's repair");

    // The second repair arrives: a full ACK through recover.
    harness::advance(RTT_MS);
    flow.ack(base + 8 * SMSS);
    let info = flow.info();
    assert!(!info.in_recovery, "the full ACK did not end fast recovery");
    assert_eq!(info.cwnd, 2 * SMSS, "min(ssthresh, max(FlightSize, SMSS) + SMSS)");
    assert_eq!(info.ssthresh, 4 * SMSS);
    assert!(data(&flow.sent()).is_empty());

    assert_eq!(stat("fast_retransmits") - fast_before, 1);
    assert_eq!(stat("partial_ack_retransmits") - partial_before, 1);
    assert_eq!(stat("fast_recoveries") - recoveries_before, 1);
}

#[test]
fn a_timeout_repairs_the_oldest_segment_and_marks_recover() {
    let _serial = harness::lock(2_000_000);
    let flow = warmed_flow(40002, 12);
    let base = flow.at(9);
    let timeouts_before = stat("retransmit_timeouts");
    let fast_before = stat("fast_retransmits");
    let resent_before = stat("timeout_retransmits");

    flow.write((12 * SMSS) as usize);
    assert_eq!(data(&flow.sent()).len(), 12, "the twelve-segment flight");
    let deadline = flow.info().rtx_deadline_ms;
    assert_eq!(deadline, harness::now_ms() + 1_000);

    // The peer acknowledges no segment: the timer expires.
    harness::set_clock(deadline);
    tcp::run_timers();
    let info = flow.info();
    assert_eq!(data(&flow.sent()), vec![(base, SMSS as usize)], "only the oldest segment goes again");
    assert_eq!((info.cwnd, info.ssthresh), (SMSS, 6 * SMSS), "(cwnd, ssthresh) after the timeout");
    assert_eq!(info.recover, base + 12 * SMSS - 1, "recover marks the highest byte sent");
    assert!(!info.in_recovery);
    assert_eq!(info.rto_ms, 2_000, "the timeout backs off");
    assert_eq!(info.rtx_deadline_ms, deadline + 2_000);
    assert_eq!(stat("retransmit_timeouts") - timeouts_before, 1);
    assert_eq!(stat("timeout_retransmits") - resent_before, 1);

    // Three duplicates inside the marked flight start no fast retransmit and
    // move no threshold.
    harness::advance(RTT_MS);
    for _ in 0..3 {
        flow.ack(base);
    }
    let info = flow.info();
    assert!(!info.in_recovery, "duplicates within recover started fast recovery");
    assert_eq!((info.cwnd, info.ssthresh), (SMSS, 6 * SMSS));
    assert!(data(&flow.sent()).is_empty(), "duplicates within recover drew a retransmission");
    assert_eq!(stat("fast_retransmits"), fast_before);

    // The same segment expires again: the threshold holds.
    harness::set_clock(info.rtx_deadline_ms);
    tcp::run_timers();
    let info = flow.info();
    assert_eq!(data(&flow.sent()), vec![(base, SMSS as usize)]);
    assert_eq!((info.cwnd, info.ssthresh), (SMSS, 6 * SMSS), "the second expiry moved the threshold");
    assert_eq!(info.rto_ms, 4_000);

    // Everything is acknowledged, past recover. New data, one loss among it,
    // and three duplicates beyond recover start a fresh recovery.
    harness::advance(RTT_MS);
    let next = base + 12 * SMSS;
    flow.ack(next);
    assert_eq!(flow.info().cwnd, 2 * SMSS, "slow start after the timeout");
    flow.write((6 * SMSS) as usize);
    assert_eq!(data(&flow.sent()).len(), 2);
    harness::advance(RTT_MS);
    flow.ack(next + SMSS);
    assert_eq!(data(&flow.sent()).len(), 2);
    harness::advance(RTT_MS);
    flow.ack(next + 2 * SMSS);
    let sent = data(&flow.sent());
    assert_eq!(sent.len(), 2);
    assert_eq!(flow.info().send_max, next + 6 * SMSS, "four segments are out");
    // The segment at next + 2 SMSS is lost; the three behind it draw duplicates.
    harness::advance(RTT_MS);
    for _ in 0..3 {
        flow.ack(next + 2 * SMSS);
    }
    let info = flow.info();
    assert!(info.in_recovery, "duplicates beyond recover did not start fast recovery");
    assert_eq!((info.cwnd, info.ssthresh), (5 * SMSS, 2 * SMSS));
    assert_eq!(info.recover, next + 6 * SMSS - 1);
    assert_eq!(data(&flow.sent()), vec![(next + 2 * SMSS, SMSS as usize)], "the fresh fast retransmit");
    assert_eq!(stat("fast_retransmits") - fast_before, 1);

    // The repair lands and the peer acknowledges the rest: no data is left
    // outstanding, so no timer of this connection runs into another check.
    harness::advance(RTT_MS);
    flow.ack(next + 6 * SMSS);
    let info = flow.info();
    assert!(!info.in_recovery);
    assert_eq!(info.rtx_deadline_ms, 0);
}

#[test]
fn a_lost_first_segment_fast_retransmits() {
    // SMSS 1000 gives a four-segment initial window (RFC 5681 3.1), so the
    // three segments behind a lost first one draw three duplicates.
    let _serial = harness::lock(3_000_000);
    let smss = 1000;
    let flow = Flow::open(40003, smss as u16, false);
    let start = flow.data_start;
    let fast_before = stat("fast_retransmits");

    flow.write(4 * smss);
    let flight: Vec<_> = (0..4).map(|n| (start + n * smss as u32, smss)).collect();
    assert_eq!(data(&flow.sent()), flight, "the four-segment initial window");

    harness::advance(RTT_MS);
    for _ in 0..3 {
        flow.ack(start);
    }
    let info = flow.info();
    assert!(info.in_recovery, "three duplicates of the first data byte started no recovery");
    assert_eq!(data(&flow.sent()), vec![(start, smss)], "the first segment's fast retransmit");
    assert_eq!(stat("fast_retransmits") - fast_before, 1);

    harness::advance(RTT_MS);
    flow.ack(start + 4 * smss as u32);
    let info = flow.info();
    assert!(!info.in_recovery);
    assert_eq!(info.rtx_deadline_ms, 0);
}

#[test]
fn limited_transmit_sends_new_data_on_the_first_two_duplicates() {
    // RFC 3042 and RFC 5681 3.2: after an idle restart the window is three
    // segments, so one loss leaves only two to draw duplicates. Each of the
    // first two sends one new segment without moving cwnd, and the third
    // duplicate that draws starts fast retransmit. The threshold is taken
    // from the flight without the Limited Transmit segments.
    let _serial = harness::lock(4_000_000);
    let flow = warmed_flow(40004, 6);
    let base = flow.at(3);
    harness::advance(2_000);
    flow.write((6 * SMSS) as usize);
    let flight: Vec<_> = (0..3).map(|n| (base + n * SMSS, SMSS as usize)).collect();
    assert_eq!(data(&flow.sent()), flight, "the idle restart's three-segment window");
    assert_eq!(flow.info().cwnd, 3 * SMSS);

    harness::advance(RTT_MS);
    flow.ack(base);
    assert_eq!(data(&flow.sent()), vec![(base + 3 * SMSS, SMSS as usize)], "the first duplicate's segment");
    assert_eq!(flow.info().cwnd, 3 * SMSS, "Limited Transmit moved cwnd");
    flow.ack(base);
    assert_eq!(data(&flow.sent()), vec![(base + 4 * SMSS, SMSS as usize)], "the second duplicate's segment");
    assert_eq!(flow.info().cwnd, 3 * SMSS, "Limited Transmit moved cwnd");

    flow.ack(base);
    let info = flow.info();
    assert!(info.in_recovery, "the third duplicate started no recovery");
    assert_eq!(data(&flow.sent()), vec![(base, SMSS as usize)], "the fast retransmit");
    assert_eq!((info.cwnd, info.ssthresh), (5 * SMSS, 2 * SMSS), "(cwnd, ssthresh) on entry");

    // The full ACK ends recovery; the last queued segment goes, and its ACK
    // stops the retransmission timer.
    harness::advance(RTT_MS);
    flow.ack(base + 5 * SMSS);
    assert!(!flow.info().in_recovery);
    assert_eq!(data(&flow.sent()), vec![(base + 5 * SMSS, SMSS as usize)]);
    harness::advance(RTT_MS);
    flow.ack(base + 6 * SMSS);
    assert_eq!(flow.info().rtx_deadline_ms, 0);
}

#[test]
fn a_lost_syn_starts_data_with_one_segment() {
    // RFC 5681 3.1: when the SYN or SYN+ACK was lost, the initial window
    // after the handshake is one segment.
    let _serial = harness::lock(5_000_000);
    let flow = Flow::open(40005, SMSS as u16, true);
    let info = flow.info();
    assert_eq!(info.cwnd, SMSS, "the window after a lost SYN");
    assert_eq!(info.rto_ms, 3_000, "RFC 6298 5.7's timeout after a lost SYN");

    flow.write((3 * SMSS) as usize);
    assert_eq!(data(&flow.sent()), vec![(flow.at(0), SMSS as usize)], "one segment leaves");
    harness::advance(RTT_MS);
    flow.ack(flow.at(1));
    assert_eq!(flow.info().cwnd, 2 * SMSS, "slow start from one segment");
    assert_eq!(data(&flow.sent()), vec![(flow.at(1), SMSS as usize), (flow.at(2), SMSS as usize)]);
    harness::advance(RTT_MS);
    flow.ack(flow.at(3));
    assert_eq!(flow.info().rtx_deadline_ms, 0);
}
