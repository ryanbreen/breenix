//! New data in whole segments leaves as one segmentation-offload batch when the
//! route's device takes one (virtio 1.2 section 5.1.6.2): the flight is debited
//! for the whole batch, the segments it stands for are counted, the checksum
//! field holds the pseudo-header sum, and a short tail, a refused batch and a
//! route without offload fall back to the segment-by-segment path.

use crate::harness::{self, data, stat, Flow, SMSS};

#[test]
fn whole_segments_leave_as_one_batch_and_a_short_tail_alone() {
    let _serial = harness::lock(2_000_000);
    harness::offer_batches(Some(64_000));
    let flow = Flow::connect(40101);
    let batches = stat("offload_batches");
    let segments = stat("offload_segments");

    // The three-segment initial window admits three whole segments; the
    // 700-byte tail waits for the window.
    flow.write((3 * SMSS + 700) as usize);
    let sent = harness::take_batches(40101);
    assert_eq!(sent, vec![(flow.at(0), (3 * SMSS) as usize, SMSS as u16)], "one batch");
    assert_eq!(data(&flow.sent()), vec![], "no segment left beside the batch");
    assert_eq!(stat("offload_batches"), batches + 1);
    assert_eq!(stat("offload_segments"), segments + 3);
    let info = flow.info();
    assert_eq!(info.send_unack, flow.at(0));
    assert_eq!(info.send_next, flow.at(3), "the flight is debited for the batch");
    assert_eq!(info.send_max, flow.at(3));
    assert_ne!(info.rtx_deadline_ms, 0, "the retransmission timer runs for it");

    // The ACK opens the window: the short tail goes alone, at once.
    harness::advance(10);
    flow.ack(flow.at(3));
    assert_eq!(harness::take_batches(40101), vec![]);
    assert_eq!(data(&flow.sent()), vec![(flow.at(3), 700)]);

    // A refused batch stays queued and goes when output is retried.
    harness::advance(10);
    flow.ack(flow.at(3) + 700);
    harness::refuse_output(true);
    flow.write((2 * SMSS) as usize);
    assert_eq!(harness::take_batches(40101), vec![]);
    harness::refuse_output(false);
    crate::net::tcp::drain_deferred_tx();
    assert_eq!(
        harness::take_batches(40101),
        vec![(flow.at(3) + 700, (2 * SMSS) as usize, SMSS as u16)],
        "the refused batch was retried"
    );

    // Without a device that segments, the same data goes segment by segment.
    harness::advance(10);
    flow.ack(flow.at(5) + 700);
    harness::offer_batches(None);
    flow.write((2 * SMSS) as usize);
    assert_eq!(harness::take_batches(40101), vec![]);
    let base = flow.at(5) + 700;
    assert_eq!(data(&flow.sent()), vec![(base, SMSS as usize), (base + SMSS, SMSS as usize)]);
    assert_eq!(stat("offload_segments"), segments + 5);
}
