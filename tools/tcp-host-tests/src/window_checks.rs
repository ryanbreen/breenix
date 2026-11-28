//! The receiver's window update after a read (RFC 9293 3.8.6.2.2): once the
//! reader frees a segment's worth or more of a nearly full buffer, the update
//! leaves at once, even when a delayed ACK is pending. Waiting for the
//! delayed-ACK timer holds the peer at the shrunken window for that timer's
//! 10 ms each time the window fills.

use crate::harness::{self, Flow, SMSS};

#[test]
fn a_read_that_reopens_the_window_sends_the_update_without_waiting_for_the_delayed_ack() {
    let _serial = harness::lock(3_000_000);
    let mut flow = Flow::connect(40301);
    flow.sent();

    // Full segments in order, within the window the receiver advertises:
    // segments 2, 4, 6 and so on are acknowledged at once. Stop on an odd segment, whose
    // ACK is delayed, once the advertised window is below three segments.
    let mut segments = 0;
    let shrunk = loop {
        flow.send(SMSS as usize);
        segments += 1;
        let acks = flow.sent();
        let window = acks.last().map(|ack| ack.window);
        if segments % 2 == 0 {
            let ack = acks.last().expect("every second segment is acknowledged at once");
            assert_eq!(ack.ack, flow.peer_next(), "segment {segments}'s ACK");
        } else {
            assert_eq!(acks, vec![], "segment {segments}'s ACK is delayed");
        }
        if let Some(window) = window {
            if u32::from(window) < 3 * SMSS {
                flow.send(SMSS as usize);
                segments += 1;
                assert_eq!(flow.sent(), vec![], "segment {segments}'s ACK is delayed");
                break window;
            }
        }
        assert!(segments < 200, "the window never shrank");
    };

    // The application reads what arrived. No time passes.
    let mut read = 0;
    while read < segments * SMSS as usize {
        let n = flow.read(65536);
        assert_ne!(n, 0, "the data is readable");
        read += n;
    }
    let update = flow.sent();
    assert_eq!(update.len(), 1, "one window update after the read: {update:?}");
    assert_eq!(update[0].ack, flow.peer_next(), "it acknowledges the last segment");
    assert!(update[0].window > shrunk, "it reopens the window: {} after {shrunk}", update[0].window);
}
