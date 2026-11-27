//! The virtio-net PCI driver holds transmissions submitted while it is
//! consuming its RX ring and publishes them, in order, once the RX buffers
//! are re-posted (`kernel/src/drivers/virtio/tx_hold.rs`). That rule used to
//! live in TCP as a queue for SYN+ACKs only (audit locals, DEV-29(4)); it is
//! a property of one device, so it belongs to that device's driver.

#[path = "../kernel/src/drivers/virtio/tx_hold.rs"]
mod tx_hold;

use tx_hold::TxHold;

#[test]
fn transmissions_outside_rx_consumption_publish_at_once() {
    let mut hold = TxHold::<4>::new();
    assert_eq!(hold.submit(3), Some(3));
    assert_eq!(hold.submit(0), Some(0));
    assert!(hold.end_rx().is_empty());
}

#[test]
fn transmissions_during_rx_consumption_wait_for_it_and_keep_their_order() {
    let mut hold = TxHold::<4>::new();
    hold.begin_rx();
    assert_eq!(hold.submit(2), None);
    assert_eq!(hold.submit(0), None);
    hold.begin_rx(); // a second packet of the same batch
    assert_eq!(hold.submit(3), None);
    assert_eq!(hold.end_rx(), &[2, 0, 3]);
    // Consumption is over: the hold is empty, and the next one goes at once.
    assert!(hold.end_rx().is_empty());
    assert_eq!(hold.submit(1), Some(1));
}

#[test]
fn a_full_pool_of_held_slots_fits() {
    // Each held entry owns a claimed TX slot, so the hold needs no more
    // room than the pool has slots.
    let mut hold = TxHold::<4>::new();
    hold.begin_rx();
    for slot in 0..4 {
        assert_eq!(hold.submit(slot), None);
    }
    assert_eq!(hold.end_rx(), &[0, 1, 2, 3]);
}
